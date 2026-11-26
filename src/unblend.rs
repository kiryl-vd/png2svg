//! Removal of anti-aliasing pixels (requirement ALG-03).
//!
//! When a program draws a shape, the pixels on its edge get a mix of the shape
//! color and the color behind it. These *blend pixels* carry no information
//! about the colors of the picture, only about the exact position of the edge.
//! If they are kept, every edge becomes a thin ring of extra regions, and the
//! SVG gets many small paths.
//!
//! This step finds the blend pixels and replaces each of them with the pixel
//! on its nearer side. A pixel is a blend pixel if two opposite neighbors have
//! clearly different colors and the pixel lies on the straight line between
//! these two colors. Colors are compared as premultiplied sRGB values
//! (red, green, blue, alpha). This is the space where drawing programs blend,
//! so a blend of two colors is exactly on the line between them.
//!
//! The vector outline that is fitted later is smooth, so the renderer creates
//! the anti-aliasing again.

use crate::png::Image;

/// The two neighbor colors must be at least this far apart (squared distance
/// in the premultiplied RGBA space), otherwise the pixel is not an edge.
const MIN_EDGE_DISTANCE_SQ: i64 = 40 * 40;
/// Largest squared distance between the pixel and the line of its neighbors.
const MAX_LINE_DISTANCE_SQ: f64 = 10.0 * 10.0;

/// The neighbor pairs that are checked: west-east, north-south and the two
/// diagonals. Each entry is `(dx1, dy1, dx2, dy2)`.
const PAIRS: [(i32, i32, i32, i32); 4] =
    [(-1, 0, 1, 0), (0, -1, 0, 1), (-1, -1, 1, 1), (1, -1, -1, 1)];

/// Premultiplied RGBA of the pixel at an index of the pixel array.
fn premultiplied(rgba: &[u8], index: usize) -> [i64; 4] {
    let a = i64::from(rgba[index * 4 + 3]);
    let channel = |c: u8| (i64::from(c) * a + 127) / 255;
    [
        channel(rgba[index * 4]),
        channel(rgba[index * 4 + 1]),
        channel(rgba[index * 4 + 2]),
        a,
    ]
}

/// Replaces blend pixels with the neighbor on the nearer side.
///
/// Returns the new image and the number of replaced pixels. All decisions use
/// the original image, so the result does not depend on the scan order.
pub fn remove_antialiasing(image: &Image) -> (Image, usize) {
    let (w, h) = (image.width as i32, image.height as i32);
    let mut result = image.clone();
    if w < 3 || h < 3 {
        return (result, 0);
    }
    let mut replaced = 0;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let index = (y * w + x) as usize;
            let p = premultiplied(&image.rgba, index);
            // Look for the neighbor pair whose line is nearest to the pixel.
            let mut best: Option<(f64, usize)> = None;
            for &(dx1, dy1, dx2, dy2) in &PAIRS {
                let i1 = ((y + dy1) * w + x + dx1) as usize;
                let i2 = ((y + dy2) * w + x + dx2) as usize;
                let q1 = premultiplied(&image.rgba, i1);
                let q2 = premultiplied(&image.rgba, i2);
                let (mut len2, mut dot, mut v2) = (0i64, 0i64, 0i64);
                for k in 0..4 {
                    let wk = q2[k] - q1[k];
                    let vk = p[k] - q1[k];
                    len2 += wk * wk;
                    dot += vk * wk;
                    v2 += vk * vk;
                }
                if len2 < MIN_EDGE_DISTANCE_SQ {
                    continue;
                }
                // The position along the line must be strictly between the two
                // neighbors: 0.08 <= t <= 0.92, with t = dot / len2.
                if 100 * dot < 8 * len2 || 100 * dot > 92 * len2 {
                    continue;
                }
                // Squared distance from the line: |v|^2 - dot^2 / len2.
                let off2 = v2 as f64 - (dot as f64) * (dot as f64) / len2 as f64;
                if off2 > MAX_LINE_DISTANCE_SQ {
                    continue;
                }
                // The nearer neighbor wins: t < 0.5 means the first one.
                let source = if 2 * dot < len2 { i1 } else { i2 };
                if best.map(|b| off2 < b.0).unwrap_or(true) {
                    best = Some((off2, source));
                }
            }
            if let Some((_, source)) = best {
                result.rgba[index * 4..index * 4 + 4]
                    .copy_from_slice(&image.rgba[source * 4..source * 4 + 4]);
                replaced += 1;
            }
        }
    }
    (result, replaced)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: usize, height: usize, pixels: &[[u8; 4]]) -> Image {
        Image {
            width,
            height,
            rgba: pixels.iter().flatten().copied().collect(),
        }
    }

    #[test]
    fn blend_between_two_colors_is_replaced_by_the_nearer_one() {
        // Columns: red, red, blend (30% blue), blue, blue.
        let red = [255, 0, 0, 255];
        let blue = [0, 0, 255, 255];
        let blend = [179, 0, 76, 255]; // 70% red + 30% blue
        let row = [red, red, blend, blue, blue];
        let pixels: Vec<[u8; 4]> = (0..3).flat_map(|_| row).collect();
        let (out, count) = remove_antialiasing(&image(5, 3, &pixels));
        assert_eq!(count, 1);
        assert_eq!(&out.rgba[(5 + 2) * 4..(5 + 2) * 4 + 4], &red);
    }

    #[test]
    fn blend_with_transparency_is_replaced() {
        // Opaque red next to transparent, with a 40% alpha pixel between them.
        let red = [255, 0, 0, 255];
        let none = [0, 0, 0, 0];
        let half = [255, 0, 0, 102];
        let row = [red, red, half, none, none];
        let pixels: Vec<[u8; 4]> = (0..3).flat_map(|_| row).collect();
        let (out, count) = remove_antialiasing(&image(5, 3, &pixels));
        assert_eq!(count, 1);
        // 40% alpha is nearer to transparent.
        assert_eq!(&out.rgba[(5 + 2) * 4..(5 + 2) * 4 + 4], &none);
    }

    #[test]
    fn real_line_with_its_own_color_is_kept() {
        // A green line between red and blue is not on the red-blue line.
        let red = [255, 0, 0, 255];
        let green = [0, 255, 0, 255];
        let blue = [0, 0, 255, 255];
        let row = [red, red, green, blue, blue];
        let pixels: Vec<[u8; 4]> = (0..3).flat_map(|_| row).collect();
        let (out, count) = remove_antialiasing(&image(5, 3, &pixels));
        assert_eq!(count, 0);
        assert_eq!(out, image(5, 3, &pixels));
    }

    #[test]
    fn smooth_gradient_is_not_changed() {
        let row: Vec<[u8; 4]> = (0..16).map(|i| [i * 8, i * 8, i * 8, 255]).collect();
        let pixels: Vec<[u8; 4]> = (0..3).flat_map(|_| row.clone()).collect();
        let (_, count) = remove_antialiasing(&image(16, 3, &pixels));
        assert_eq!(count, 0);
    }

    #[test]
    fn tiny_images_are_returned_unchanged() {
        let img = image(2, 2, &[[1, 2, 3, 4]; 4]);
        assert_eq!(remove_antialiasing(&img), (img, 0));
    }
}
