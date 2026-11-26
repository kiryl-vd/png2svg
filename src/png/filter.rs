//! PNG scanline filters and the Adam7 interlace layout.

use std::fmt;

/// Error for a scanline that uses an unknown filter type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterError {
    /// The invalid filter type byte.
    pub filter_type: u8,
    /// Zero based index of the row in its pass.
    pub row: usize,
}

impl fmt::Display for FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown row filter type {} in row {}",
            self.filter_type, self.row
        )
    }
}

impl std::error::Error for FilterError {}

/// Geometry of one Adam7 pass (or of the whole image when not interlaced).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pass {
    /// Width of the pass in pixels.
    pub width: usize,
    /// Height of the pass in pixels.
    pub height: usize,
    /// X position of the first pixel of the pass in the full image.
    pub x_start: usize,
    /// Y position of the first pixel of the pass in the full image.
    pub y_start: usize,
    /// Horizontal distance between pixels of the pass in the full image.
    pub x_step: usize,
    /// Vertical distance between pixels of the pass in the full image.
    pub y_step: usize,
}

/// Start and step values of the seven Adam7 passes: `(x0, y0, dx, dy)`.
const ADAM7: [(usize, usize, usize, usize); 7] = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
];

/// Returns the passes of an image. A non-interlaced image has one pass that
/// covers the whole image. An interlaced image has up to seven passes. Passes
/// that contain no pixels (possible for small images) are left out.
pub fn passes(width: usize, height: usize, interlaced: bool) -> Vec<Pass> {
    if !interlaced {
        return vec![Pass {
            width,
            height,
            x_start: 0,
            y_start: 0,
            x_step: 1,
            y_step: 1,
        }];
    }
    ADAM7
        .iter()
        .filter_map(|&(x0, y0, dx, dy)| {
            // Number of pixels = ceil((size - start) / step), or zero.
            let w = if width > x0 {
                (width - x0).div_ceil(dx)
            } else {
                0
            };
            let h = if height > y0 {
                (height - y0).div_ceil(dy)
            } else {
                0
            };
            if w == 0 || h == 0 {
                None
            } else {
                Some(Pass {
                    width: w,
                    height: h,
                    x_start: x0,
                    y_start: y0,
                    x_step: dx,
                    y_step: dy,
                })
            }
        })
        .collect()
}

/// Number of bytes in one scanline (without the filter type byte).
pub fn row_bytes(width: usize, bits_per_pixel: usize) -> usize {
    (width * bits_per_pixel).div_ceil(8)
}

/// Removes the row filters of one pass in place.
///
/// `data` holds `height` rows. Each row is one filter type byte followed by
/// `row_len` bytes. `bpp` is the number of bytes per complete pixel (at least 1).
/// The result is written to `out` as `height` rows of `row_len` bytes without
/// the filter bytes.
pub fn unfilter_pass(
    data: &[u8],
    height: usize,
    row_len: usize,
    bpp: usize,
    out: &mut Vec<u8>,
) -> Result<(), FilterError> {
    let stride = row_len + 1;
    let start = out.len();
    out.resize(start + height * row_len, 0);

    for row in 0..height {
        let filter = data[row * stride];
        let src = &data[row * stride + 1..(row + 1) * stride];
        let row_start = start + row * row_len;
        // Split so the previous row can be read while the current is written.
        let (done, rest) = out.split_at_mut(row_start);
        let cur = &mut rest[..row_len];
        let prev: &[u8] = if row == 0 {
            &[]
        } else {
            &done[row_start - row_len..row_start]
        };
        unfilter_row(filter, src, prev, cur, bpp)
            .map_err(|filter_type| FilterError { filter_type, row })?;
    }
    Ok(())
}

/// Reverses the filter of a single row.
///
/// `prev` is the already unfiltered previous row, or an empty slice for the
/// first row (which behaves like a row of zeros). On error, returns the
/// invalid filter type.
fn unfilter_row(filter: u8, src: &[u8], prev: &[u8], cur: &mut [u8], bpp: usize) -> Result<(), u8> {
    let n = cur.len();
    // Helper for the "up" pixel: zero when there is no previous row.
    let up = |i: usize| -> u8 {
        if prev.is_empty() {
            0
        } else {
            prev[i]
        }
    };
    match filter {
        // None: the bytes are stored as they are.
        0 => cur.copy_from_slice(src),
        // Sub: add the byte of the pixel on the left.
        1 => {
            for i in 0..n {
                let left = if i >= bpp { cur[i - bpp] } else { 0 };
                cur[i] = src[i].wrapping_add(left);
            }
        }
        // Up: add the byte of the pixel above.
        2 => {
            for i in 0..n {
                cur[i] = src[i].wrapping_add(up(i));
            }
        }
        // Average: add the floor of the mean of left and up.
        3 => {
            for i in 0..n {
                let left = if i >= bpp { u16::from(cur[i - bpp]) } else { 0 };
                let avg = ((left + u16::from(up(i))) / 2) as u8;
                cur[i] = src[i].wrapping_add(avg);
            }
        }
        // Paeth: add the neighbour that is closest to left + up - upper-left.
        4 => {
            for i in 0..n {
                let (a, c) = if i >= bpp {
                    (
                        cur[i - bpp],
                        if prev.is_empty() { 0 } else { prev[i - bpp] },
                    )
                } else {
                    (0, 0)
                };
                cur[i] = src[i].wrapping_add(paeth(a, up(i), c));
            }
        }
        other => return Err(other),
    }
    Ok(())
}

/// The Paeth predictor from the PNG specification.
/// `a` is the left byte, `b` the byte above, `c` the byte above left.
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (i32::from(a), i32::from(b), i32::from(c));
    let p = ia + ib - ic;
    let pa = (p - ia).abs();
    let pb = (p - ib).abs();
    let pc = (p - ic).abs();
    // Ties are resolved in the order a, b, c.
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_interlaced_has_a_single_pass() {
        let p = passes(5, 3, false);
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].width, p[0].height), (5, 3));
    }

    #[test]
    fn adam7_passes_cover_every_pixel_once() {
        for (w, h) in [(1, 1), (2, 2), (3, 5), (8, 8), (9, 7), (17, 33)] {
            let mut seen = vec![0u8; w * h];
            for p in passes(w, h, true) {
                for y in 0..p.height {
                    for x in 0..p.width {
                        seen[(p.y_start + y * p.y_step) * w + p.x_start + x * p.x_step] += 1;
                    }
                }
            }
            assert!(seen.iter().all(|&c| c == 1), "size {w}x{h}");
        }
    }

    #[test]
    fn one_by_one_image_has_only_the_first_pass() {
        assert_eq!(passes(1, 1, true).len(), 1);
    }

    #[test]
    fn row_bytes_rounds_up() {
        assert_eq!(row_bytes(1, 1), 1);
        assert_eq!(row_bytes(9, 1), 2);
        assert_eq!(row_bytes(3, 24), 9);
    }

    #[test]
    fn paeth_matches_specification_examples() {
        assert_eq!(paeth(0, 0, 0), 0);
        assert_eq!(paeth(10, 20, 10), 20);
        assert_eq!(paeth(20, 10, 10), 20);
        assert_eq!(paeth(10, 10, 20), 10);
    }

    /// Applies a filter to raw rows, the way an encoder does.
    fn filter_rows(rows: &[Vec<u8>], filter: u8, bpp: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            out.push(filter);
            for i in 0..row.len() {
                let a = if i >= bpp { row[i - bpp] } else { 0 };
                let b = if r > 0 { rows[r - 1][i] } else { 0 };
                let c = if r > 0 && i >= bpp {
                    rows[r - 1][i - bpp]
                } else {
                    0
                };
                let predicted = match filter {
                    0 => 0,
                    1 => a,
                    2 => b,
                    3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                    _ => paeth(a, b, c),
                };
                out.push(row[i].wrapping_sub(predicted));
            }
        }
        out
    }

    #[test]
    fn all_filters_round_trip() {
        let rows: Vec<Vec<u8>> = (0..6)
            .map(|r| {
                (0..12)
                    .map(|i| ((r * 37 + i * 91 + 13) % 256) as u8)
                    .collect()
            })
            .collect();
        for filter in 0..=4u8 {
            for bpp in [1usize, 3, 4] {
                let encoded = filter_rows(&rows, filter, bpp);
                let mut decoded = Vec::new();
                unfilter_pass(&encoded, rows.len(), 12, bpp, &mut decoded).unwrap();
                let expected: Vec<u8> = rows.iter().flatten().copied().collect();
                assert_eq!(decoded, expected, "filter {filter} bpp {bpp}");
            }
        }
    }

    #[test]
    fn unknown_filter_is_an_error() {
        let data = [9u8, 1, 2, 3];
        let mut out = Vec::new();
        let err = unfilter_pass(&data, 1, 3, 1, &mut out).unwrap_err();
        assert_eq!(err.filter_type, 9);
    }
}
