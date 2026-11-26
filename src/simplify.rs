//! Preparation of an arc for curve fitting: smoothing and corner detection.
//!
//! An arc from [`crate::trace`] is a staircase of unit steps along pixel edges.
//! A staircase cannot be fitted with a smooth curve directly, because every
//! step looks like a corner. This module
//!
//! 1. smooths the staircase with a short moving average (the average of the
//!    corner points lies close to the true edge of the shape),
//! 2. finds *real* corners, where the smoothed line turns sharply
//!    (like the corner of a square or a letter), as potrace does with its
//!    `alphamax` parameter (requirement ALG-07),
//! 3. cuts the arc at the corners into *pieces*, and every piece is a smooth
//!    sequence of points for the curve fitting in [`crate::bezier`].
//!
//! Short arcs are not smoothed. They are kept as exact polygons, because a few
//! pixels do not tell if a shape is round or has corners.
//!
//! All calculations use only `+ - * /` and `sqrt`, so the results are the same
//! on every platform (requirement SVG-07).

/// A point with real coordinates.
pub type Pt = (f64, f64);

/// A smooth part of an arc between two corners (or between the ends of the arc).
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    /// The points to fit. The first and the last point are the end points of
    /// the piece and they are exact (not smoothed).
    pub points: Vec<Pt>,
    /// Direction of the curve at the start, if it is known from the neighbor
    /// piece (closed smooth rings). `None` means "estimate it from the points".
    pub tangent_start: Option<Pt>,
    /// Direction of the curve at the end, pointing backward (into the piece).
    pub tangent_end: Option<Pt>,
}

/// Distance (in arc steps) between the points that are used to measure a turn.
const CORNER_SPAN: usize = 4;
/// A turn is a corner if the cosine of the angle between the incoming and the
/// outgoing direction is below this value. `0.7` is about a 45 degree turn.
const CORNER_COS: f64 = 0.7;
/// A turn of the pixel edge is always a corner if both straight runs next to
/// it have at least this many steps. This finds the corners of small shapes
/// (a small square, a short bar), where the turn measure has too little room.
const LATTICE_RUN: usize = 3;
/// Arcs with fewer steps than this are kept as exact polygons.
const SHORT_ARC: usize = 2 * CORNER_SPAN + 1;
/// Closed rings with fewer steps than this are kept as exact polygons.
const SHORT_RING: usize = 16;

/// Cuts an arc into pieces. `closed` tells if the arc is a ring (the first and
/// the last point are equal). `tolerance` is the fit tolerance of the detail
/// level. It also sets the smoothing strength. A tolerance of zero (or less)
/// switches the smoothing off: the arc is kept as the exact polygon of the
/// pixel edges.
pub fn split_arc(points: &[(i32, i32)], closed: bool, tolerance: f64) -> Vec<Piece> {
    let n = points.len().saturating_sub(1);
    if n == 0 {
        return Vec::new();
    }
    let raw: Vec<Pt> = points
        .iter()
        .map(|&(x, y)| (f64::from(x), f64::from(y)))
        .collect();

    if tolerance <= 0.0 || (!closed && n < SHORT_ARC) || (closed && n < SHORT_RING) {
        return polygon_pieces(&raw, closed);
    }
    // Smoothing window (points on each side): stronger for coarser detail levels.
    // Measured with the test images: 3 gives smaller files than 2 and looks
    // smoother, with the same similarity to the source.
    let window = if tolerance >= 2.0 { 4 } else { 3 };
    if closed {
        split_ring(&raw[..n], window)
    } else {
        split_open(&raw, window)
    }
}

/// Exact polygon: one two-point piece for every straight run of the staircase.
fn polygon_pieces(raw: &[Pt], closed: bool) -> Vec<Piece> {
    let n = raw.len() - 1;
    let mut vertices: Vec<Pt> = Vec::new();
    if closed {
        // Turning points of the ring (cyclic), so the polygon starts at a turn.
        let pts = &raw[..n];
        for i in 0..n {
            let prev = pts[(i + n - 1) % n];
            let next = pts[(i + 1) % n];
            let cur = pts[i];
            if !is_straight(prev, cur, next) {
                vertices.push(cur);
            }
        }
        if vertices.is_empty() {
            vertices.push(pts[0]);
        }
        let first = vertices[0];
        vertices.push(first);
    } else {
        vertices.push(raw[0]);
        for i in 1..n {
            if !is_straight(raw[i - 1], raw[i], raw[i + 1]) {
                vertices.push(raw[i]);
            }
        }
        vertices.push(raw[n]);
    }
    vertices
        .windows(2)
        .map(|w| Piece {
            points: vec![w[0], w[1]],
            tangent_start: None,
            tangent_end: None,
        })
        .collect()
}

/// `true` if the three points are on a straight line (for unit steps: no turn).
fn is_straight(a: Pt, b: Pt, c: Pt) -> bool {
    (b.0 - a.0) * (c.1 - b.1) - (b.1 - a.1) * (c.0 - b.0) == 0.0
}

/// Average of the points with indexes `i - w ..= i + w` (clamped to the range
/// `lo..=hi`, and symmetric so the average does not drift to one side).
fn smooth_at(points: &[Pt], i: usize, w: usize, lo: usize, hi: usize) -> Pt {
    let w = w.min(i - lo).min(hi - i);
    let (mut sx, mut sy) = (0.0, 0.0);
    for p in &points[i - w..=i + w] {
        sx += p.0;
        sy += p.1;
    }
    let count = (2 * w + 1) as f64;
    (sx / count, sy / count)
}

/// Cosine of the turn at index `i`, measured with chords of `CORNER_SPAN` steps.
fn turn_cosine(a: Pt, b: Pt, c: Pt) -> f64 {
    let u = (b.0 - a.0, b.1 - a.1);
    let v = (c.0 - b.0, c.1 - b.1);
    let lu = (u.0 * u.0 + u.1 * u.1).sqrt();
    let lv = (v.0 * v.0 + v.1 * v.1).sqrt();
    if lu == 0.0 || lv == 0.0 {
        return 1.0;
    }
    (u.0 * v.0 + u.1 * v.1) / (lu * lv)
}

/// Picks the corners from the turn cosines: an index is a corner if its cosine
/// is below [`CORNER_COS`] and it is the sharpest turn within `CORNER_SPAN / 2`
/// steps on both sides (non-maximum suppression). If two indexes have exactly
/// the same cosine, the first one wins. Two corners of a short side stay
/// separate, because every one is the sharpest turn in its own neighborhood.
///
/// `cosines[i]` is `None` where the turn cannot be measured. With `cyclic` the
/// neighborhood wraps around the ends (for rings).
fn pick_corners(cosines: &[Option<f64>], cyclic: bool) -> Vec<usize> {
    let n = cosines.len();
    let radius = CORNER_SPAN / 2;
    let mut corners = Vec::new();
    for i in 0..n {
        let c = match cosines[i] {
            Some(c) if c < CORNER_COS => c,
            _ => continue,
        };
        let mut sharpest = true;
        for step in 1..=radius {
            for (offset, before) in [(step, false), (step, true)] {
                let j = if before {
                    if i >= offset {
                        Some(i - offset)
                    } else if cyclic {
                        Some(i + n - offset)
                    } else {
                        None
                    }
                } else if i + offset < n {
                    Some(i + offset)
                } else if cyclic {
                    Some(i + offset - n)
                } else {
                    None
                };
                if let Some(Some(other)) = j.map(|j| cosines[j]) {
                    // A neighbor before the index must be strictly less sharp.
                    // A neighbor after it may be equal.
                    if (before && other <= c) || (!before && other < c) {
                        sharpest = false;
                    }
                }
            }
        }
        if sharpest {
            corners.push(i);
        }
    }
    corners
}

/// Finds lattice corners close to the ends of an open arc.
///
/// The turn measure of [`pick_corners`] needs `CORNER_SPAN` steps on both sides,
/// so it cannot see a corner that is only a few steps from the end of the arc.
/// This happens at the corners of the image: the boundary of a region runs
/// around the corner, and the arc ends one step later at a junction. Here a
/// point is a corner if the raw edge turns by 90 degrees there and the longer
/// side is a straight run of at least `CORNER_SPAN` steps.
fn end_corners(raw: &[Pt]) -> Vec<usize> {
    let n = raw.len() - 1;
    let step = |i: usize| (raw[i + 1].0 - raw[i].0, raw[i + 1].1 - raw[i].1);
    let mut corners = Vec::new();
    for i in 1..n {
        let near_start = i < CORNER_SPAN;
        let near_end = i + CORNER_SPAN > n;
        if !(near_start || near_end) || step(i - 1) == step(i) {
            continue;
        }
        if near_start {
            // The run after the corner must be straight and long enough.
            let run = (i..n).take_while(|&k| step(k) == step(i)).count();
            if run >= CORNER_SPAN {
                corners.push(i);
            }
        }
        if near_end {
            // The run before the corner must be straight and long enough.
            let run = (0..i).rev().take_while(|&k| step(k) == step(i - 1)).count();
            if run >= CORNER_SPAN {
                corners.push(i);
            }
        }
    }
    corners
}

/// Finds corners from the raw pixel edge: a 90 degree turn between two long
/// straight runs (see [`LATTICE_RUN`]). For a ring (`cyclic`), `raw` holds the
/// different points of the ring only.
fn lattice_corners(raw: &[Pt], cyclic: bool) -> Vec<usize> {
    let count = raw.len();
    // Number of steps; an open arc has one step less than it has points.
    let steps = if cyclic { count } else { count - 1 };
    let step = |i: usize| {
        let (a, b) = (raw[i % count], raw[(i + 1) % count]);
        (b.0 - a.0, b.1 - a.1)
    };
    let range = if cyclic { 0..steps } else { 1..steps };
    let mut corners = Vec::new();
    for i in range {
        let before = step((i + steps - 1) % steps);
        let after = step(i % steps);
        if before == after {
            continue;
        }
        // Length of the straight runs on both sides of the turn.
        let forward = (0..steps)
            .take_while(|&k| (cyclic || i + k < steps) && step((i + k) % steps) == after)
            .count();
        let backward = (1..=steps)
            .take_while(|&k| (cyclic || k <= i) && step((i + steps - k) % steps) == before)
            .count();
        if forward >= LATTICE_RUN && backward >= LATTICE_RUN {
            corners.push(i);
        }
    }
    corners
}

/// Joins two lists of corner indexes. A corner of the second list replaces
/// corners of the first list that are closer than `CORNER_SPAN / 2` steps,
/// because the exact lattice corner is better than the smoothed estimate.
fn merge_corners(smooth: Vec<usize>, exact: Vec<usize>, len: usize, cyclic: bool) -> Vec<usize> {
    let near = |a: usize, b: usize| {
        let d = a.abs_diff(b);
        let d = if cyclic { d.min(len - d) } else { d };
        d <= CORNER_SPAN / 2
    };
    let mut result: Vec<usize> = smooth
        .into_iter()
        .filter(|&c| !exact.iter().any(|&e| near(c, e)))
        .collect();
    result.extend(exact);
    result.sort_unstable();
    result.dedup();
    result
}

/// Splits an open arc.
fn split_open(raw: &[Pt], window: usize) -> Vec<Piece> {
    let n = raw.len() - 1;
    let g: Vec<Pt> = (0..=n).map(|i| smooth_at(raw, i, window, 0, n)).collect();
    let cosines: Vec<Option<f64>> = (0..=n)
        .map(|i| {
            if i >= CORNER_SPAN && i + CORNER_SPAN <= n {
                Some(turn_cosine(g[i - CORNER_SPAN], g[i], g[i + CORNER_SPAN]))
            } else {
                None
            }
        })
        .collect();
    let mut exact = lattice_corners(raw, false);
    exact.extend(end_corners(raw));
    let corners = merge_corners(pick_corners(&cosines, false), exact, n + 1, false);
    let mut cuts = vec![0];
    cuts.extend(corners);
    cuts.push(n);

    cuts.windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            // Inside a piece the points are smoothed with a window that gets
            // smaller near the ends, so the end points stay exact.
            let points = (a..=b).map(|i| smooth_at(raw, i, window, a, b)).collect();
            Piece {
                points,
                tangent_start: None,
                tangent_end: None,
            }
        })
        .collect()
}

/// Splits a closed ring. `raw` holds the `n` different points of the ring.
fn split_ring(raw: &[Pt], window: usize) -> Vec<Piece> {
    let n = raw.len();
    // Smooth around the ring (cyclic).
    let g: Vec<Pt> = (0..n)
        .map(|i| {
            let (mut sx, mut sy) = (0.0, 0.0);
            for k in 0..=2 * window {
                let p = raw[(i + n + k - window) % n];
                sx += p.0;
                sy += p.1;
            }
            let count = (2 * window + 1) as f64;
            (sx / count, sy / count)
        })
        .collect();
    let cosines: Vec<Option<f64>> = (0..n)
        .map(|i| {
            Some(turn_cosine(
                g[(i + n - CORNER_SPAN) % n],
                g[i],
                g[(i + CORNER_SPAN) % n],
            ))
        })
        .collect();
    let corners = merge_corners(
        pick_corners(&cosines, true),
        lattice_corners(raw, true),
        n,
        true,
    );

    if corners.is_empty() {
        // A smooth ring. Cut it into two halves; the curve must be smooth at
        // the cut points, so the tangents come from the whole ring.
        let cut = [0, n / 2];
        let tangent = |i: usize| -> Pt {
            let a = g[(i + n - 3) % n];
            let b = g[(i + 3) % n];
            normalize((b.0 - a.0, b.1 - a.1))
        };
        return (0..2)
            .map(|k| {
                let a = cut[k];
                let b = if k == 0 { cut[1] } else { n };
                let points: Vec<Pt> = (a..=b).map(|i| g[i % n]).collect();
                let t_start = tangent(a % n);
                let t_end = tangent(b % n);
                Piece {
                    points,
                    tangent_start: Some(t_start),
                    tangent_end: Some((-t_end.0, -t_end.1)),
                }
            })
            .collect();
    }

    // Rotate the ring so that it starts at the first corner, then cut it at
    // all corners like an open arc.
    let start = corners[0];
    let rotated: Vec<Pt> = (0..=n).map(|i| raw[(start + i) % n]).collect();
    let cuts: Vec<usize> = corners
        .iter()
        .map(|&c| (c + n - start) % n)
        .chain(std::iter::once(n))
        .collect();
    cuts.windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let points = (a..=b)
                .map(|i| smooth_at(&rotated, i, window, a, b))
                .collect();
            Piece {
                points,
                tangent_start: None,
                tangent_end: None,
            }
        })
        .collect()
}

/// Scales a vector to length 1. A zero vector stays zero.
pub fn normalize(v: Pt) -> Pt {
    let len = (v.0 * v.0 + v.1 * v.1).sqrt();
    if len == 0.0 {
        (0.0, 0.0)
    } else {
        (v.0 / len, v.1 / len)
    }
}

/// Helpers shared by the tests of several modules.
#[cfg(test)]
pub mod tests_support {
    /// Walks around the outer boundary of a simply connected set of cells.
    pub fn walk_cells(cells: &std::collections::BTreeSet<(i32, i32)>) -> Vec<(i32, i32)> {
        // Start at the top-left corner of the first cell and keep the cells on
        // the right-hand side (clockwise on screen).
        let first = *cells.iter().next().unwrap();
        let start = (first.0, first.1);
        let inside = |x: i32, y: i32| cells.contains(&(x, y));
        let mut pos = start;
        let mut dir = 0usize; // east
        let mut ring = vec![pos];
        for _ in 0..1_000_000 {
            let d = [(1, 0), (0, 1), (-1, 0), (0, -1)][dir];
            pos = (pos.0 + d.0, pos.1 + d.1);
            ring.push(pos);
            if pos == start && ring.len() > 2 {
                break;
            }
            // Cells ahead-left and ahead-right of the new position.
            let (ax, ay) = (pos.0, pos.1);
            let q = |sx: i32, sy: i32| {
                inside(
                    if sx > 0 { ax } else { ax - 1 },
                    if sy > 0 { ay } else { ay - 1 },
                )
            };
            let left_dir = (dir + 3) % 4;
            let dl = [(1, 0), (0, 1), (-1, 0), (0, -1)][left_dir];
            let dd = [(1, 0), (0, 1), (-1, 0), (0, -1)][dir];
            let dr = [(1, 0), (0, 1), (-1, 0), (0, -1)][(dir + 1) % 4];
            let ahead_left = q(dd.0 + dl.0, dd.1 + dl.1);
            let ahead_right = q(dd.0 + dr.0, dd.1 + dr.1);
            // The inside is on the right-hand side of the walker.
            dir = if !ahead_right {
                (dir + 1) % 4
            } else if ahead_left {
                (dir + 3) % 4
            } else {
                dir
            };
        }
        ring
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lattice points of the rectangle boundary with the given size, walked as
    /// a closed ring.
    fn rectangle_ring(w: i32, h: i32) -> Vec<(i32, i32)> {
        let mut pts = Vec::new();
        for x in 0..w {
            pts.push((x, 0));
        }
        for y in 0..h {
            pts.push((w, y));
        }
        for x in (1..=w).rev() {
            pts.push((x, h));
        }
        for y in (1..=h).rev() {
            pts.push((0, y));
        }
        pts.push(pts[0]);
        pts
    }

    #[test]
    fn short_arc_becomes_an_exact_polygon() {
        // An L shape: 3 steps right, then 3 steps down (6 steps, a short arc).
        let pts = [(0, 0), (1, 0), (2, 0), (3, 0), (3, 1), (3, 2), (3, 3)];
        let pieces = split_arc(&pts, false, 0.35);
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[0].points, vec![(0.0, 0.0), (3.0, 0.0)]);
        assert_eq!(pieces[1].points, vec![(3.0, 0.0), (3.0, 3.0)]);
    }

    #[test]
    fn zero_tolerance_keeps_the_exact_polygon() {
        // A long staircase that would be smoothed with a positive tolerance.
        let mut pts = vec![(0, 0)];
        for i in 0..20 {
            pts.push((i + 1, i));
            pts.push((i + 1, i + 1));
        }
        assert_eq!(split_arc(&pts, false, 0.35).len(), 1);
        assert_eq!(split_arc(&pts, false, 0.0).len(), 40);
    }

    #[test]
    fn straight_long_arc_is_one_piece() {
        let pts: Vec<(i32, i32)> = (0..=30).map(|x| (x, 0)).collect();
        let pieces = split_arc(&pts, false, 0.35);
        assert_eq!(pieces.len(), 1);
        assert_eq!(pieces[0].points.len(), 31);
        assert!(pieces[0].points.iter().all(|p| p.1 == 0.0));
    }

    #[test]
    fn right_angle_in_a_long_arc_is_a_corner() {
        let mut pts: Vec<(i32, i32)> = (0..=20).map(|x| (x, 0)).collect();
        pts.extend((1..=20).map(|y| (20, y)));
        let pieces = split_arc(&pts, false, 0.35);
        assert_eq!(pieces.len(), 2);
        // The corner point is exact.
        assert_eq!(pieces[0].points.last(), Some(&(20.0, 0.0)));
        assert_eq!(pieces[1].points.first(), Some(&(20.0, 0.0)));
    }

    #[test]
    fn corner_next_to_the_end_of_an_arc_is_found() {
        // The arc starts one step before a right angle (like at an image corner).
        let mut pts = vec![(0, 0), (1, 0)];
        pts.extend((1..=30).map(|y| (1, y)));
        let pieces = split_arc(&pts, false, 0.35);
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[0].points, vec![(0.0, 0.0), (1.0, 0.0)]);
        assert_eq!(pieces[1].points.first(), Some(&(1.0, 0.0)));
        // The same at the end of the arc.
        let mut pts: Vec<(i32, i32)> = (0..=30).map(|x| (x, 0)).collect();
        pts.push((30, 1));
        let pieces = split_arc(&pts, false, 0.35);
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[1].points, vec![(30.0, 0.0), (30.0, 1.0)]);
    }

    #[test]
    fn staircase_of_a_diagonal_has_no_corners() {
        let mut pts = vec![(0, 0)];
        for i in 0..20 {
            pts.push((i + 1, i));
            pts.push((i + 1, i + 1));
        }
        let pieces = split_arc(&pts, false, 0.35);
        assert_eq!(pieces.len(), 1);
    }

    #[test]
    fn small_rectangle_ring_keeps_its_four_corners() {
        // A 5 x 4 rectangle is too small for the smoothed turn measure.
        let ring = rectangle_ring(5, 4);
        let pieces = split_arc(&ring, true, 0.6);
        assert_eq!(pieces.len(), 4);
        for piece in &pieces {
            assert!(piece.tangent_start.is_none());
        }
    }

    #[test]
    fn one_pixel_jog_between_long_runs_is_not_a_corner() {
        // 10 steps east, 1 step south, 10 steps east: a step of a flat curve.
        let mut pts: Vec<(i32, i32)> = (0..=10).map(|x| (x, 0)).collect();
        pts.push((10, 1));
        pts.extend((11..=20).map(|x| (x, 1)));
        assert_eq!(split_arc(&pts, false, 0.35).len(), 1);
    }

    #[test]
    fn rectangle_ring_has_four_corners() {
        let ring = rectangle_ring(20, 12);
        let pieces = split_arc(&ring, true, 0.35);
        assert_eq!(pieces.len(), 4);
        for piece in &pieces {
            assert!(piece.points.len() >= 12);
        }
    }

    #[test]
    fn round_ring_is_cut_into_two_smooth_halves() {
        // A big circle drawn as a staircase on the pixel grid.
        let r = 20.0f64;
        let mut cells = std::collections::BTreeSet::new();
        for y in -25..25i32 {
            for x in -25..25i32 {
                let (cx, cy) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
                if cx * cx + cy * cy <= r * r {
                    cells.insert((x, y));
                }
            }
        }
        // Collect the boundary as a ring by walking along the cell edges.
        let ring = tests_support::walk_cells(&cells);
        let pieces = split_arc(&ring, true, 0.35);
        assert_eq!(pieces.len(), 2, "no corners expected on a circle");
        assert!(pieces[0].tangent_start.is_some());
    }

    #[test]
    fn pieces_never_contain_nan() {
        let ring = rectangle_ring(30, 30);
        for piece in split_arc(&ring, true, 3.0) {
            assert!(piece
                .points
                .iter()
                .all(|p| p.0.is_finite() && p.1.is_finite()));
        }
    }

    #[test]
    fn normalize_handles_zero() {
        assert_eq!(normalize((0.0, 0.0)), (0.0, 0.0));
        assert_eq!(normalize((3.0, 4.0)), (0.6, 0.8));
    }
}
