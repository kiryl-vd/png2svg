//! A PNG decoder written with the Rust standard library only.
//!
//! Supported: all color types and bit depths, `tRNS` transparency, Adam7
//! interlace, all five row filters, multiple `IDAT` chunks. The decoder checks
//! the CRC-32 of every chunk and the Adler-32 checksum of the zlib stream.
//! Ancillary chunks (gamma, ICC profile, text, APNG animation data and so on)
//! are ignored, so the first frame of an APNG file is decoded as a normal image.
//!
//! The result is always an [`Image`] with 8-bit RGBA pixels.

pub mod checksum;
pub mod convert;
pub mod filter;
pub mod inflate;

use std::fmt;

use convert::{ColorType, Converter};

/// The 8 bytes that every PNG file starts with.
pub const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Largest accepted image width or height in pixels (requirement IN-11).
pub const MAX_DIMENSION: u32 = 16_384;
/// Largest accepted number of pixels (requirement IN-11).
pub const MAX_PIXELS: u64 = 100_000_000;

/// A decoded image: `width * height` pixels, 4 bytes (R, G, B, A) each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Pixel data, row by row, 4 bytes per pixel. Alpha is not premultiplied.
    pub rgba: Vec<u8>,
}

/// Reasons why a PNG file cannot be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PngError {
    /// The file is not a PNG file (the signature is wrong).
    NotPng,
    /// The file is damaged or breaks the PNG rules. The text tells what is wrong.
    Corrupt(String),
    /// The file is valid but uses a feature that this decoder does not support.
    Unsupported(String),
    /// The image is larger than the limits of the program.
    TooLarge(String),
}

impl fmt::Display for PngError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PngError::NotPng => write!(f, "not a PNG file (the signature is wrong)"),
            PngError::Corrupt(why) => write!(f, "corrupt PNG file: {why}"),
            PngError::Unsupported(why) => write!(f, "unsupported PNG file: {why}"),
            PngError::TooLarge(why) => write!(f, "image is too large: {why}"),
        }
    }
}

impl std::error::Error for PngError {}

/// Shortcut to build a [`PngError::Corrupt`] value.
fn corrupt<T>(why: impl Into<String>) -> Result<T, PngError> {
    Err(PngError::Corrupt(why.into()))
}

/// Data of the IHDR chunk.
#[derive(Debug, Clone, Copy)]
struct Header {
    width: u32,
    height: u32,
    depth: u8,
    color_type: ColorType,
    interlaced: bool,
}

impl Header {
    /// Number of bits of one pixel.
    fn bits_per_pixel(&self) -> usize {
        self.color_type.channels() * usize::from(self.depth)
    }
}

/// Decodes a PNG file that is already loaded in memory.
///
/// The format is detected by the file signature, not by the file name.
/// The function never panics. Every problem is returned as a [`PngError`].
pub fn decode(bytes: &[u8]) -> Result<Image, PngError> {
    if bytes.len() < SIGNATURE.len() || bytes[..8] != SIGNATURE {
        return Err(PngError::NotPng);
    }

    let mut pos = SIGNATURE.len();
    let mut header: Option<Header> = None;
    let mut palette: Option<Vec<[u8; 3]>> = None;
    let mut trns: Option<Vec<u8>> = None;
    let mut idat: Vec<u8> = Vec::new();
    // State of the IDAT sequence: 0 = not seen, 1 = inside, 2 = finished.
    let mut idat_state = 0u8;
    let mut seen_end = false;
    let mut first_chunk = true;

    while pos < bytes.len() {
        // A chunk is: length (4), type (4), data (length), CRC (4).
        if bytes.len() - pos < 12 {
            return corrupt("file ends inside a chunk header");
        }
        let length =
            u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]);
        if length > 0x7FFF_FFFF {
            return corrupt("chunk length is too large");
        }
        let length = length as usize;
        let kind: [u8; 4] = [
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ];
        if !kind.iter().all(u8::is_ascii_alphabetic) {
            return corrupt("chunk type has invalid characters");
        }
        let data_start = pos + 8;
        let data_end = match data_start.checked_add(length) {
            Some(end) if end + 4 <= bytes.len() => end,
            _ => return corrupt("file ends inside a chunk"),
        };
        let data = &bytes[data_start..data_end];
        let stored_crc = u32::from_be_bytes([
            bytes[data_end],
            bytes[data_end + 1],
            bytes[data_end + 2],
            bytes[data_end + 3],
        ]);
        // The CRC covers the chunk type and the chunk data.
        let crc = checksum::crc32_update(checksum::crc32(&kind), data);
        if crc != stored_crc {
            return corrupt(format!(
                "checksum of chunk {} does not match",
                String::from_utf8_lossy(&kind)
            ));
        }
        pos = data_end + 4;

        // The first chunk must be IHDR.
        if first_chunk && &kind != b"IHDR" {
            return corrupt("the first chunk is not IHDR");
        }
        first_chunk = false;

        // IDAT chunks must be consecutive.
        if &kind != b"IDAT" && idat_state == 1 {
            idat_state = 2;
        }

        match &kind {
            b"IHDR" => {
                if header.is_some() {
                    return corrupt("duplicate IHDR chunk");
                }
                header = Some(parse_header(data)?);
            }
            b"PLTE" => {
                let h = header.ok_or_else(|| PngError::Corrupt("PLTE before IHDR".into()))?;
                if palette.is_some() {
                    return corrupt("duplicate PLTE chunk");
                }
                if idat_state != 0 {
                    return corrupt("PLTE after IDAT");
                }
                if matches!(
                    h.color_type,
                    ColorType::Grayscale | ColorType::GrayscaleAlpha
                ) {
                    return corrupt("PLTE chunk in a grayscale image");
                }
                if data.is_empty() || data.len() % 3 != 0 || data.len() / 3 > 256 {
                    return corrupt("PLTE chunk has a wrong length");
                }
                if h.color_type == ColorType::Indexed && data.len() / 3 > 1usize << h.depth {
                    return corrupt("palette has more entries than the bit depth allows");
                }
                palette = Some(data.chunks(3).map(|c| [c[0], c[1], c[2]]).collect());
            }
            b"tRNS" => {
                if header.is_none() {
                    return corrupt("tRNS before IHDR");
                }
                if idat_state != 0 {
                    return corrupt("tRNS after IDAT");
                }
                trns = Some(data.to_vec());
            }
            b"IDAT" => {
                if header.is_none() {
                    return corrupt("IDAT before IHDR");
                }
                if idat_state == 2 {
                    return corrupt("IDAT chunks are not consecutive");
                }
                idat_state = 1;
                idat.extend_from_slice(data);
            }
            b"IEND" => {
                if !data.is_empty() {
                    return corrupt("IEND chunk is not empty");
                }
                seen_end = true;
                break;
            }
            _ => {
                // Bit 5 of the first letter is set for ancillary chunks. Unknown
                // ancillary chunks (including APNG data) are skipped. An unknown
                // critical chunk means we cannot show the image correctly.
                let critical = kind[0] & 0x20 == 0;
                if critical {
                    return Err(PngError::Unsupported(format!(
                        "unknown critical chunk {}",
                        String::from_utf8_lossy(&kind)
                    )));
                }
            }
        }
    }

    let header = match header {
        Some(h) => h,
        None => return corrupt("IHDR chunk is missing"),
    };
    if !seen_end {
        return corrupt("the IEND chunk is missing (the file is cut)");
    }
    if idat_state == 0 {
        return corrupt("the image has no IDAT chunk");
    }
    build_image(&header, palette, trns, &idat)
}

/// Parses and checks the 13 bytes of the IHDR chunk.
fn parse_header(data: &[u8]) -> Result<Header, PngError> {
    if data.len() != 13 {
        return corrupt("IHDR chunk has a wrong length");
    }
    let width = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    let height = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let depth = data[8];
    let color_type = match ColorType::from_code(data[9]) {
        Some(c) => c,
        None => return corrupt(format!("unknown color type {}", data[9])),
    };
    if !color_type.allows_depth(depth) {
        return corrupt(format!(
            "bit depth {depth} is not allowed for color type {}",
            data[9]
        ));
    }
    if data[10] != 0 {
        return corrupt("unknown compression method");
    }
    if data[11] != 0 {
        return corrupt("unknown filter method");
    }
    let interlaced = match data[12] {
        0 => false,
        1 => true,
        _ => return corrupt("unknown interlace method"),
    };
    if width == 0 || height == 0 {
        return corrupt("image size is zero");
    }
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(PngError::TooLarge(format!(
            "{width}x{height} pixels, the limit is {MAX_DIMENSION} pixels per side"
        )));
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(PngError::TooLarge(format!(
            "{width}x{height} pixels, the limit is {MAX_PIXELS} pixels"
        )));
    }
    Ok(Header {
        width,
        height,
        depth,
        color_type,
        interlaced,
    })
}

/// Decompresses the image data, removes the filters and converts the pixels.
fn build_image(
    header: &Header,
    palette: Option<Vec<[u8; 3]>>,
    trns: Option<Vec<u8>>,
    idat: &[u8],
) -> Result<Image, PngError> {
    let width = header.width as usize;
    let height = header.height as usize;
    let bpp_bits = header.bits_per_pixel();
    let filter_bpp = (bpp_bits / 8).max(1);
    let passes = filter::passes(width, height, header.interlaced);

    // Size of the filtered data that the image header promises.
    let expected: usize = passes
        .iter()
        .map(|p| p.height * (1 + filter::row_bytes(p.width, bpp_bits)))
        .sum();
    // DEFLATE cannot make data more than about 1032 times smaller. A bigger
    // promise means the header is wrong, and we do not try to allocate it.
    if expected / 1032 > idat.len() + 16 {
        return corrupt("image data is too short for the image size");
    }
    let raw =
        inflate::zlib_decompress(idat, expected).map_err(|e| PngError::Corrupt(e.to_string()))?;
    if raw.len() != expected {
        return corrupt("image data has a wrong size");
    }

    let converter = make_converter(header, palette, trns)?;

    let mut rgba = vec![0u8; width * height * 4];
    let mut offset = 0usize;
    let mut unfiltered: Vec<u8> = Vec::new();
    let mut row_rgba: Vec<u8> = Vec::new();
    for pass in &passes {
        let row_len = filter::row_bytes(pass.width, bpp_bits);
        let pass_size = pass.height * (1 + row_len);
        unfiltered.clear();
        filter::unfilter_pass(
            &raw[offset..offset + pass_size],
            pass.height,
            row_len,
            filter_bpp,
            &mut unfiltered,
        )
        .map_err(|e| PngError::Corrupt(e.to_string()))?;
        offset += pass_size;

        row_rgba.clear();
        row_rgba.resize(pass.width * 4, 0);
        for y in 0..pass.height {
            let row = &unfiltered[y * row_len..(y + 1) * row_len];
            converter
                .convert_row(row, pass.width, &mut row_rgba)
                .map_err(|e| PngError::Corrupt(format!("palette index {} is out of range", e.0)))?;
            // Copy the pass pixels to their final place in the image.
            let target_y = pass.y_start + y * pass.y_step;
            for x in 0..pass.width {
                let target_x = pass.x_start + x * pass.x_step;
                let dst = (target_y * width + target_x) * 4;
                rgba[dst..dst + 4].copy_from_slice(&row_rgba[x * 4..x * 4 + 4]);
            }
        }
    }
    Ok(Image {
        width,
        height,
        rgba,
    })
}

/// Collects palette and transparency data in a [`Converter`].
fn make_converter(
    header: &Header,
    palette: Option<Vec<[u8; 3]>>,
    trns: Option<Vec<u8>>,
) -> Result<Converter, PngError> {
    let mut converter = Converter {
        color_type: header.color_type,
        depth: header.depth,
        palette: Vec::new(),
        palette_alpha: Vec::new(),
        transparent_gray: None,
        transparent_rgb: None,
    };
    let be16 = |b: &[u8]| u16::from_be_bytes([b[0], b[1]]);
    match header.color_type {
        ColorType::Indexed => {
            converter.palette = match palette {
                Some(p) => p,
                None => return corrupt("indexed image without PLTE chunk"),
            };
            if let Some(alpha) = trns {
                if alpha.len() > converter.palette.len() {
                    return corrupt("tRNS chunk has more entries than the palette");
                }
                converter.palette_alpha = alpha;
            }
        }
        ColorType::Grayscale => {
            if let Some(t) = trns {
                if t.len() != 2 {
                    return corrupt("tRNS chunk has a wrong length");
                }
                // Only the low bits that fit the bit depth are significant.
                let mask = if header.depth >= 16 {
                    0xFFFF
                } else {
                    (1u16 << header.depth) - 1
                };
                converter.transparent_gray = Some(be16(&t) & mask);
            }
        }
        ColorType::Rgb => {
            if let Some(t) = trns {
                if t.len() != 6 {
                    return corrupt("tRNS chunk has a wrong length");
                }
                converter.transparent_rgb = Some([be16(&t[0..2]), be16(&t[2..4]), be16(&t[4..6])]);
            }
        }
        // The specification forbids tRNS here, but the alpha channel already
        // exists, so the chunk is simply ignored.
        ColorType::GrayscaleAlpha | ColorType::Rgba => {}
    }
    Ok(converter)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds one PNG chunk with a correct CRC.
    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = checksum::crc32_update(checksum::crc32(kind), data);
        out.extend_from_slice(&crc.to_be_bytes());
        out
    }

    /// Wraps data in a zlib stream with stored blocks.
    fn zlib_stored(payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        out.push(1);
        let len = payload.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(payload);
        out.extend_from_slice(&checksum::adler32(payload).to_be_bytes());
        out
    }

    /// Builds a small PNG file from unfiltered scanline data (filter byte 0).
    fn png(width: u32, height: u32, depth: u8, color: u8, interlace: u8, raw: &[u8]) -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[depth, color, 0, 0, interlace]);
        let mut file = SIGNATURE.to_vec();
        file.extend(chunk(b"IHDR", &ihdr));
        file.extend(chunk(b"IDAT", &zlib_stored(raw)));
        file.extend(chunk(b"IEND", &[]));
        file
    }

    #[test]
    fn decodes_one_by_one_rgba() {
        let file = png(1, 1, 8, 6, 0, &[0, 10, 20, 30, 40]);
        let image = decode(&file).unwrap();
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.rgba, [10, 20, 30, 40]);
    }

    #[test]
    fn decodes_interlaced_gray() {
        // 3x3 image, 8-bit gray, Adam7. Passes (w x h): 1x1, 0, 1x1... computed
        // below from the pass geometry, pixel value = y * 3 + x.
        let (w, h) = (3usize, 3usize);
        let mut raw = Vec::new();
        for p in filter::passes(w, h, true) {
            for y in 0..p.height {
                raw.push(0);
                for x in 0..p.width {
                    let (px, py) = (p.x_start + x * p.x_step, p.y_start + y * p.y_step);
                    raw.push((py * w + px) as u8);
                }
            }
        }
        let image = decode(&png(3, 3, 8, 0, 1, &raw)).unwrap();
        let grays: Vec<u8> = image.rgba.chunks(4).map(|p| p[0]).collect();
        assert_eq!(grays, [0, 1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn rejects_wrong_signature() {
        assert_eq!(decode(b"GIF89a....."), Err(PngError::NotPng));
        assert_eq!(decode(b""), Err(PngError::NotPng));
    }

    #[test]
    fn rejects_bad_crc() {
        let mut file = png(1, 1, 8, 0, 0, &[0, 7]);
        // Damage the last byte of the IHDR CRC (signature 8 + length 4 + type 4 + data 13).
        file[8 + 4 + 4 + 13 + 3] ^= 1;
        assert!(matches!(decode(&file), Err(PngError::Corrupt(_))));
    }

    #[test]
    fn rejects_huge_dimensions_before_allocation() {
        let file = png(16_385, 1, 8, 0, 0, &[0, 0]);
        assert!(matches!(decode(&file), Err(PngError::TooLarge(_))));
        let file = png(16_384, 16_384, 8, 0, 0, &[0, 0]);
        assert!(matches!(decode(&file), Err(PngError::TooLarge(_))));
    }

    #[test]
    fn rejects_missing_idat_and_iend() {
        let mut file = SIGNATURE.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
        file.extend(chunk(b"IHDR", &ihdr));
        file.extend(chunk(b"IEND", &[]));
        assert!(matches!(decode(&file), Err(PngError::Corrupt(_))));

        let cut = png(1, 1, 8, 0, 0, &[0, 7]);
        assert!(decode(&cut[..cut.len() - 12]).is_err());
    }

    #[test]
    fn rejects_wrong_data_size() {
        // A 2x2 gray image needs 6 bytes, not 4.
        let file = png(2, 2, 8, 0, 0, &[0, 1, 2, 0]);
        assert!(matches!(decode(&file), Err(PngError::Corrupt(_))));
    }

    #[test]
    fn rejects_invalid_color_type_and_depth() {
        assert!(matches!(
            decode(&png(1, 1, 8, 1, 0, &[0, 0])),
            Err(PngError::Corrupt(_))
        ));
        assert!(matches!(
            decode(&png(1, 1, 3, 0, 0, &[0, 0])),
            Err(PngError::Corrupt(_))
        ));
        assert!(matches!(
            decode(&png(1, 1, 4, 2, 0, &[0, 0])),
            Err(PngError::Corrupt(_))
        ));
    }

    #[test]
    fn ignores_unknown_ancillary_chunks_but_not_critical_ones() {
        let base = png(1, 1, 8, 0, 0, &[0, 9]);
        // Insert a chunk right after IHDR (offset 8 + 25).
        let insert = |kind: &[u8; 4]| {
            let mut f = base[..33].to_vec();
            f.extend(chunk(kind, &[1, 2, 3]));
            f.extend_from_slice(&base[33..]);
            f
        };
        assert!(decode(&insert(b"tEXt")).is_ok());
        assert!(decode(&insert(b"acTL")).is_ok());
        assert!(matches!(
            decode(&insert(b"ABCD")),
            Err(PngError::Unsupported(_))
        ));
    }

    #[test]
    fn never_panics_on_damaged_files() {
        let base = png(2, 2, 8, 2, 0, &[0; 14]);
        for i in 0..base.len() {
            for bit in 0..8 {
                let mut f = base.clone();
                f[i] ^= 1 << bit;
                let _ = decode(&f);
            }
            let _ = decode(&base[..i]);
        }
    }
}
