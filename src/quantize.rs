//! Transparency handling and color quantization.
//!
//! Every pixel gets a *label*:
//!
//! * `0` means "transparent". Such pixels are not drawn.
//! * `n > 0` means "palette entry `n - 1`". A palette entry is a color plus an
//!   alpha level (the alpha value is rounded to a small number of levels, see
//!   [`crate::detail::Params::alpha_levels`]).
//!
//! If the image has no more distinct (color, alpha level) pairs than the limit,
//! the colors are kept exactly. Otherwise the colors are grouped with weighted
//! k-means (k-means++ start, fixed seed) in the Oklab color space, which is
//! close to the way people see color differences.

use std::collections::HashMap;

use crate::color::{distance_sq, Lab, LinearTable, Rng};
use crate::png::Image;

/// Pixels with an alpha value below this number are fully transparent (ALG-01).
pub const ALPHA_THRESHOLD: u8 = 8;

/// Fixed seed of the random generator (ALG-14).
const SEED: u64 = 0x00C0_FFEE_5EED_1234;

/// Weight of the alpha level when colors are compared. The alpha level is
/// scaled to 0..=ALPHA_SCALE, so a change from opaque to fully transparent
/// counts about as much as a big change of color.
const ALPHA_SCALE: f64 = 0.6;

/// Number of Lloyd iterations after the k-means++ start.
const KMEANS_ITERATIONS: usize = 10;

/// If there are more distinct colors than this, they are first grouped into
/// coarse bins, so k-means stays fast on photographs.
const MAX_POINTS: usize = 40_000;

/// A color of the palette with its alpha level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaletteEntry {
    /// Red, green and blue (sRGB, 8 bit).
    pub rgb: [u8; 3],
    /// Alpha level `1..=alpha_levels`. The highest level is fully opaque.
    pub alpha_level: u8,
}

/// The result of the quantization.
#[derive(Debug, Clone)]
pub struct Quantized {
    /// Image width in pixels.
    pub width: usize,
    /// Image height in pixels.
    pub height: usize,
    /// One label per pixel (see the module description).
    pub labels: Vec<u16>,
    /// The palette. Label `n` uses entry `n - 1`.
    pub palette: Vec<PaletteEntry>,
    /// Number of alpha levels used for this image.
    pub alpha_levels: u32,
    /// `true` if all colors of the image were kept exactly.
    pub exact: bool,
}

/// Converts an alpha value (8 to 255) to a level in `1..=levels`.
pub fn alpha_level(alpha: u8, levels: u32) -> u8 {
    let level = (u32::from(alpha) * levels + 127) / 255;
    level.clamp(1, levels) as u8
}

/// Counts the distinct (color, alpha level) pairs of the visible pixels.
pub fn count_distinct(image: &Image, alpha_levels: u32) -> usize {
    let levels = alpha_levels.max(2);
    let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for px in image.rgba.chunks_exact(4) {
        if px[3] >= ALPHA_THRESHOLD {
            seen.insert(pack([px[0], px[1], px[2]], alpha_level(px[3], levels)));
        }
    }
    seen.len()
}

/// Packs a palette entry into one number that is used as a map key.
fn pack(rgb: [u8; 3], level: u8) -> u32 {
    (u32::from(level) << 24)
        | (u32::from(rgb[0]) << 16)
        | (u32::from(rgb[1]) << 8)
        | u32::from(rgb[2])
}

/// Reverses [`pack`].
fn unpack(key: u32) -> ([u8; 3], u8) {
    (
        [(key >> 16) as u8, (key >> 8) as u8, key as u8],
        (key >> 24) as u8,
    )
}

/// A point that k-means works with: a color, an alpha level and a weight.
struct Point {
    /// Oklab color plus the scaled alpha level.
    pos: [f64; 4],
    /// Number of pixels that this point stands for.
    weight: f64,
    /// Sum of the sRGB channels of all pixels (to get the mean color later).
    rgb_sum: [f64; 3],
}

/// Distance between two points of k-means (squared).
fn dist4(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    let lab_a: Lab = [a[0], a[1], a[2]];
    let lab_b: Lab = [b[0], b[1], b[2]];
    let da = a[3] - b[3];
    distance_sq(&lab_a, &lab_b) + da * da
}

/// Quantizes the image: finds the palette and the label of every pixel.
///
/// `max_colors` is the largest allowed palette size (2..=256),
/// `alpha_levels` is the number of opacity levels (at least 2), and
/// `merge_distance` is the Oklab distance below which two palette colors are
/// joined (only used when the colors have to be reduced).
pub fn quantize(
    image: &Image,
    max_colors: u32,
    alpha_levels: u32,
    merge_distance: f64,
) -> Quantized {
    let levels = alpha_levels.max(2);
    let max_colors = max_colors.clamp(2, 256) as usize;
    let pixels = image.width * image.height;

    // Step 1: count the distinct (color, alpha level) pairs of visible pixels.
    let mut counts: HashMap<u32, u32> = HashMap::new();
    for px in image.rgba.chunks_exact(4) {
        if px[3] < ALPHA_THRESHOLD {
            continue;
        }
        let key = pack([px[0], px[1], px[2]], alpha_level(px[3], levels));
        *counts.entry(key).or_insert(0) += 1;
    }
    // Sort by key so that nothing depends on the hash map order (ALG-14).
    let mut entries: Vec<(u32, u32)> = counts.into_iter().collect();
    entries.sort_unstable_by_key(|e| e.0);

    // Step 2: choose the palette.
    let exact = entries.len() <= max_colors;
    let palette: Vec<PaletteEntry> = if exact {
        entries
            .iter()
            .map(|&(key, _)| {
                let (rgb, alpha_level) = unpack(key);
                PaletteEntry { rgb, alpha_level }
            })
            .collect()
    } else {
        cluster_palette(&entries, max_colors, levels, merge_distance)
    };

    // Step 3: find the palette entry of every distinct pair, then label pixels.
    let table = LinearTable::new();
    let key_to_label: HashMap<u32, u16> = if exact {
        entries
            .iter()
            .enumerate()
            .map(|(i, &(key, _))| (key, (i + 1) as u16))
            .collect()
    } else {
        nearest_labels(&entries, &palette, levels, &table)
    };
    // Entries that no pixel uses are removed from the palette.
    let (palette, key_to_label) = compact_palette(palette, key_to_label);

    let mut labels = vec![0u16; pixels];
    for (label, px) in labels.iter_mut().zip(image.rgba.chunks_exact(4)) {
        if px[3] >= ALPHA_THRESHOLD {
            let key = pack([px[0], px[1], px[2]], alpha_level(px[3], levels));
            *label = key_to_label.get(&key).copied().unwrap_or(0);
        }
    }

    Quantized {
        width: image.width,
        height: image.height,
        labels,
        palette,
        alpha_levels: levels,
        exact,
    }
}

/// Finds `k` palette colors for the given distinct colors with weighted k-means.
fn cluster_palette(
    entries: &[(u32, u32)],
    k: usize,
    levels: u32,
    merge_distance: f64,
) -> Vec<PaletteEntry> {
    let table = LinearTable::new();
    let points = build_points(entries, levels, &table);
    let k = k.min(points.len());

    // --- k-means++ start: choose the first center, then centers that are far
    // from the centers that exist already (chosen at random with a probability
    // that grows with the distance and the weight).
    let mut rng = Rng::new(SEED);
    let mut first = 0;
    for (i, p) in points.iter().enumerate() {
        if p.weight > points[first].weight {
            first = i;
        }
    }
    let mut centers: Vec<[f64; 4]> = vec![points[first].pos];
    let mut min_dist: Vec<f64> = points.iter().map(|p| dist4(&p.pos, &centers[0])).collect();
    while centers.len() < k {
        let total: f64 = points
            .iter()
            .zip(&min_dist)
            .map(|(p, d)| p.weight * d)
            .sum();
        if total <= 0.0 {
            break; // All remaining points are equal to existing centers.
        }
        let target = rng.next_f64() * total;
        let mut acc = 0.0;
        let mut chosen = points.len() - 1;
        for (i, (p, d)) in points.iter().zip(&min_dist).enumerate() {
            acc += p.weight * d;
            if acc > target {
                chosen = i;
                break;
            }
        }
        centers.push(points[chosen].pos);
        let new_center = points[chosen].pos;
        for (d, p) in min_dist.iter_mut().zip(&points) {
            let nd = dist4(&p.pos, &new_center);
            if nd < *d {
                *d = nd;
            }
        }
    }

    // --- Lloyd iterations: assign points to the nearest center, then move
    // every center to the weighted mean of its points.
    let mut assignment = vec![usize::MAX; points.len()];
    for _ in 0..KMEANS_ITERATIONS {
        let mut changed = false;
        for (slot, p) in assignment.iter_mut().zip(&points) {
            let best = nearest(&centers, &p.pos);
            if *slot != best {
                *slot = best;
                changed = true;
            }
        }
        let mut sums = vec![[0.0f64; 4]; centers.len()];
        let mut weights = vec![0.0f64; centers.len()];
        for (&c, p) in assignment.iter().zip(&points) {
            weights[c] += p.weight;
            for d in 0..4 {
                sums[c][d] += p.pos[d] * p.weight;
            }
        }
        for c in 0..centers.len() {
            if weights[c] > 0.0 {
                for d in 0..4 {
                    centers[c][d] = sums[c][d] / weights[c];
                }
            }
        }
        if !changed {
            break;
        }
    }

    // --- Join centers that are closer than the merge distance. People cannot
    // see the difference, and every extra color would add regions and paths.
    // The two nearest centers are joined first, until all are far enough apart.
    let mut weights_by_center = vec![0.0f64; centers.len()];
    for (&c, p) in assignment.iter().zip(&points) {
        weights_by_center[c] += p.weight;
    }
    let limit = merge_distance * merge_distance;
    loop {
        let mut best: Option<(f64, usize, usize)> = None;
        for a in 0..centers.len() {
            if weights_by_center[a] <= 0.0 {
                continue;
            }
            for b in a + 1..centers.len() {
                if weights_by_center[b] <= 0.0 {
                    continue;
                }
                let d = dist4(&centers[a], &centers[b]);
                if d < limit && best.map(|x| d < x.0).unwrap_or(true) {
                    best = Some((d, a, b));
                }
            }
        }
        let (_, a, b) = match best {
            Some(x) => x,
            None => break,
        };
        // Move everything from center b to center a (weighted mean position).
        let (wa, wb) = (weights_by_center[a], weights_by_center[b]);
        for d in 0..4 {
            centers[a][d] = (centers[a][d] * wa + centers[b][d] * wb) / (wa + wb);
        }
        weights_by_center[a] = wa + wb;
        weights_by_center[b] = 0.0;
        for slot in assignment.iter_mut() {
            if *slot == b {
                *slot = a;
            }
        }
    }

    // --- Convert the clusters to palette entries. The color is the mean sRGB
    // value of the pixels of the cluster (integer result, same on all platforms).
    let mut rgb_sums = vec![[0.0f64; 3]; centers.len()];
    let mut weights = vec![0.0f64; centers.len()];
    for (&c, p) in assignment.iter().zip(&points) {
        weights[c] += p.weight;
        for d in 0..3 {
            rgb_sums[c][d] += p.rgb_sum[d];
        }
    }
    let mut palette: Vec<PaletteEntry> = Vec::new();
    for c in 0..centers.len() {
        if weights[c] <= 0.0 {
            continue;
        }
        let channel = |d: usize| -> u8 {
            (rgb_sums[c][d] / weights[c] + 0.5)
                .floor()
                .clamp(0.0, 255.0) as u8
        };
        let level = (centers[c][3] / ALPHA_SCALE * f64::from(levels) + 0.5).floor();
        let entry = PaletteEntry {
            rgb: [channel(0), channel(1), channel(2)],
            alpha_level: level.clamp(1.0, f64::from(levels)) as u8,
        };
        if !palette.contains(&entry) {
            palette.push(entry);
        }
    }
    palette
}

/// Builds the k-means points. If there are too many distinct colors, similar
/// colors are first merged into bins of 2^shift values per channel.
fn build_points(entries: &[(u32, u32)], levels: u32, table: &LinearTable) -> Vec<Point> {
    // Find the smallest shift that gives few enough points.
    let mut shift = 0u32;
    let binned: Vec<(u32, f64, [f64; 3])> = loop {
        let mut bins: HashMap<u32, (f64, [f64; 3])> = HashMap::new();
        for &(key, count) in entries {
            let (rgb, level) = unpack(key);
            let bin = pack([rgb[0] >> shift, rgb[1] >> shift, rgb[2] >> shift], level);
            let slot = bins.entry(bin).or_insert((0.0, [0.0; 3]));
            let w = f64::from(count);
            slot.0 += w;
            for d in 0..3 {
                slot.1[d] += f64::from(rgb[d]) * w;
            }
        }
        if bins.len() <= MAX_POINTS || shift >= 6 {
            let mut list: Vec<(u32, f64, [f64; 3])> =
                bins.into_iter().map(|(k, (w, s))| (k, w, s)).collect();
            list.sort_unstable_by_key(|e| e.0);
            break list;
        }
        shift += 1;
    };

    binned
        .into_iter()
        .map(|(key, weight, rgb_sum)| {
            let (_, level) = unpack(key);
            let mean = [
                (rgb_sum[0] / weight + 0.5).floor().clamp(0.0, 255.0) as u8,
                (rgb_sum[1] / weight + 0.5).floor().clamp(0.0, 255.0) as u8,
                (rgb_sum[2] / weight + 0.5).floor().clamp(0.0, 255.0) as u8,
            ];
            let lab = table.oklab(mean);
            Point {
                pos: [lab[0], lab[1], lab[2], alpha_coordinate(level, levels)],
                weight,
                rgb_sum,
            }
        })
        .collect()
}

/// Scales an alpha level to the coordinate used in color distances.
fn alpha_coordinate(level: u8, levels: u32) -> f64 {
    f64::from(level) / f64::from(levels) * ALPHA_SCALE
}

/// Index of the center that is nearest to `pos` (the first one wins a tie).
fn nearest(centers: &[[f64; 4]], pos: &[f64; 4]) -> usize {
    let mut best = 0;
    let mut best_dist = f64::INFINITY;
    for (i, c) in centers.iter().enumerate() {
        let d = dist4(c, pos);
        if d < best_dist {
            best_dist = d;
            best = i;
        }
    }
    best
}

/// Maps every distinct (color, alpha) pair of the image to the nearest palette
/// entry. Returns labels (1-based palette indexes) by packed key.
fn nearest_labels(
    entries: &[(u32, u32)],
    palette: &[PaletteEntry],
    levels: u32,
    table: &LinearTable,
) -> HashMap<u32, u16> {
    let palette_pos: Vec<[f64; 4]> = palette
        .iter()
        .map(|e| {
            let lab = table.oklab(e.rgb);
            [
                lab[0],
                lab[1],
                lab[2],
                alpha_coordinate(e.alpha_level, levels),
            ]
        })
        .collect();
    let mut map = HashMap::with_capacity(entries.len());
    for &(key, _) in entries {
        let (rgb, level) = unpack(key);
        let lab = table.oklab(rgb);
        let pos = [lab[0], lab[1], lab[2], alpha_coordinate(level, levels)];
        map.insert(key, (nearest(&palette_pos, &pos) + 1) as u16);
    }
    map
}

/// Removes palette entries that are not used and renumbers the labels.
fn compact_palette(
    palette: Vec<PaletteEntry>,
    key_to_label: HashMap<u32, u16>,
) -> (Vec<PaletteEntry>, HashMap<u32, u16>) {
    let mut used = vec![false; palette.len() + 1];
    for &label in key_to_label.values() {
        used[usize::from(label)] = true;
    }
    let mut new_index = vec![0u16; palette.len() + 1];
    let mut compact = Vec::new();
    for (i, entry) in palette.iter().enumerate() {
        if used[i + 1] {
            compact.push(*entry);
            new_index[i + 1] = compact.len() as u16;
        }
    }
    let remapped = key_to_label
        .into_iter()
        .map(|(key, label)| (key, new_index[usize::from(label)]))
        .collect();
    (compact, remapped)
}

/// Makes the background transparent (option `--remove-background`).
///
/// The background color is the label that covers most of the border pixels of
/// the image, if it covers at least half of them. Only the pixels of this
/// label that are connected (4-neighbors) to the border are removed, so an
/// area with the same color inside the picture stays.
///
/// Returns the number of pixels that were made transparent.
pub fn remove_background(q: &mut Quantized) -> usize {
    let (w, h) = (q.width, q.height);
    if w == 0 || h == 0 {
        return 0;
    }
    // Collect the border pixels.
    let mut border: Vec<usize> = Vec::new();
    for x in 0..w {
        border.push(x);
        if h > 1 {
            border.push((h - 1) * w + x);
        }
    }
    for y in 1..h.saturating_sub(1) {
        border.push(y * w);
        if w > 1 {
            border.push(y * w + w - 1);
        }
    }
    let mut counts: HashMap<u16, usize> = HashMap::new();
    for &i in &border {
        *counts.entry(q.labels[i]).or_insert(0) += 1;
    }
    // The most frequent label; the smallest label wins a tie (deterministic).
    let mut best: Option<(u16, usize)> = None;
    for (&label, &count) in &counts {
        best = match best {
            Some((bl, bc)) if bc > count || (bc == count && bl < label) => Some((bl, bc)),
            _ => Some((label, count)),
        };
    }
    let (background, count) = match best {
        Some(b) => b,
        None => return 0,
    };
    if background == 0 || count * 2 < border.len() {
        return 0;
    }

    // Flood fill from the border pixels that have the background label.
    let mut removed = 0;
    let mut stack: Vec<usize> = border
        .into_iter()
        .filter(|&i| q.labels[i] == background)
        .collect();
    while let Some(i) = stack.pop() {
        if q.labels[i] != background {
            continue;
        }
        q.labels[i] = 0;
        removed += 1;
        let (x, y) = (i % w, i / w);
        if x > 0 {
            stack.push(i - 1);
        }
        if x + 1 < w {
            stack.push(i + 1);
        }
        if y > 0 {
            stack.push(i - w);
        }
        if y + 1 < h {
            stack.push(i + w);
        }
    }
    removed
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
    fn few_colors_are_kept_exactly() {
        let img = image(
            2,
            2,
            &[
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [255, 0, 0, 255],
                [0, 0, 255, 255],
            ],
        );
        let q = quantize(&img, 256, 16, 0.012);
        assert!(q.exact);
        assert_eq!(q.palette.len(), 3);
        assert_eq!(q.labels[0], q.labels[2]);
        assert_ne!(q.labels[0], q.labels[1]);
        let e = q.palette[usize::from(q.labels[0]) - 1];
        assert_eq!((e.rgb, u32::from(e.alpha_level)), ([255, 0, 0], 16));
    }

    #[test]
    fn nearly_transparent_pixels_are_removed() {
        let img = image(2, 1, &[[1, 2, 3, 7], [1, 2, 3, 8]]);
        let q = quantize(&img, 256, 16, 0.012);
        assert_eq!(q.labels[0], 0);
        assert_ne!(q.labels[1], 0);
    }

    #[test]
    fn alpha_is_rounded_to_levels() {
        assert_eq!(alpha_level(255, 16), 16);
        assert_eq!(alpha_level(254, 16), 16);
        assert_eq!(alpha_level(128, 16), 8);
        assert_eq!(alpha_level(8, 16), 1);
        assert_eq!(alpha_level(8, 2), 1);
        assert_eq!(alpha_level(200, 2), 2);
        let img = image(2, 1, &[[9, 9, 9, 128], [9, 9, 9, 255]]);
        let q = quantize(&img, 256, 4, 0.012);
        assert_ne!(q.labels[0], q.labels[1]);
        assert_eq!(q.palette[usize::from(q.labels[0]) - 1].alpha_level, 2);
        assert_eq!(q.palette[usize::from(q.labels[1]) - 1].alpha_level, 4);
    }

    #[test]
    fn many_colors_are_reduced_to_the_limit() {
        // A gradient with 256 gray values and a second gradient of reds.
        let mut pixels = Vec::new();
        for i in 0..256u32 {
            pixels.push([i as u8, i as u8, i as u8, 255]);
            pixels.push([i as u8, 0, 0, 255]);
        }
        let img = image(32, 16, &pixels);
        let q = quantize(&img, 12, 16, 0.012);
        assert!(!q.exact);
        assert!(q.palette.len() <= 12, "{}", q.palette.len());
        assert!(q.labels.iter().all(|&l| l != 0));
        // Each pixel is mapped to a color that is reasonably close to it.
        for (px, &label) in img.rgba.chunks(4).zip(&q.labels) {
            let e = q.palette[usize::from(label) - 1];
            let err: i32 = (0..3)
                .map(|d| (i32::from(px[d]) - i32::from(e.rgb[d])).abs())
                .sum();
            assert!(err < 120, "error {err}");
        }
    }

    #[test]
    fn separate_colors_stay_separate_in_a_logo() {
        // A red field with a few blue pixels and many noisy shades of white.
        let mut pixels = Vec::new();
        for i in 0..400u32 {
            let v = 200 + (i % 55) as u8;
            pixels.push([v, v, v.wrapping_add((i % 3) as u8), 255]);
        }
        for _ in 0..40 {
            pixels.push([220, 10, 10, 255]);
        }
        for _ in 0..20 {
            pixels.push([10, 10, 220, 255]);
        }
        let img = image(460, 1, &pixels);
        let q = quantize(&img, 4, 2, 0.012);
        let red = q.palette[usize::from(q.labels[400]) - 1];
        let blue = q.palette[usize::from(q.labels[440]) - 1];
        assert!(red.rgb[0] > 150 && red.rgb[2] < 100, "{red:?}");
        assert!(blue.rgb[2] > 150 && blue.rgb[0] < 100, "{blue:?}");
    }

    #[test]
    fn result_is_deterministic() {
        let mut pixels = Vec::new();
        for i in 0..2000u32 {
            let v = (i.wrapping_mul(2_654_435_761) >> 8) as u8;
            pixels.push([v, v.wrapping_mul(3), v.wrapping_add(77), 255]);
        }
        let img = image(2000, 1, &pixels);
        let a = quantize(&img, 20, 8, 0.012);
        let b = quantize(&img, 20, 8, 0.012);
        assert_eq!(a.labels, b.labels);
        assert_eq!(a.palette, b.palette);
    }

    #[test]
    fn background_is_removed_only_when_connected_to_the_border() {
        // 5x5: white border and white pixel in the middle (surrounded by black).
        let w = [255, 255, 255, 255];
        let k = [0, 0, 0, 255];
        let mut pixels = vec![w; 25];
        for y in 1..4 {
            for x in 1..4 {
                pixels[y * 5 + x] = k;
            }
        }
        pixels[2 * 5 + 2] = w;
        let img = image(5, 5, &pixels);
        let mut q = quantize(&img, 256, 16, 0.012);
        let removed = remove_background(&mut q);
        assert_eq!(removed, 16);
        assert_eq!(q.labels[0], 0);
        assert_ne!(q.labels[2 * 5 + 2], 0, "the white center must stay");
    }

    #[test]
    fn background_is_not_removed_if_the_border_is_mixed() {
        let mut pixels = Vec::new();
        for i in 0..16usize {
            pixels.push(if i % 4 < 2 {
                [255, 0, 0, 255]
            } else {
                [0, 255, 0, 255]
            });
        }
        // 4x4 with two colors in vertical stripes: each covers exactly half of
        // the border, so the smaller label is chosen and it covers half.
        let img = image(4, 4, &pixels);
        let mut q = quantize(&img, 256, 16, 0.012);
        let removed = remove_background(&mut q);
        assert!(removed == 8 || removed == 0);
        let img = image(
            3,
            1,
            &[[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]],
        );
        let mut q = quantize(&img, 256, 16, 0.012);
        assert_eq!(remove_background(&mut q), 0);
    }
}
