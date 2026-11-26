//! Segmentation: connected regions of equal labels, and noise removal.
//!
//! A *region* is a set of pixels that have the same label and are connected
//! through their 4 neighbors (left, right, up, down). Diagonal contact does not
//! connect pixels (requirement ALG-04).
//!
//! Small regions (fewer pixels than the minimum area of the detail level) are
//! merged into the neighbor region that has the most similar color (ALG-03).

use crate::color::{distance_sq, Lab, LinearTable};
use crate::quantize::Quantized;

/// Region id that stands for "outside of the image".
pub const OUTSIDE: u32 = u32::MAX;

/// The regions of an image.
#[derive(Debug, Clone)]
pub struct Regions {
    /// Image width in pixels.
    pub width: usize,
    /// Image height in pixels.
    pub height: usize,
    /// Region id of every pixel (ids are `0..count`, in scan order of the
    /// first pixel of each region).
    pub ids: Vec<u32>,
    /// Label of every region (0 = transparent, otherwise palette index + 1).
    pub label: Vec<u16>,
    /// Number of pixels of every region.
    pub area: Vec<u32>,
}

impl Regions {
    /// Number of regions.
    pub fn count(&self) -> usize {
        self.label.len()
    }
}

/// Finds the 4-connected regions of equal labels.
pub fn label_components(labels: &[u16], width: usize, height: usize) -> Regions {
    let mut ids = vec![OUTSIDE; width * height];
    let mut label: Vec<u16> = Vec::new();
    let mut area: Vec<u32> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();

    for start in 0..width * height {
        if ids[start] != OUTSIDE {
            continue;
        }
        let id = label.len() as u32;
        let value = labels[start];
        let mut count = 0u32;
        ids[start] = id;
        stack.push(start);
        // Depth-first flood fill with an explicit stack (no recursion, so a
        // huge region cannot overflow the call stack).
        while let Some(i) = stack.pop() {
            count += 1;
            let (x, y) = (i % width, i / width);
            if x > 0 && ids[i - 1] == OUTSIDE && labels[i - 1] == value {
                ids[i - 1] = id;
                stack.push(i - 1);
            }
            if x + 1 < width && ids[i + 1] == OUTSIDE && labels[i + 1] == value {
                ids[i + 1] = id;
                stack.push(i + 1);
            }
            if y > 0 && ids[i - width] == OUTSIDE && labels[i - width] == value {
                ids[i - width] = id;
                stack.push(i - width);
            }
            if y + 1 < height && ids[i + width] == OUTSIDE && labels[i + width] == value {
                ids[i + width] = id;
                stack.push(i + width);
            }
        }
        label.push(value);
        area.push(count);
    }
    Regions {
        width,
        height,
        ids,
        label,
        area,
    }
}

/// Runs the segmentation with noise removal.
///
/// Regions smaller than `min_area` pixels are merged into their most similar
/// neighbor. The labels in `q` are changed to the merged result. The function
/// repeats until no small regions are left (merging can create a new small
/// region only in rare cases, so the loop ends quickly).
pub fn segment(q: &mut Quantized, min_area: u32) -> Regions {
    let (w, h) = (q.width, q.height);
    let table = LinearTable::new();
    // Color of every label as a point in Oklab plus the alpha level.
    let positions: Vec<Option<[f64; 4]>> = std::iter::once(None)
        .chain(q.palette.iter().map(|e| {
            let lab = table.oklab(e.rgb);
            Some([
                lab[0],
                lab[1],
                lab[2],
                f64::from(e.alpha_level) / f64::from(q.alpha_levels) * 0.6,
            ])
        }))
        .collect();

    for _round in 0..8 {
        let regions = label_components(&q.labels, w, h);
        if min_area <= 1 || !merge_small_regions(q, &regions, min_area, &positions) {
            // Nothing was merged, so the regions are final. (When something
            // is merged, the loop computes the regions again.)
            return regions;
        }
    }
    label_components(&q.labels, w, h)
}

/// Color distance used to choose a neighbor. A transparent label is far from
/// every color, so an opaque speck is only removed into transparency if it has
/// no opaque neighbor.
fn label_distance(a: u16, b: u16, positions: &[Option<[f64; 4]>]) -> f64 {
    match (positions[usize::from(a)], positions[usize::from(b)]) {
        (Some(pa), Some(pb)) => {
            let la: Lab = [pa[0], pa[1], pa[2]];
            let lb: Lab = [pb[0], pb[1], pb[2]];
            let da = pa[3] - pb[3];
            distance_sq(&la, &lb) + da * da
        }
        // Two transparent regions are never neighbors (they would be one region).
        (None, None) => 0.0,
        _ => 1.0e9,
    }
}

/// Merges every region with fewer than `min_area` pixels into the neighbor with
/// the closest color. Returns `true` if at least one region was merged.
fn merge_small_regions(
    q: &mut Quantized,
    regions: &Regions,
    min_area: u32,
    positions: &[Option<[f64; 4]>],
) -> bool {
    let (w, h) = (q.width, q.height);
    let count = regions.count();

    // Collect the pixel lists of the small regions only.
    let mut small_slot: Vec<u32> = vec![u32::MAX; count];
    let mut small: Vec<usize> = Vec::new();
    for (id, &area) in regions.area.iter().enumerate() {
        if area < min_area {
            small_slot[id] = small.len() as u32;
            small.push(id);
        }
    }
    if small.is_empty() {
        return false;
    }
    let mut pixels: Vec<Vec<u32>> = vec![Vec::new(); small.len()];
    for (i, &id) in regions.ids.iter().enumerate() {
        let slot = small_slot[id as usize];
        if slot != u32::MAX {
            pixels[slot as usize].push(i as u32);
        }
    }

    // Working copies that change while regions are merged.
    let mut ids = regions.ids.clone();
    let mut area = regions.area.clone();
    let mut label = regions.label.clone();
    // A region that was merged into another one is dead.
    let mut dead = vec![false; count];

    // Process the smallest regions first. The id is the tie breaker.
    let mut order = small.clone();
    order.sort_unstable_by_key(|&id| (regions.area[id], id));

    let mut merged_any = false;
    for id in order {
        let slot = small_slot[id] as usize;
        if dead[id] || area[id] >= min_area {
            continue;
        }
        // Count the border length to every neighbor region.
        let mut neighbors: Vec<(u32, u32)> = Vec::new();
        for &p in &pixels[slot] {
            let p = p as usize;
            let (x, y) = (p % w, p / w);
            let mut visit = |n: usize| {
                let other = ids[n];
                if other as usize != id {
                    match neighbors.iter_mut().find(|e| e.0 == other) {
                        Some(e) => e.1 += 1,
                        None => neighbors.push((other, 1)),
                    }
                }
            };
            if x > 0 {
                visit(p - 1);
            }
            if x + 1 < w {
                visit(p + 1);
            }
            if y > 0 {
                visit(p - w);
            }
            if y + 1 < h {
                visit(p + w);
            }
        }
        if neighbors.is_empty() {
            continue; // The region covers the whole image.
        }

        // Choose the neighbor: closest color, then longest shared border,
        // then the smallest id.
        let my_label = label[id];
        let mut best = neighbors[0];
        let mut best_dist = label_distance(my_label, label[best.0 as usize], positions);
        for &(other, shared) in &neighbors[1..] {
            let d = label_distance(my_label, label[other as usize], positions);
            let better = d < best_dist
                || (d == best_dist && shared > best.1)
                || (d == best_dist && shared == best.1 && other < best.0);
            if better {
                best = (other, shared);
                best_dist = d;
            }
        }
        let target = best.0 as usize;
        let target_label = label[target];

        // Move the pixels of the small region to the target region.
        let moved = std::mem::take(&mut pixels[slot]);
        for &p in &moved {
            ids[p as usize] = target as u32;
            q.labels[p as usize] = target_label;
        }
        area[target] += area[id];
        area[id] = 0;
        dead[id] = true;
        label[id] = target_label;
        // If the target is small too, it takes the pixel list with it.
        let target_slot = small_slot[target];
        if target_slot != u32::MAX {
            pixels[target_slot as usize].extend_from_slice(&moved);
        }
        merged_any = true;
    }
    merged_any
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quantize::PaletteEntry;

    /// Builds a quantized image from a character map. Characters: '.' is
    /// transparent, 'r' red, 'g' green, 'k' black, 'R' a slightly different red.
    fn make(rows: &[&str]) -> Quantized {
        let palette = vec![
            PaletteEntry {
                rgb: [255, 0, 0],
                alpha_level: 2,
            },
            PaletteEntry {
                rgb: [0, 255, 0],
                alpha_level: 2,
            },
            PaletteEntry {
                rgb: [0, 0, 0],
                alpha_level: 2,
            },
            PaletteEntry {
                rgb: [250, 10, 10],
                alpha_level: 2,
            },
        ];
        let mut labels = Vec::new();
        for row in rows {
            for c in row.chars() {
                labels.push(match c {
                    '.' => 0,
                    'r' => 1,
                    'g' => 2,
                    'k' => 3,
                    'R' => 4,
                    other => panic!("bad char {other}"),
                });
            }
        }
        Quantized {
            width: rows[0].len(),
            height: rows.len(),
            labels,
            palette,
            alpha_levels: 2,
            exact: true,
        }
    }

    #[test]
    fn diagonal_pixels_are_separate_regions() {
        let q = make(&["rg", "gr"]);
        let regions = label_components(&q.labels, 2, 2);
        assert_eq!(regions.count(), 4);
    }

    #[test]
    fn regions_have_ids_in_scan_order_and_areas() {
        let q = make(&["rrg", "rrg"]);
        let regions = label_components(&q.labels, 3, 2);
        assert_eq!(regions.count(), 2);
        assert_eq!(regions.ids, [0, 0, 1, 0, 0, 1]);
        assert_eq!(regions.area, [4, 2]);
        assert_eq!(regions.label, [1, 2]);
    }

    #[test]
    fn small_region_joins_the_neighbor_with_the_closest_color() {
        // The single 'R' (a red shade) touches green and red regions. It must
        // join the red one, although the border to green is equally long.
        let mut q = make(&["rrrRggg", "rrrrggg", "rrrrggg"]);
        let regions = segment(&mut q, 2);
        assert_eq!(regions.count(), 2);
        assert_eq!(q.labels[3], 1, "the shade became red");
    }

    #[test]
    fn small_transparent_hole_is_filled() {
        let mut q = make(&["rrr", "r.r", "rrr"]);
        let regions = segment(&mut q, 2);
        assert_eq!(regions.count(), 1);
        assert!(q.labels.iter().all(|&l| l == 1));
    }

    #[test]
    fn speck_on_transparent_background_is_removed() {
        let mut q = make(&["....", ".k..", "...."]);
        let regions = segment(&mut q, 2);
        assert_eq!(regions.count(), 1);
        assert!(q.labels.iter().all(|&l| l == 0));
    }

    #[test]
    fn min_area_one_keeps_every_pixel() {
        let mut q = make(&["rgr", "gkg", "rgr"]);
        let before = q.labels.clone();
        let regions = segment(&mut q, 1);
        assert_eq!(q.labels, before);
        assert_eq!(regions.count(), 9);
    }

    #[test]
    fn merging_never_leaves_small_regions_behind() {
        let mut q = make(&["rgrgrg", "gkgkgk", "rgrgrg", "gkgkgk"]);
        let regions = segment(&mut q, 3);
        assert!(
            regions.area.iter().all(|&a| a >= 3 || regions.count() == 1),
            "{:?}",
            regions.area
        );
        // The labels and the region map describe the same picture.
        for (i, &id) in regions.ids.iter().enumerate() {
            assert_eq!(regions.label[id as usize], q.labels[i]);
        }
    }

    #[test]
    fn same_color_regions_that_touch_after_merging_become_one() {
        // The 'k' pixel between two red areas is merged into red, and the two
        // red areas must become one region.
        let mut q = make(&["rrkrr", "rrrrr"]);
        let regions = segment(&mut q, 2);
        assert_eq!(regions.count(), 1);
    }
}
