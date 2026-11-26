//! Writing of the SVG text. The output is as short as possible.
//!
//! Space saving rules (requirements SVG-08 to SVG-16):
//!
//! * no XML declaration, no comments, no metadata, no extra white space,
//! * one `<path>` per color,
//! * colors as `#abc` when possible, black (`#000`, the default) is not written,
//! * numbers without extra zeros: `.5`, `-.5`, `1.25`, `3`,
//! * no separator where it is not needed: `1-2`, `.5.5`,
//! * for every path command, the shorter of the absolute and the relative
//!   form is used, `h` and `v` for horizontal and vertical lines, `s` for a
//!   smooth continuation of a curve, and the command letter is not repeated.

use crate::bezier::Seg;
use crate::vectorize::{Layer, Subpath, Vector};

/// Width of the thin stroke that makes neighbors overlap (see
/// [`crate::vectorize`]), in pixels.
const OVERLAP_STROKE_WIDTH: &str = ".3";

/// Creates the complete SVG document for an image of the given size.
pub fn write_svg(width: usize, height: usize, vector: &Vector) -> String {
    let mut svg = String::with_capacity(1024);
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {width} {height}\" width=\"{width}\" height=\"{height}\""
    ));
    if vector.layers.is_empty() {
        // A fully transparent image (requirement SVG-06).
        svg.push_str("/>");
        return svg;
    }
    svg.push('>');
    // The overlap stroke settings are shared by all layers that use it.
    let grouped = vector.layers.iter().any(|l| l.stroke);
    if grouped {
        svg.push_str(&format!(
            "<g stroke-width=\"{OVERLAP_STROKE_WIDTH}\" stroke-linejoin=\"round\">"
        ));
    }
    for layer in &vector.layers {
        write_layer(&mut svg, layer, vector);
    }
    if grouped {
        svg.push_str("</g>");
    }
    svg.push_str("</svg>");
    svg
}

/// Writes one `<path>` element.
fn write_layer(svg: &mut String, layer: &Layer, vector: &Vector) {
    svg.push_str("<path");
    let color = short_color(layer.rgb);
    // Black is the default fill, so it is not written.
    if color != "#000" {
        svg.push_str(" fill=\"");
        svg.push_str(&color);
        svg.push('"');
    }
    if u32::from(layer.alpha_level) < vector.alpha_levels {
        svg.push_str(" fill-opacity=\"");
        svg.push_str(&opacity_text(
            u32::from(layer.alpha_level),
            vector.alpha_levels,
        ));
        svg.push('"');
    }
    if layer.stroke {
        svg.push_str(" stroke=\"");
        svg.push_str(&color);
        svg.push('"');
    }
    svg.push_str(" d=\"");
    svg.push_str(&path_data(&layer.subpaths, vector.precision));
    svg.push_str("\"/>");
}

/// Writes a color in the shortest form: `#abc` if every pair of digits has
/// equal digits, otherwise `#aabbcc`.
pub fn short_color(rgb: [u8; 3]) -> String {
    if rgb.iter().all(|c| c >> 4 == c & 0x0F) {
        format!("#{:x}{:x}{:x}", rgb[0] & 0x0F, rgb[1] & 0x0F, rgb[2] & 0x0F)
    } else {
        format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
    }
}

/// Text of a `fill-opacity` value for a level: two decimal places at most,
/// without a leading zero (for example `.5`). The value is never zero.
fn opacity_text(level: u32, levels: u32) -> String {
    let hundredths = ((level * 200 + levels) / (2 * levels)).clamp(1, 100);
    if hundredths == 100 {
        return "1".to_string();
    }
    format_scaled(hundredths as i32, 2)
}

/// Formats a number that is stored as an integer multiple of `10^-precision`.
///
/// The text has no trailing zeros and no zero before the decimal point:
/// `150` with precision 2 becomes `1.5`, `-50` becomes `-.5`, `0` becomes `0`.
pub fn format_scaled(value: i32, precision: u32) -> String {
    let mut out = String::new();
    push_scaled(&mut out, value, precision);
    out
}

/// Appends the text of [`format_scaled`] to a string.
fn push_scaled(out: &mut String, value: i32, precision: u32) {
    if value == 0 {
        out.push('0');
        return;
    }
    if value < 0 {
        out.push('-');
    }
    let magnitude = value.unsigned_abs();
    let divisor = 10u32.pow(precision);
    let whole = magnitude / divisor;
    let mut fraction = magnitude % divisor;
    if whole != 0 {
        out.push_str(&whole.to_string());
    }
    if fraction != 0 {
        out.push('.');
        // Write the digits of the fraction without the trailing zeros.
        let mut digits = precision;
        while fraction % 10 == 0 {
            fraction /= 10;
            digits -= 1;
        }
        let text = fraction.to_string();
        for _ in text.len()..digits as usize {
            out.push('0');
        }
        out.push_str(&text);
    }
}

/// Describes the end of the text that was written so far. This is needed to
/// know if a separator is required before the next number.
#[derive(Clone, Copy, Default)]
struct Tail {
    /// The last character is a digit or a dot of a number.
    after_number: bool,
    /// The last number already contains a decimal point.
    has_dot: bool,
}

/// State of the path writer.
struct PathWriter {
    out: String,
    precision: u32,
    /// Current point (absolute, grid units).
    current: (i32, i32),
    /// Start of the current subpath (where `z` goes).
    subpath_start: (i32, i32),
    /// The command that follows without a letter, if any.
    implicit: Option<char>,
    /// Second control point of the last curve (for `s`), if the last command
    /// was a curve.
    last_control: Option<(i32, i32)>,
    tail: Tail,
}

impl PathWriter {
    fn new(precision: u32) -> PathWriter {
        PathWriter {
            out: String::new(),
            precision,
            current: (0, 0),
            subpath_start: (0, 0),
            implicit: None,
            last_control: None,
            tail: Tail::default(),
        }
    }

    /// Builds the text of a command (letter and numbers) for the current tail
    /// state, and returns it with the new tail state.
    fn encode(&self, letter: char, numbers: &[i32]) -> (String, Tail) {
        let mut text = String::new();
        let mut tail = self.tail;
        // The letter is left out if the same command is repeated. After `M` the
        // implicit command is `L`, and after `m` it is `l`.
        if self.implicit != Some(letter) {
            text.push(letter);
            tail = Tail::default();
        }
        for &n in numbers {
            let mut digits = String::new();
            push_scaled(&mut digits, n, self.precision);
            let negative = digits.starts_with('-');
            let starts_with_dot = digits.starts_with('.');
            // A separator is needed between two numbers unless the next number
            // starts with a minus sign, or with a dot while the previous number
            // has a dot already (the dot ends the previous number).
            if tail.after_number && !negative && !(starts_with_dot && tail.has_dot) {
                text.push(' ');
            }
            text.push_str(&digits);
            tail = Tail {
                after_number: true,
                has_dot: digits.contains('.'),
            };
        }
        (text, tail)
    }

    /// Writes the shortest of the given alternatives.
    /// Each alternative is `(letter, numbers, new current point, new
    /// control point, implicit letter after the command)`.
    fn emit_best(
        &mut self,
        alternatives: &[(char, Vec<i32>)],
        new_current: (i32, i32),
        control: Option<(i32, i32)>,
    ) {
        let mut best: Option<(String, Tail, char)> = None;
        for (letter, numbers) in alternatives {
            let (text, tail) = self.encode(*letter, numbers);
            let better = match &best {
                None => true,
                Some((b, _, _)) => text.len() < b.len(),
            };
            if better {
                best = Some((text, tail, *letter));
            }
        }
        if let Some((text, tail, letter)) = best {
            self.out.push_str(&text);
            self.tail = tail;
            self.implicit = Some(match letter {
                'M' => 'L',
                'm' => 'l',
                other => other,
            });
        }
        self.current = new_current;
        self.last_control = control;
    }

    /// Starts a new subpath.
    fn move_to(&mut self, p: (i32, i32)) {
        let alternatives = [
            ('M', vec![p.0, p.1]),
            ('m', vec![p.0 - self.current.0, p.1 - self.current.1]),
        ];
        self.emit_best(&alternatives, p, None);
        self.subpath_start = p;
    }

    /// Draws a straight line.
    fn line_to(&mut self, p: (i32, i32)) {
        let (cx, cy) = self.current;
        let alternatives: Vec<(char, Vec<i32>)> = if p.1 == cy {
            vec![('H', vec![p.0]), ('h', vec![p.0 - cx])]
        } else if p.0 == cx {
            vec![('V', vec![p.1]), ('v', vec![p.1 - cy])]
        } else {
            vec![('L', vec![p.0, p.1]), ('l', vec![p.0 - cx, p.1 - cy])]
        };
        self.emit_best(&alternatives, p, None);
    }

    /// Draws a cubic curve.
    fn curve_to(&mut self, c1: (i32, i32), c2: (i32, i32), p: (i32, i32)) {
        let (cx, cy) = self.current;
        let mut alternatives = vec![
            ('C', vec![c1.0, c1.1, c2.0, c2.1, p.0, p.1]),
            (
                'c',
                vec![
                    c1.0 - cx,
                    c1.1 - cy,
                    c2.0 - cx,
                    c2.1 - cy,
                    p.0 - cx,
                    p.1 - cy,
                ],
            ),
        ];
        // The shortcut `s` takes the first control point from the previous
        // curve (mirrored). After a command that is not a curve, the first
        // control point is the current point.
        let implied = match self.last_control {
            Some(prev) => (2 * cx - prev.0, 2 * cy - prev.1),
            None => (cx, cy),
        };
        if c1 == implied {
            alternatives.push(('S', vec![c2.0, c2.1, p.0, p.1]));
            alternatives.push(('s', vec![c2.0 - cx, c2.1 - cy, p.0 - cx, p.1 - cy]));
        }
        self.emit_best(&alternatives, p, Some(c2));
    }

    /// Closes the subpath.
    fn close(&mut self) {
        self.out.push('z');
        self.tail = Tail::default();
        self.implicit = None;
        self.current = self.subpath_start;
        self.last_control = None;
    }
}

/// Creates the value of the `d` attribute for a list of closed outlines.
pub fn path_data(subpaths: &[Subpath], precision: u32) -> String {
    let mut writer = PathWriter::new(precision);
    for sub in subpaths {
        writer.move_to(sub.start);
        // A last line that goes back to the start is replaced by `z`.
        let mut count = sub.segs.len();
        if let Some(Seg::Line(to)) = sub.segs.last() {
            if *to == sub.start && count > 1 {
                count -= 1;
            }
        }
        for seg in &sub.segs[..count] {
            match *seg {
                Seg::Line(to) => writer.line_to(to),
                Seg::Cubic(c1, c2, to) => writer.curve_to(c1, c2, to),
            }
        }
        writer.close();
    }
    writer.out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_written_in_the_shortest_form() {
        assert_eq!(format_scaled(0, 2), "0");
        assert_eq!(format_scaled(100, 2), "1");
        assert_eq!(format_scaled(150, 2), "1.5");
        assert_eq!(format_scaled(-50, 2), "-.5");
        assert_eq!(format_scaled(5, 2), ".05");
        assert_eq!(format_scaled(-105, 2), "-1.05");
        assert_eq!(format_scaled(1234, 2), "12.34");
        assert_eq!(format_scaled(-7, 0), "-7");
        assert_eq!(format_scaled(25, 1), "2.5");
        assert_eq!(format_scaled(1, 3), ".001");
        assert_eq!(format_scaled(1010, 3), "1.01");
    }

    #[test]
    fn colors_use_the_short_form_when_possible() {
        assert_eq!(short_color([0, 0, 0]), "#000");
        assert_eq!(short_color([255, 255, 255]), "#fff");
        assert_eq!(short_color([0xAA, 0xBB, 0xCC]), "#abc");
        assert_eq!(short_color([0xAB, 0xBB, 0xCC]), "#abbbcc");
        assert_eq!(short_color([1, 2, 3]), "#010203");
    }

    #[test]
    fn opacity_text_is_short() {
        assert_eq!(opacity_text(8, 16), ".5");
        assert_eq!(opacity_text(1, 2), ".5");
        assert_eq!(opacity_text(1, 4), ".25");
        assert_eq!(opacity_text(3, 4), ".75");
        assert_eq!(opacity_text(1, 16), ".06");
        assert_eq!(opacity_text(16, 16), "1");
    }

    fn square(x: i32, y: i32, size: i32) -> Subpath {
        Subpath {
            start: (x, y),
            segs: vec![
                Seg::Line((x + size, y)),
                Seg::Line((x + size, y + size)),
                Seg::Line((x, y + size)),
                Seg::Line((x, y)),
            ],
        }
    }

    #[test]
    fn square_is_written_with_h_v_and_z() {
        assert_eq!(path_data(&[square(0, 0, 10)], 0), "M0 0H10V10H0z");
    }

    #[test]
    fn second_subpath_uses_the_shorter_move() {
        // After `z` the current point is (0,0). The next square starts at (20,20).
        assert_eq!(
            path_data(&[square(0, 0, 10), square(20, 20, 5)], 0),
            "M0 0H10V10H0zM20 20h5v5H20z"
        );
    }

    #[test]
    fn separators_are_left_out_where_possible() {
        let sub = Subpath {
            start: (0, 0),
            segs: vec![Seg::Line((10, -20)), Seg::Line((15, 5)), Seg::Line((0, 0))],
        };
        // The letter is not repeated, a minus sign needs no separator, and the
        // absolute and the relative form have the same length here.
        assert_eq!(path_data(&[sub], 0), "M0 0 10-20 15 5z");
    }

    #[test]
    fn dot_numbers_need_no_separator_after_a_dot_number() {
        let sub = Subpath {
            start: (0, 0),
            segs: vec![
                Seg::Line((50, 50)),
                Seg::Line((100, 150)),
                Seg::Line((0, 0)),
            ],
        };
        let d = path_data(&[sub], 2);
        assert_eq!(d, "M0 0 .5.5l.5 1z");
    }

    #[test]
    fn smooth_curve_continuation_uses_s() {
        let sub = Subpath {
            start: (0, 0),
            segs: vec![
                Seg::Cubic((0, 10), (10, 20), (20, 20)),
                // The first control point (30, 20) mirrors (10, 20) around (20, 20).
                Seg::Cubic((30, 20), (40, 10), (40, 0)),
                Seg::Line((0, 0)),
            ],
        };
        let d = path_data(&[sub], 0);
        let lower = d.to_lowercase();
        assert!(lower.contains('s'), "{d}");
        // Only the first curve needs a full curve command.
        assert_eq!(lower.matches('c').count(), 1, "{d}");
    }

    #[test]
    fn first_curve_can_use_s_when_the_control_point_is_the_current_point() {
        let sub = Subpath {
            start: (0, 0),
            segs: vec![Seg::Cubic((0, 0), (10, 10), (20, 0)), Seg::Line((0, 0))],
        };
        let d = path_data(&[sub], 0);
        assert!(
            d.starts_with("M0 0") && d[4..].to_lowercase().starts_with('s'),
            "{d}"
        );
    }

    #[test]
    fn absolute_commands_are_used_when_they_are_shorter() {
        let sub = Subpath {
            start: (1000, 1000),
            segs: vec![
                Seg::Line((1000, 2)),
                Seg::Line((1, 2)),
                Seg::Line((1, 1000)),
                Seg::Line((1000, 1000)),
            ],
        };
        let d = path_data(&[sub], 0);
        // `V2` is shorter than `v-998`, and `H1` is shorter than `h-999`.
        // `v998` is shorter than `V1000`.
        assert_eq!(d, "M1000 1000V2H1v998z");
    }

    #[test]
    fn empty_image_gives_an_empty_svg() {
        let v = Vector {
            layers: Vec::new(),
            alpha_levels: 2,
            precision: 0,
            stats: Default::default(),
        };
        assert_eq!(
            write_svg(3, 4, &v),
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 3 4\" width=\"3\" height=\"4\"/>"
        );
    }

    #[test]
    fn document_has_expected_structure() {
        let layer = |rgb, alpha_level, stroke| Layer {
            rgb,
            alpha_level,
            stroke,
            subpaths: vec![square(0, 0, 4)],
        };
        let v = Vector {
            layers: vec![layer([0, 0, 0], 2, false), layer([0xFF, 0, 0], 1, true)],
            alpha_levels: 2,
            precision: 0,
            stats: Default::default(),
        };
        let svg = write_svg(4, 4, &v);
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 4 4\" width=\"4\" height=\"4\">"));
        assert!(svg.contains("<g stroke-width=\".3\" stroke-linejoin=\"round\">"));
        assert!(svg.contains("<path d=\"M0 0H4V4H0z\"/>"), "{svg}");
        assert!(
            svg.contains("<path fill=\"#f00\" fill-opacity=\".5\" stroke=\"#f00\" d="),
            "{svg}"
        );
        assert!(svg.ends_with("</g></svg>"));
        assert!(!svg.contains('\n') && !svg.contains("<?xml"));
    }
}
