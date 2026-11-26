//! Builds the drawing layers of the SVG from the regions and their boundaries.
//!
//! One *layer* is one `<path>` element: all regions of one palette color
//! (requirement SVG-09). The layers are drawn from the biggest color to the
//! smallest one (requirements ALG-11 and ALG-13).
//!
//! # Gaps between neighbors
//!
//! Browsers draw the edge of a shape with anti-aliasing. If two shapes touch
//! along a shared edge, a thin light line can appear between them. Two methods
//! prevent this:
//!
//! 1. **Underlay.** If a region has a hole and everything inside the hole is
//!    drawn later (and is fully opaque), the hole is not cut out. The lower
//!    layer extends under the upper layers. This is also shorter, because the
//!    boundary of the hole is not written twice.
//! 2. **Base rectangle.** If the whole image is opaque, the first layer also
//!    covers the complete image rectangle. Every light line between two later
//!    layers then shows the color of the first layer, and never the page
//!    behind the image. (For an image with transparency this is not allowed,
//!    because the transparent areas must stay transparent.)
//! 3. **Overlap stroke.** If two opaque neighbors touch side by side, the layer
//!    that is drawn first gets a very thin stroke in its own color. The stroke
//!    makes it a little wider, so it reaches under the layer that is drawn
//!    later and no light line can show between them.

use std::thread;

use crate::bezier::{fit_arc, FittedArc, Seg};
use crate::detail::Params;
use crate::quantize::Quantized;
use crate::segment::Regions;
use crate::trace::{Topology, NONE};

/// A closed outline: a start point and the segments that lead back to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subpath {
    /// Start point in grid units.
    pub start: (i32, i32),
    /// The segments. The last one ends at the start point.
    pub segs: Vec<Seg>,
}

/// All outlines of one color.
#[derive(Debug, Clone)]
pub struct Layer {
    /// Fill color.
    pub rgb: [u8; 3],
    /// Opacity level `1..=alpha_levels`.
    pub alpha_level: u8,
    /// `true` if this layer needs the thin overlap stroke.
    pub stroke: bool,
    /// The outlines.
    pub subpaths: Vec<Subpath>,
}

/// Counters for the `--verbose` output.
#[derive(Debug, Clone, Copy, Default)]
pub struct VectorStats {
    /// Number of shared boundary arcs.
    pub arcs: usize,
    /// Number of outlines that were written.
    pub contours: usize,
    /// Number of holes that were not cut out (underlay).
    pub filled_holes: usize,
}

/// The result of the vectorization.
#[derive(Debug, Clone)]
pub struct Vector {
    /// The layers in drawing order (first = bottom).
    pub layers: Vec<Layer>,
    /// Number of opacity levels (the highest level is opaque).
    pub alpha_levels: u32,
    /// Number of decimal places of the coordinates.
    pub precision: u32,
    /// Counters.
    pub stats: VectorStats,
}

/// Multiplier of the coordinate grid for a number of decimal places.
pub fn scale_for(precision: u32) -> i32 {
    10i32.pow(precision.min(3))
}

/// Creates the layers.
pub fn vectorize(
    q: &Quantized,
    regions: &Regions,
    topo: &Topology,
    params: &Params,
    precision: u32,
) -> Vector {
    let scale = scale_for(precision);
    let fitted = fit_all_arcs(topo, params.tolerance, scale);
    let max_level = q.alpha_levels as u8;

    // --- Drawing order of the colors. Opaque colors are first, the biggest
    // area first. Semi-transparent colors are last.
    let label_count = q.palette.len() + 1;
    let mut label_area = vec![0u64; label_count];
    for (region, &label) in regions.label.iter().enumerate() {
        label_area[usize::from(label)] += u64::from(regions.area[region]);
    }
    let mut labels_in_order: Vec<usize> = (1..label_count).filter(|&l| label_area[l] > 0).collect();
    let is_opaque = |label: usize| q.palette[label - 1].alpha_level == max_level;
    labels_in_order.sort_by_key(|&l| (!is_opaque(l), std::cmp::Reverse(label_area[l]), l));
    let mut order = vec![usize::MAX; label_count];
    for (rank, &l) in labels_in_order.iter().enumerate() {
        order[l] = rank;
    }

    // --- Decide which contours are written.
    let adjacency = topo.adjacency(regions.count());
    let mut checker = HoleChecker::new(regions.count());
    let mut write_contour = vec![false; topo.contours.len()];
    let mut stats = VectorStats {
        arcs: topo.arcs.len(),
        ..VectorStats::default()
    };
    for (i, contour) in topo.contours.iter().enumerate() {
        let label = usize::from(regions.label[contour.region as usize]);
        if label == 0 {
            continue; // Transparent regions are not drawn.
        }
        if contour.is_hole {
            let can_fill = checker.interior_is_drawn_later(
                topo,
                regions,
                &adjacency,
                contour,
                &|l: usize| l != 0 && is_opaque(l),
                &order,
                label,
            );
            if can_fill {
                stats.filled_holes += 1;
                continue;
            }
        }
        write_contour[i] = true;
    }

    // --- How many times is every arc written? (0, 1 or 2)
    let mut written = vec![0u8; topo.arcs.len()];
    for (i, contour) in topo.contours.iter().enumerate() {
        if write_contour[i] {
            for da in &contour.arcs {
                written[da.arc as usize] += 1;
            }
        }
    }

    // --- Build the layers.
    let mut layers: Vec<Layer> = labels_in_order
        .iter()
        .map(|&l| Layer {
            rgb: q.palette[l - 1].rgb,
            alpha_level: q.palette[l - 1].alpha_level,
            stroke: false,
            subpaths: Vec::new(),
        })
        .collect();
    for (i, contour) in topo.contours.iter().enumerate() {
        if !write_contour[i] {
            continue;
        }
        let label = usize::from(regions.label[contour.region as usize]);
        let layer = &mut layers[order[label]];
        let first = &fitted[contour.arcs[0].arc as usize];
        let start = if contour.arcs[0].reversed {
            first.end()
        } else {
            first.start
        };
        let mut segs: Vec<Seg> = Vec::new();
        for da in &contour.arcs {
            let arc = &fitted[da.arc as usize];
            if da.reversed {
                segs.extend(arc.reversed().segs);
            } else {
                segs.extend_from_slice(&arc.segs);
            }
            // A layer gets the overlap stroke if it touches a layer that is drawn
            // later along a shared edge. The stroke makes it extend a little
            // under the later layer. The later layer itself stays exact.
            let geometry = &topo.arcs[da.arc as usize];
            let other = if da.reversed {
                geometry.left
            } else {
                geometry.right
            };
            if other != NONE && written[da.arc as usize] == 2 && is_opaque(label) {
                let other_label = usize::from(regions.label[other as usize]);
                if other_label != 0 && is_opaque(other_label) && order[other_label] > order[label] {
                    layer.stroke = true;
                }
            }
        }
        if !segs.is_empty() {
            layer.subpaths.push(Subpath { start, segs });
            stats.contours += 1;
        }
    }
    // A fully opaque picture gets a base rectangle under the first layer. It is
    // not added if the first layer has this outline already.
    let fully_opaque = regions
        .label
        .iter()
        .all(|&l| l != 0 && is_opaque(usize::from(l)));
    if fully_opaque && !layers.is_empty() {
        let (w, h) = (regions.width as i32 * scale, regions.height as i32 * scale);
        if !layers[0].subpaths.iter().any(|s| is_rectangle(s, w, h)) {
            // The corners go around in the direction of an outer outline.
            layers[0].subpaths.insert(
                0,
                Subpath {
                    start: (0, 0),
                    segs: vec![
                        Seg::Line((0, h)),
                        Seg::Line((w, h)),
                        Seg::Line((w, 0)),
                        Seg::Line((0, 0)),
                    ],
                },
            );
        }
    }
    layers.retain(|l| !l.subpaths.is_empty());

    Vector {
        layers,
        alpha_levels: q.alpha_levels,
        precision,
        stats,
    }
}

/// `true` if the outline is the rectangle `(0, 0) - (w, h)` (any start corner).
fn is_rectangle(sub: &Subpath, w: i32, h: i32) -> bool {
    if sub.segs.len() != 4 || sub.segs.iter().any(|s| !matches!(s, Seg::Line(_))) {
        return false;
    }
    let mut corners = vec![sub.start];
    corners.extend(sub.segs.iter().map(Seg::end));
    corners.sort_unstable();
    corners.dedup();
    corners == [(0, 0), (0, h), (w, 0), (w, h)]
}

/// Fits all arcs. Big images are processed by several threads. The arcs are
/// split into fixed blocks and the results are joined in the original order,
/// so the output does not depend on the thread timing (requirement SVG-07).
fn fit_all_arcs(topo: &Topology, tolerance: f64, scale: i32) -> Vec<FittedArc> {
    let arcs = &topo.arcs;
    let work = |range: &[crate::trace::Arc]| -> Vec<FittedArc> {
        range
            .iter()
            .map(|a| fit_arc(&a.points, a.closed, tolerance, scale))
            .collect()
    };
    let total_points: usize = arcs.iter().map(|a| a.points.len()).sum();
    let threads = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(8);
    if threads <= 1 || total_points < 50_000 {
        return work(arcs);
    }
    let block = arcs.len().div_ceil(threads * 4).max(1);
    let mut result: Vec<FittedArc> = Vec::with_capacity(arcs.len());
    thread::scope(|scope| {
        let handles: Vec<_> = arcs
            .chunks(block)
            .map(|chunk| scope.spawn(move || work(chunk)))
            .collect();
        for handle in handles {
            match handle.join() {
                Ok(part) => result.extend(part),
                // A panic in a worker is not expected. Re-raise it so the
                // top level handler reports it as an internal error.
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }
    });
    result
}

/// Checks if the inside of a hole is completely covered by later layers.
struct HoleChecker {
    /// Marks of regions that were visited by the current search.
    stamp: Vec<u32>,
    /// Number of the current search.
    epoch: u32,
}

impl HoleChecker {
    fn new(regions: usize) -> HoleChecker {
        HoleChecker {
            stamp: vec![0; regions],
            epoch: 0,
        }
    }

    /// Returns `true` if the hole can be filled: every region inside the hole
    /// is opaque and either has the same color as the region that owns the
    /// hole, or is drawn later than it.
    #[allow(clippy::too_many_arguments)]
    fn interior_is_drawn_later(
        &mut self,
        topo: &Topology,
        regions: &Regions,
        adjacency: &[Vec<u32>],
        hole: &crate::trace::Contour,
        is_covering: &dyn Fn(usize) -> bool,
        order: &[usize],
        owner_label: usize,
    ) -> bool {
        self.epoch += 1;
        let epoch = self.epoch;
        let owner = hole.region;
        // The regions that touch the hole are inside it.
        let mut stack: Vec<u32> = Vec::new();
        for da in &hole.arcs {
            let arc = &topo.arcs[da.arc as usize];
            let inside = if da.reversed { arc.left } else { arc.right };
            if inside != NONE && self.stamp[inside as usize] != epoch {
                self.stamp[inside as usize] = epoch;
                stack.push(inside);
            }
        }
        // Search the whole inside. The owner region separates it from the
        // rest of the image, so the search cannot leave the hole.
        while let Some(region) = stack.pop() {
            let label = usize::from(regions.label[region as usize]);
            let ok =
                is_covering(label) && (label == owner_label || order[label] > order[owner_label]);
            if !ok {
                return false;
            }
            for &next in &adjacency[region as usize] {
                if next != owner && self.stamp[next as usize] != epoch {
                    self.stamp[next as usize] = epoch;
                    stack.push(next);
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detail::params_for;
    use crate::quantize::PaletteEntry;
    use crate::segment::segment;
    use crate::trace::trace;

    /// Builds a quantized image from rows of characters.
    /// '.' transparent, 'a' big color, 'b' second color, 'c' third color,
    /// 's' semi-transparent.
    fn make(rows: &[&str]) -> Quantized {
        let palette = vec![
            PaletteEntry {
                rgb: [255, 255, 255],
                alpha_level: 2,
            },
            PaletteEntry {
                rgb: [255, 0, 0],
                alpha_level: 2,
            },
            PaletteEntry {
                rgb: [0, 0, 255],
                alpha_level: 2,
            },
            PaletteEntry {
                rgb: [0, 255, 0],
                alpha_level: 1,
            },
        ];
        let labels = rows
            .iter()
            .flat_map(|r| {
                r.chars().map(|c| match c {
                    '.' => 0,
                    'a' => 1,
                    'b' => 2,
                    'c' => 3,
                    's' => 4,
                    _ => panic!(),
                })
            })
            .collect();
        Quantized {
            width: rows[0].len(),
            height: rows.len(),
            labels,
            palette,
            alpha_levels: 2,
            exact: true,
        }
    }

    fn run(rows: &[&str]) -> Vector {
        let mut q = make(rows);
        let regions = segment(&mut q, 1);
        let topo = trace(&regions);
        vectorize(&q, &regions, &topo, &params_for(100), 2)
    }

    #[test]
    fn enclosed_shape_fills_the_hole_of_the_background() {
        let v = run(&["aaaaa", "abbba", "abbba", "aaaaa"]);
        assert_eq!(v.layers.len(), 2);
        // Layer 0 is the big color and has one outline only (no hole).
        assert_eq!(v.layers[0].rgb, [255, 255, 255]);
        assert_eq!(v.layers[0].subpaths.len(), 1);
        assert_eq!(v.layers[1].rgb, [255, 0, 0]);
        assert_eq!(v.stats.filled_holes, 1);
        // The inner shape does not need a stroke: it is above the filled hole.
        assert!(!v.layers[1].stroke);
    }

    #[test]
    fn hole_stays_if_it_contains_transparency() {
        let v = run(&["aaaaa", "a...a", "a...a", "aaaaa"]);
        assert_eq!(v.layers.len(), 1);
        assert_eq!(v.layers[0].subpaths.len(), 2, "outer outline and hole");
        assert_eq!(v.stats.filled_holes, 0);
    }

    #[test]
    fn hole_stays_if_a_bigger_color_is_inside() {
        // The inner 'b' area is bigger than the 'a' ring, so 'b' is drawn first.
        let v = run(&[
            "aaaaaaaaa",
            "abbbbbbba",
            "abbbbbbba",
            "abbbbbbba",
            "abbbbbbba",
            "abbbbbbba",
            "abbbbbbba",
            "aaaaaaaaa",
        ]);
        assert_eq!(v.layers.len(), 2);
        assert_eq!(v.stats.filled_holes, 0);
        // The red layer is below, and the white ring above it has a hole.
        assert_eq!(v.layers[0].rgb, [255, 0, 0]);
        assert_eq!(v.layers[1].subpaths.len(), 2);
        // The first layer reaches under the second one. The second one is exact.
        assert!(v.layers[0].stroke && !v.layers[1].stroke);
    }

    #[test]
    fn neighbors_side_by_side_get_the_stroke() {
        let v = run(&["aaaaaa", "aaaaaa", "bbbccc", "bbbccc"]);
        // 'a' is biggest (12), then 'b' and 'c' (6 each... a=12, b=6, c=6).
        let strokes: Vec<bool> = v.layers.iter().map(|l| l.stroke).collect();
        // Every layer that touches a later layer gets the stroke. The last layer
        // is exact, so it has no stroke.
        assert_eq!(strokes, vec![true, true, false]);
    }

    #[test]
    fn semi_transparent_layers_are_last_and_have_no_stroke() {
        let v = run(&["aaaa", "aass", "aass"]);
        assert_eq!(v.layers.len(), 2);
        assert_eq!(v.layers[1].alpha_level, 1);
        assert!(!v.layers[1].stroke);
        // The hole of 'a' stays: a semi-transparent shape would show 'a' through.
        assert_eq!(v.stats.filled_holes, 0);
    }

    #[test]
    fn opaque_picture_gets_a_base_rectangle_under_the_first_layer() {
        let v = run(&["aabb", "aabb"]);
        // The first subpath of the first layer is the image rectangle.
        assert_eq!(v.layers[0].subpaths[0].start, (0, 0));
        assert_eq!(v.layers[0].subpaths[0].segs[1], Seg::Line((400, 200)));
        // A picture with transparency has no base rectangle.
        let v = run(&["aab.", "aab."]);
        assert!(v.layers[0]
            .subpaths
            .iter()
            .all(|s| s.segs.len() != 4 || s.segs[1] != Seg::Line((400, 200))));
    }

    #[test]
    fn fully_transparent_image_has_no_layers() {
        let v = run(&["...", "..."]);
        assert!(v.layers.is_empty());
    }

    #[test]
    fn contours_are_closed_and_connected() {
        let v = run(&["aaaaa", "abbca", "abbca", "aaaaa"]);
        for layer in &v.layers {
            for sub in &layer.subpaths {
                assert_eq!(sub.segs.last().map(Seg::end), Some(sub.start));
            }
        }
    }

    #[test]
    fn scale_matches_precision() {
        assert_eq!(scale_for(0), 1);
        assert_eq!(scale_for(2), 100);
        assert_eq!(scale_for(9), 1000);
    }
}
