//! Helpers that are shared by the integration tests:
//!
//! * a tiny PNG writer (stored DEFLATE blocks, no compression),
//! * a checker for the XML syntax of the output (requirement TEST-05),
//! * a rasterizer for the subset of SVG that png2svg writes,
//! * the SSIM image similarity metric.
//!
//! The rasterizer draws the thin overlap stroke too (as a union of rectangles
//! along the outline, without round joins). It samples every pixel with an 8x8
//! grid, so it shows the same light seams between neighbors that a browser
//! shows when the stroke is missing.
#![allow(dead_code)]

use std::path::PathBuf;

use png2svg::png::checksum::{adler32, crc32_update};

/// Absolute path of a file in `tests/fixtures`.
pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Names of the test images (requirement QA-01).
pub const FIXTURES: [&str; 7] = [
    "logo",
    "icon",
    "text",
    "screenshot",
    "pixelart",
    "photo",
    "gradient",
];

/// Creates a PNG file (8-bit RGBA) from pixels.
pub fn write_png_rgba(width: usize, height: usize, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(rgba.len(), width * height * 4);
    // Scanlines with filter type 0.
    let mut raw = Vec::with_capacity(height * (width * 4 + 1));
    for row in rgba.chunks(width * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    // zlib stream made of stored blocks.
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = png2svg::png::SIGNATURE.to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    push_chunk(&mut out, b"IHDR", &ihdr);
    push_chunk(&mut out, b"IDAT", &z);
    push_chunk(&mut out, b"IEND", &[]);
    out
}

/// Appends one PNG chunk with its CRC.
pub fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32_update(png2svg::png::checksum::crc32(kind), data);
    out.extend_from_slice(&crc.to_be_bytes());
}

// ---------------------------------------------------------------------------
// XML check
// ---------------------------------------------------------------------------

/// Checks that the text is well-formed XML in the simple form that png2svg
/// writes: elements with quoted attributes, no text, no comments. Returns the
/// number of elements.
pub fn check_xml(text: &str) -> Result<usize, String> {
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut stack: Vec<String> = Vec::new();
    let mut elements = 0;
    let mut root_closed = false;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            if !bytes[i].is_ascii_whitespace() {
                return Err(format!("text outside of an element at byte {i}"));
            }
            i += 1;
            continue;
        }
        let end = text[i..].find('>').ok_or("tag is not closed")? + i;
        let tag = &text[i + 1..end];
        i = end + 1;
        if root_closed {
            return Err("content after the root element".to_string());
        }
        if let Some(name) = tag.strip_prefix('/') {
            match stack.pop() {
                Some(open) if open == name.trim() => {}
                other => return Err(format!("closing tag {name} does not match {other:?}")),
            }
            if stack.is_empty() {
                root_closed = true;
            }
            continue;
        }
        let self_closing = tag.ends_with('/');
        let body = if self_closing {
            &tag[..tag.len() - 1]
        } else {
            tag
        };
        let name_end = body.find(char::is_whitespace).unwrap_or(body.len());
        let name = &body[..name_end];
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == ':' || c == '-')
        {
            return Err(format!("bad element name {name:?}"));
        }
        // Attributes: name="value" pairs separated by white space.
        let mut rest = body[name_end..].trim_start();
        while !rest.is_empty() {
            let eq = rest.find('=').ok_or("attribute without value")?;
            let attr = rest[..eq].trim();
            if attr.is_empty()
                || !attr
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == ':' || c == '-')
            {
                return Err(format!("bad attribute name {attr:?}"));
            }
            let after = rest[eq + 1..].trim_start();
            if !after.starts_with('"') {
                return Err("attribute value is not quoted".to_string());
            }
            let close = after[1..]
                .find('"')
                .ok_or("attribute value is not closed")?
                + 1;
            let value = &after[1..close];
            if value.contains('<') || value.contains('&') {
                return Err("forbidden character in an attribute value".to_string());
            }
            rest = after[close + 1..].trim_start();
        }
        elements += 1;
        if self_closing {
            if stack.is_empty() {
                root_closed = true;
            }
        } else {
            stack.push(name.to_string());
        }
    }
    if !stack.is_empty() {
        return Err(format!("unclosed elements: {stack:?}"));
    }
    if elements == 0 {
        return Err("no elements".to_string());
    }
    Ok(elements)
}

// ---------------------------------------------------------------------------
// SVG parsing and rasterization
// ---------------------------------------------------------------------------

/// A filled path of the SVG.
#[derive(Debug, Clone)]
pub struct SvgPath {
    /// Fill color.
    pub rgb: [u8; 3],
    /// Value of `fill-opacity`.
    pub opacity: f64,
    /// Flattened subpaths (closed polygons).
    pub polygons: Vec<Vec<(f64, f64)>>,
    /// Width of the stroke in pixels, or 0 if the path has no stroke.
    pub stroke_width: f64,
}

/// A parsed SVG image.
#[derive(Debug, Clone)]
pub struct Svg {
    pub width: usize,
    pub height: usize,
    pub paths: Vec<SvgPath>,
}

/// Reads the value of an attribute of a tag text.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

/// Parses a color like `#abc` or `#aabbcc`.
fn parse_color(text: &str) -> [u8; 3] {
    let hex = text.trim_start_matches('#');
    let digit = |s: &str| u8::from_str_radix(s, 16).expect("hex digit");
    match hex.len() {
        3 => {
            let d = |i: usize| digit(&hex[i..i + 1]) * 17;
            [d(0), d(1), d(2)]
        }
        6 => [digit(&hex[0..2]), digit(&hex[2..4]), digit(&hex[4..6])],
        _ => panic!("unsupported color {text}"),
    }
}

/// Parses the SVG text that png2svg writes.
pub fn parse_svg(text: &str) -> Svg {
    let root_end = text.find('>').expect("svg tag");
    let root = &text[..root_end];
    let view_box: Vec<f64> = attribute(root, "viewBox")
        .expect("viewBox")
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    let width = view_box[2] as usize;
    let height = view_box[3] as usize;
    // The stroke width is set once on the group element.
    let group_width: f64 = text
        .find("<g ")
        .and_then(|p| {
            let end = text[p..].find('>')? + p;
            attribute(&text[p..end], "stroke-width").and_then(|v| v.parse().ok())
        })
        .unwrap_or(1.0);
    let mut paths = Vec::new();
    let mut rest = text;
    while let Some(pos) = rest.find("<path") {
        let end = rest[pos..].find("/>").expect("path end") + pos;
        let tag = &rest[pos..end];
        let rgb = attribute(tag, "fill").map(parse_color).unwrap_or([0, 0, 0]);
        let opacity = attribute(tag, "fill-opacity")
            .map(|v| v.parse().unwrap())
            .unwrap_or(1.0);
        let d = attribute(tag, "d").expect("d attribute");
        paths.push(SvgPath {
            rgb,
            opacity,
            polygons: parse_path_data(d),
            stroke_width: if attribute(tag, "stroke").is_some() {
                group_width
            } else {
                0.0
            },
        });
        rest = &rest[end + 2..];
    }
    Svg {
        width,
        height,
        paths,
    }
}

/// Reads a number from the path data starting at `*i`.
fn read_number(d: &[u8], i: &mut usize) -> f64 {
    while *i < d.len() && (d[*i] == b' ' || d[*i] == b',') {
        *i += 1;
    }
    let start = *i;
    if *i < d.len() && (d[*i] == b'-' || d[*i] == b'+') {
        *i += 1;
    }
    let mut seen_dot = false;
    while *i < d.len() {
        match d[*i] {
            b'0'..=b'9' => *i += 1,
            b'.' if !seen_dot => {
                seen_dot = true;
                *i += 1;
            }
            _ => break,
        }
    }
    std::str::from_utf8(&d[start..*i])
        .unwrap()
        .parse()
        .unwrap_or_else(|_| panic!("bad number at {start} in {}", String::from_utf8_lossy(d)))
}

/// Parses path data (M, L, H, V, C, S, Z in both cases) into closed polygons.
/// Curves are flattened into 24 line segments.
pub fn parse_path_data(d: &str) -> Vec<Vec<(f64, f64)>> {
    let bytes = d.as_bytes();
    let mut i = 0;
    let mut polygons: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut current: Vec<(f64, f64)> = Vec::new();
    let (mut x, mut y) = (0.0f64, 0.0f64);
    let (mut start_x, mut start_y) = (0.0f64, 0.0f64);
    let mut last_ctrl: Option<(f64, f64)> = None;
    let mut command = b' ';
    loop {
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b',') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i].is_ascii_alphabetic() {
            command = bytes[i];
            i += 1;
            if command == b'z' || command == b'Z' {
                if !current.is_empty() {
                    polygons.push(std::mem::take(&mut current));
                }
                x = start_x;
                y = start_y;
                last_ctrl = None;
                continue;
            }
        }
        let relative = command.is_ascii_lowercase();
        let (ox, oy) = if relative { (x, y) } else { (0.0, 0.0) };
        match command.to_ascii_uppercase() {
            b'M' => {
                if !current.is_empty() {
                    polygons.push(std::mem::take(&mut current));
                }
                x = ox + read_number(bytes, &mut i);
                y = oy + read_number(bytes, &mut i);
                start_x = x;
                start_y = y;
                current.push((x, y));
                // Further pairs after M are implicit L commands.
                command = if relative { b'l' } else { b'L' };
                last_ctrl = None;
            }
            b'L' => {
                x = ox + read_number(bytes, &mut i);
                y = oy + read_number(bytes, &mut i);
                current.push((x, y));
                last_ctrl = None;
            }
            b'H' => {
                x = ox + read_number(bytes, &mut i);
                current.push((x, y));
                last_ctrl = None;
            }
            b'V' => {
                y = oy + read_number(bytes, &mut i);
                current.push((x, y));
                last_ctrl = None;
            }
            b'C' | b'S' => {
                let c1 = if command.eq_ignore_ascii_case(&b'C') {
                    (
                        ox + read_number(bytes, &mut i),
                        oy + read_number(bytes, &mut i),
                    )
                } else {
                    match last_ctrl {
                        Some((cx, cy)) => (2.0 * x - cx, 2.0 * y - cy),
                        None => (x, y),
                    }
                };
                let c2 = (
                    ox + read_number(bytes, &mut i),
                    oy + read_number(bytes, &mut i),
                );
                let end = (
                    ox + read_number(bytes, &mut i),
                    oy + read_number(bytes, &mut i),
                );
                let (p0, steps) = ((x, y), 24);
                for k in 1..=steps {
                    let t = f64::from(k) / f64::from(steps);
                    let s = 1.0 - t;
                    let b = [s * s * s, 3.0 * t * s * s, 3.0 * t * t * s, t * t * t];
                    current.push((
                        p0.0 * b[0] + c1.0 * b[1] + c2.0 * b[2] + end.0 * b[3],
                        p0.1 * b[0] + c1.1 * b[1] + c2.1 * b[2] + end.1 * b[3],
                    ));
                }
                x = end.0;
                y = end.1;
                last_ctrl = Some(c2);
            }
            other => panic!("unsupported path command {}", other as char),
        }
    }
    if !current.is_empty() {
        polygons.push(current);
    }
    polygons
}

/// Number of samples per pixel side used by [`rasterize`].
const SAMPLES: usize = 8;

/// Rasterizes an SVG to RGBA (straight alpha) pixels, with 8x8 samples per
/// pixel and with the overlap stroke.
pub fn rasterize(svg: &Svg) -> Vec<u8> {
    rasterize_with(svg, SAMPLES, true)
}

/// Rasterizes with one sample at the center of every pixel and without the
/// stroke. The result shows the exact geometry of the fill shapes.
pub fn rasterize_centers(svg: &Svg) -> Vec<u8> {
    rasterize_with(svg, 1, false)
}

/// Rasterizes an SVG with `samples` x `samples` samples per pixel.
pub fn rasterize_with(svg: &Svg, samples: usize, with_stroke: bool) -> Vec<u8> {
    let (w, h) = (svg.width, svg.height);
    // Accumulated premultiplied color (0..255 scale) and alpha (0..1).
    let mut color = vec![[0.0f64; 3]; w * h];
    let mut alpha = vec![0.0f64; w * h];
    let mut coverage = vec![0u8; w * h];
    for path in &svg.paths {
        coverage.iter_mut().for_each(|c| *c = 0);
        let mut hits = fill_nonzero(&path.polygons, w, h, samples);
        if with_stroke && path.stroke_width > 0.0 {
            let band = stroke_polygons(&path.polygons, path.stroke_width / 2.0);
            for (a, b) in hits.iter_mut().zip(fill_nonzero(&band, w, h, samples)) {
                *a |= b;
            }
        }
        for (i, hit) in hits.iter().enumerate() {
            if *hit {
                let (sx, sy) = (i % (w * samples), i / (w * samples));
                coverage[(sy / samples) * w + sx / samples] += 1;
            }
        }
        for i in 0..w * h {
            if coverage[i] == 0 {
                continue;
            }
            let a = path.opacity * f64::from(coverage[i]) / (samples * samples) as f64;
            for c in 0..3 {
                color[i][c] = color[i][c] * (1.0 - a) + f64::from(path.rgb[c]) * a;
            }
            alpha[i] = alpha[i] * (1.0 - a) + a;
        }
    }
    let mut out = vec![0u8; w * h * 4];
    for i in 0..w * h {
        if alpha[i] > 0.0 {
            for c in 0..3 {
                out[i * 4 + c] = (color[i][c] / alpha[i]).round().clamp(0.0, 255.0) as u8;
            }
            out[i * 4 + 3] = (alpha[i] * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Builds one rectangle along every edge of the polygons. All rectangles have
/// the same orientation, so their union is the stroke (non-zero rule).
fn stroke_polygons(polygons: &[Vec<(f64, f64)>], half_width: f64) -> Vec<Vec<(f64, f64)>> {
    let mut out = Vec::new();
    for poly in polygons {
        for k in 0..poly.len() {
            let (a, b) = (poly[k], poly[(k + 1) % poly.len()]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt();
            if len == 0.0 {
                continue;
            }
            let (nx, ny) = (-dy / len * half_width, dx / len * half_width);
            // The corners go around in one fixed direction for every edge.
            out.push(vec![
                (a.0 + nx, a.1 + ny),
                (b.0 + nx, b.1 + ny),
                (b.0 - nx, b.1 - ny),
                (a.0 - nx, a.1 - ny),
            ]);
        }
    }
    out
}

/// Fills polygons with the non-zero winding rule. The result has one flag for
/// every sample of the `(w * per_side) x (h * per_side)` grid.
fn fill_nonzero(polygons: &[Vec<(f64, f64)>], w: usize, h: usize, per_side: usize) -> Vec<bool> {
    let row = w * per_side;
    let mut samples = vec![false; row * h * per_side];
    // Edges: (x0, y0, x1, y1) with direction as winding sign.
    let mut edges: Vec<(f64, f64, f64, f64)> = Vec::new();
    for poly in polygons {
        for k in 0..poly.len() {
            let a = poly[k];
            let b = poly[(k + 1) % poly.len()];
            if a.1 != b.1 {
                edges.push((a.0, a.1, b.0, b.1));
            }
        }
    }
    let mut crossings: Vec<(f64, i32)> = Vec::new();
    for sy in 0..h * per_side {
        let yc = (sy as f64 + 0.5) / per_side as f64;
        crossings.clear();
        for &(x0, y0, x1, y1) in &edges {
            let (lo, hi, dir) = if y0 < y1 { (y0, y1, 1) } else { (y1, y0, -1) };
            if yc >= lo && yc < hi {
                let t = (yc - y0) / (y1 - y0);
                crossings.push((x0 + t * (x1 - x0), dir));
            }
        }
        crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let mut winding = 0;
        for k in 0..crossings.len() {
            winding += crossings[k].1;
            if winding != 0 && k + 1 < crossings.len() {
                // Sub-pixel columns whose centers lie inside [xa, xb).
                let (xa, xb) = (crossings[k].0, crossings[k + 1].0);
                let first = ((xa * per_side as f64) - 0.5).ceil().max(0.0) as usize;
                let last = (((xb * per_side as f64) - 0.5).ceil() as i64).min(row as i64);
                for sx in first..last.max(0) as usize {
                    samples[sy * row + sx] = true;
                }
            }
        }
    }
    samples
}

// ---------------------------------------------------------------------------
// SSIM
// ---------------------------------------------------------------------------

/// Converts RGBA to luma (0..255) after compositing over a background level.
fn luma_over(rgba: &[u8], background: f64) -> Vec<f64> {
    rgba.chunks(4)
        .map(|p| {
            let a = f64::from(p[3]) / 255.0;
            let mix = |c: u8| f64::from(c) * a + background * (1.0 - a);
            0.299 * mix(p[0]) + 0.587 * mix(p[1]) + 0.114 * mix(p[2])
        })
        .collect()
}

/// Mean SSIM of two RGBA images of equal size. The images are composited over
/// white and over black, and the lower of the two scores is returned, so a
/// difference in transparency is also noticed.
pub fn ssim_rgba(a: &[u8], b: &[u8], width: usize, height: usize) -> f64 {
    let over_white = ssim_gray(&luma_over(a, 255.0), &luma_over(b, 255.0), width, height);
    let over_black = ssim_gray(&luma_over(a, 0.0), &luma_over(b, 0.0), width, height);
    over_white.min(over_black)
}

/// Mean SSIM of two gray images, using a 7x7 uniform window.
pub fn ssim_gray(a: &[f64], b: &[f64], width: usize, height: usize) -> f64 {
    const WIN: usize = 7;
    if width < WIN || height < WIN {
        // Too small for a window: use the global statistics.
        let mse: f64 =
            a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f64>() / a.len() as f64;
        return 1.0 - (mse / (255.0 * 255.0)).sqrt();
    }
    let c1 = (0.01f64 * 255.0).powi(2);
    let c2 = (0.03f64 * 255.0).powi(2);
    // Integral images of a, b, a*a, b*b and a*b.
    let stride = width + 1;
    let integral = |f: &dyn Fn(usize) -> f64| -> Vec<f64> {
        let mut t = vec![0.0; (width + 1) * (height + 1)];
        for y in 0..height {
            let mut row = 0.0;
            for x in 0..width {
                row += f(y * width + x);
                t[(y + 1) * stride + x + 1] = t[y * stride + x + 1] + row;
            }
        }
        t
    };
    let sa = integral(&|i| a[i]);
    let sb = integral(&|i| b[i]);
    let saa = integral(&|i| a[i] * a[i]);
    let sbb = integral(&|i| b[i] * b[i]);
    let sab = integral(&|i| a[i] * b[i]);
    let window_sum = |t: &Vec<f64>, x: usize, y: usize| -> f64 {
        t[(y + WIN) * stride + x + WIN] - t[y * stride + x + WIN] - t[(y + WIN) * stride + x]
            + t[y * stride + x]
    };
    let n = (WIN * WIN) as f64;
    let mut total = 0.0;
    let mut count = 0.0;
    for y in 0..=height - WIN {
        for x in 0..=width - WIN {
            let ma = window_sum(&sa, x, y) / n;
            let mb = window_sum(&sb, x, y) / n;
            let va = (window_sum(&saa, x, y) / n - ma * ma).max(0.0);
            let vb = (window_sum(&sbb, x, y) / n - mb * mb).max(0.0);
            let cov = window_sum(&sab, x, y) / n - ma * mb;
            total += ((2.0 * ma * mb + c1) * (2.0 * cov + c2))
                / ((ma * ma + mb * mb + c1) * (va + vb + c2));
            count += 1.0;
        }
    }
    total / count
}
