//! Conversion of raw PNG samples to the internal RGBA 8-bit format.

/// PNG color types (IHDR field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorType {
    /// Type 0: one gray sample per pixel.
    Grayscale,
    /// Type 2: red, green and blue samples.
    Rgb,
    /// Type 3: one palette index per pixel.
    Indexed,
    /// Type 4: gray and alpha samples.
    GrayscaleAlpha,
    /// Type 6: red, green, blue and alpha samples.
    Rgba,
}

impl ColorType {
    /// Parses the numeric color type of the IHDR chunk.
    pub fn from_code(code: u8) -> Option<ColorType> {
        match code {
            0 => Some(ColorType::Grayscale),
            2 => Some(ColorType::Rgb),
            3 => Some(ColorType::Indexed),
            4 => Some(ColorType::GrayscaleAlpha),
            6 => Some(ColorType::Rgba),
            _ => None,
        }
    }

    /// Number of samples per pixel.
    pub fn channels(self) -> usize {
        match self {
            ColorType::Grayscale | ColorType::Indexed => 1,
            ColorType::GrayscaleAlpha => 2,
            ColorType::Rgb => 3,
            ColorType::Rgba => 4,
        }
    }

    /// Checks if the bit depth is allowed for this color type.
    pub fn allows_depth(self, depth: u8) -> bool {
        match self {
            ColorType::Grayscale => matches!(depth, 1 | 2 | 4 | 8 | 16),
            ColorType::Indexed => matches!(depth, 1 | 2 | 4 | 8),
            _ => matches!(depth, 8 | 16),
        }
    }
}

/// Everything that is needed to convert raw scanlines to RGBA pixels.
#[derive(Debug, Clone)]
pub struct Converter {
    /// Color type of the image.
    pub color_type: ColorType,
    /// Bits per sample: 1, 2, 4, 8 or 16.
    pub depth: u8,
    /// Palette entries as `[r, g, b]` (only for indexed images).
    pub palette: Vec<[u8; 3]>,
    /// Alpha values of the palette entries (from `tRNS`). Missing = opaque.
    pub palette_alpha: Vec<u8>,
    /// Transparent gray value (raw sample scale) for grayscale images.
    pub transparent_gray: Option<u16>,
    /// Transparent RGB value (raw sample scale) for RGB images.
    pub transparent_rgb: Option<[u16; 3]>,
}

/// Error for an index that points outside of the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteIndexError(pub usize);

/// Scales a 16-bit sample to 8 bits with rounding.
fn scale16(v: u16) -> u8 {
    ((u32::from(v) * 255 + 32_767) / 65_535) as u8
}

impl Converter {
    /// Converts one scanline (without the filter byte) of `width` pixels to
    /// RGBA and writes it to `out` (`width * 4` bytes).
    pub fn convert_row(
        &self,
        row: &[u8],
        width: usize,
        out: &mut [u8],
    ) -> Result<(), PaletteIndexError> {
        let depth = usize::from(self.depth);
        match self.color_type {
            ColorType::Grayscale => {
                for x in 0..width {
                    let raw = sample(row, x, depth);
                    let gray = scale_small(raw, self.depth);
                    let alpha = if self.transparent_gray == Some(raw) {
                        0
                    } else {
                        255
                    };
                    out[x * 4..x * 4 + 4].copy_from_slice(&[gray, gray, gray, alpha]);
                }
            }
            ColorType::Indexed => {
                for x in 0..width {
                    let index = usize::from(sample(row, x, depth));
                    let rgb = self.palette.get(index).ok_or(PaletteIndexError(index))?;
                    let alpha = self.palette_alpha.get(index).copied().unwrap_or(255);
                    out[x * 4..x * 4 + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], alpha]);
                }
            }
            ColorType::Rgb => {
                for x in 0..width {
                    let raw = [
                        sample(row, x * 3, depth),
                        sample(row, x * 3 + 1, depth),
                        sample(row, x * 3 + 2, depth),
                    ];
                    let alpha = if self.transparent_rgb == Some(raw) {
                        0
                    } else {
                        255
                    };
                    out[x * 4..x * 4 + 4].copy_from_slice(&[
                        scale_small(raw[0], self.depth),
                        scale_small(raw[1], self.depth),
                        scale_small(raw[2], self.depth),
                        alpha,
                    ]);
                }
            }
            ColorType::GrayscaleAlpha => {
                for x in 0..width {
                    let gray = scale_small(sample(row, x * 2, depth), self.depth);
                    let alpha = scale_small(sample(row, x * 2 + 1, depth), self.depth);
                    out[x * 4..x * 4 + 4].copy_from_slice(&[gray, gray, gray, alpha]);
                }
            }
            ColorType::Rgba => {
                for x in 0..width {
                    let px = [
                        sample(row, x * 4, depth),
                        sample(row, x * 4 + 1, depth),
                        sample(row, x * 4 + 2, depth),
                        sample(row, x * 4 + 3, depth),
                    ];
                    out[x * 4..x * 4 + 4].copy_from_slice(&[
                        scale_small(px[0], self.depth),
                        scale_small(px[1], self.depth),
                        scale_small(px[2], self.depth),
                        scale_small(px[3], self.depth),
                    ]);
                }
            }
        }
        Ok(())
    }
}

/// Reads the sample with the given index from a scanline.
///
/// Samples narrower than 8 bits are packed starting at the most significant
/// bit. 16-bit samples are big endian. The caller makes sure that the row is
/// long enough; a missing byte is read as zero so the function cannot panic.
fn sample(row: &[u8], index: usize, depth: usize) -> u16 {
    let byte = |i: usize| -> u16 { u16::from(row.get(i).copied().unwrap_or(0)) };
    match depth {
        8 => byte(index),
        16 => (byte(index * 2) << 8) | byte(index * 2 + 1),
        _ => {
            // 1, 2 or 4 bits: several samples share one byte.
            let per_byte = 8 / depth;
            let shift = 8 - depth * (index % per_byte + 1);
            (byte(index / per_byte) >> shift) & ((1u16 << depth) - 1)
        }
    }
}

/// Scales a sample of any depth to the full 8-bit range.
fn scale_small(value: u16, depth: u8) -> u8 {
    match depth {
        1 => (value * 255) as u8,
        2 => (value * 85) as u8,
        4 => (value * 17) as u8,
        8 => value as u8,
        _ => scale16(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn converter(color_type: ColorType, depth: u8) -> Converter {
        Converter {
            color_type,
            depth,
            palette: Vec::new(),
            palette_alpha: Vec::new(),
            transparent_gray: None,
            transparent_rgb: None,
        }
    }

    #[test]
    fn unpacks_one_bit_gray() {
        let c = converter(ColorType::Grayscale, 1);
        let mut out = [0u8; 8 * 4];
        c.convert_row(&[0b1010_0000], 8, &mut out).unwrap();
        let grays: Vec<u8> = out.chunks(4).map(|p| p[0]).collect();
        assert_eq!(grays, [255, 0, 255, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn scales_two_and_four_bit_gray() {
        let c2 = converter(ColorType::Grayscale, 2);
        let mut out = [0u8; 4 * 4];
        c2.convert_row(&[0b00_01_10_11], 4, &mut out).unwrap();
        assert_eq!([out[0], out[4], out[8], out[12]], [0, 85, 170, 255]);

        let c4 = converter(ColorType::Grayscale, 4);
        let mut out = [0u8; 2 * 4];
        c4.convert_row(&[0x0F], 2, &mut out).unwrap();
        assert_eq!([out[0], out[4]], [0, 255]);
    }

    #[test]
    fn sixteen_bit_samples_are_rounded() {
        assert_eq!(scale16(0), 0);
        assert_eq!(scale16(65_535), 255);
        assert_eq!(scale16(0x8000), 128);
        let c = converter(ColorType::Rgb, 16);
        let mut out = [0u8; 4];
        c.convert_row(&[0xFF, 0xFF, 0x00, 0x00, 0x80, 0x00], 1, &mut out)
            .unwrap();
        assert_eq!(out, [255, 0, 128, 255]);
    }

    #[test]
    fn palette_with_transparency() {
        let mut c = converter(ColorType::Indexed, 2);
        c.palette = vec![[1, 2, 3], [4, 5, 6]];
        c.palette_alpha = vec![0];
        let mut out = [0u8; 8];
        c.convert_row(&[0b0001_0000], 2, &mut out).unwrap();
        assert_eq!(out, [1, 2, 3, 0, 4, 5, 6, 255]);
    }

    #[test]
    fn palette_index_out_of_range_is_an_error() {
        let mut c = converter(ColorType::Indexed, 8);
        c.palette = vec![[0, 0, 0]];
        let mut out = [0u8; 4];
        assert_eq!(c.convert_row(&[5], 1, &mut out), Err(PaletteIndexError(5)));
    }

    #[test]
    fn color_key_transparency() {
        let mut c = converter(ColorType::Rgb, 8);
        c.transparent_rgb = Some([10, 20, 30]);
        let mut out = [0u8; 8];
        c.convert_row(&[10, 20, 30, 10, 20, 31], 2, &mut out)
            .unwrap();
        assert_eq!(out, [10, 20, 30, 0, 10, 20, 31, 255]);

        let mut g = converter(ColorType::Grayscale, 8);
        g.transparent_gray = Some(7);
        let mut out = [0u8; 8];
        g.convert_row(&[7, 8], 2, &mut out).unwrap();
        assert_eq!([out[3], out[7]], [0, 255]);
    }

    #[test]
    fn color_type_rules() {
        assert!(ColorType::Indexed.allows_depth(4));
        assert!(!ColorType::Indexed.allows_depth(16));
        assert!(!ColorType::Rgb.allows_depth(4));
        assert!(ColorType::Grayscale.allows_depth(16));
        assert_eq!(ColorType::from_code(5), None);
        assert_eq!(ColorType::Rgba.channels(), 4);
    }
}
