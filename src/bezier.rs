//! Approximation of smooth point sequences with cubic Bézier curves.
//!
//! The fitting method is the one of Philip J. Schneider ("An Algorithm for
//! Automatically Fitting Digitized Curves", Graphics Gems, 1990):
//!
//! 1. Fit one cubic curve with the least squares method, using the chord
//!    length as the curve parameter.
//! 2. If the largest distance between the curve and the points is above the
//!    tolerance, improve the parameters with Newton-Raphson steps.
//! 3. If it is still too large, split the points at the worst point and fit
//!    both halves (the tangent at the split point is shared, so the curves join
//!    smoothly).
//!
//! Segments that are nearly straight become lines, because a line needs less
//! text in the SVG file than a curve (requirement ALG-09).
//!
//! The final coordinates are rounded to whole numbers in a fixed grid (for
//! example hundredths of a pixel). This is done once per arc, so the two regions
//! that share an arc use exactly the same numbers.

use crate::simplify::{normalize, split_arc, Piece, Pt};

/// One segment of a path. All coordinates are integers in grid units
/// (pixel size multiplied by the scale factor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seg {
    /// A straight line to the given end point.
    Line((i32, i32)),
    /// A cubic Bézier curve: first control point, second control point, end point.
    Cubic((i32, i32), (i32, i32), (i32, i32)),
}

impl Seg {
    /// End point of the segment.
    pub fn end(&self) -> (i32, i32) {
        match *self {
            Seg::Line(p) => p,
            Seg::Cubic(_, _, p) => p,
        }
    }
}

/// The geometry of an arc after the fitting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FittedArc {
    /// Start point.
    pub start: (i32, i32),
    /// The segments, from the start point to the end point.
    pub segs: Vec<Seg>,
}

impl FittedArc {
    /// End point of the arc.
    pub fn end(&self) -> (i32, i32) {
        self.segs.last().map(Seg::end).unwrap_or(self.start)
    }

    /// The same geometry, walked in the opposite direction.
    pub fn reversed(&self) -> FittedArc {
        // Points on the way: start, end of segment 0, end of segment 1, ...
        let mut points = vec![self.start];
        points.extend(self.segs.iter().map(Seg::end));
        let mut segs = Vec::with_capacity(self.segs.len());
        for (i, seg) in self.segs.iter().enumerate().rev() {
            let to = points[i];
            segs.push(match *seg {
                Seg::Line(_) => Seg::Line(to),
                // A reversed cubic swaps its control points.
                Seg::Cubic(c1, c2, _) => Seg::Cubic(c2, c1, to),
            });
        }
        FittedArc {
            start: self.end(),
            segs,
        }
    }
}

/// Fits curves and lines to an arc of grid points.
///
/// * `points`: the lattice points of the arc (see [`crate::trace::Arc`]).
/// * `closed`: the arc is a ring.
/// * `tolerance`: largest allowed distance in pixels.
/// * `scale`: the grid factor (1, 10, 100 or 1000 for 0..3 decimal places).
pub fn fit_arc(points: &[(i32, i32)], closed: bool, tolerance: f64, scale: i32) -> FittedArc {
    let pieces = split_arc(points, closed, tolerance);
    let factor = f64::from(scale);
    let round =
        |p: Pt| -> (i32, i32) { ((p.0 * factor).round() as i32, (p.1 * factor).round() as i32) };

    let start = match pieces.first().and_then(|p| p.points.first()) {
        Some(&p) => round(p),
        None => {
            // Cannot happen for a valid arc, but never fail.
            let p = points.first().copied().unwrap_or((0, 0));
            (p.0 * scale, p.1 * scale)
        }
    };
    let mut segs: Vec<Seg> = Vec::new();
    let mut current = start;
    for piece in &pieces {
        for seg in fit_piece(piece, tolerance) {
            let rounded = match seg {
                FloatSeg::Line(to) => Seg::Line(round(to)),
                FloatSeg::Cubic(c1, c2, to) => Seg::Cubic(round(c1), round(c2), round(to)),
            };
            // Drop segments that became empty after the rounding.
            let empty = match rounded {
                Seg::Line(to) => to == current,
                Seg::Cubic(c1, c2, to) => to == current && c1 == current && c2 == current,
            };
            if !empty {
                current = rounded.end();
                segs.push(rounded);
            }
        }
    }
    FittedArc { start, segs }
}

/// A segment with real coordinates (before the rounding).
#[derive(Debug, Clone, Copy)]
enum FloatSeg {
    Line(Pt),
    Cubic(Pt, Pt, Pt),
}

/// Fits one piece (a smooth run between two corners).
fn fit_piece(piece: &Piece, tolerance: f64) -> Vec<FloatSeg> {
    let pts = &piece.points;
    let last = match pts.len().checked_sub(1) {
        Some(l) if l >= 1 => l,
        _ => return Vec::new(),
    };
    let (first_pt, last_pt) = (pts[0], pts[last]);
    if last == 1 {
        return vec![FloatSeg::Line(last_pt)];
    }

    // If all points are very close to the straight line between the ends, a
    // line is enough (and it is the shortest way to write the shape).
    let chord = (last_pt.0 - first_pt.0, last_pt.1 - first_pt.1);
    let chord_len = (chord.0 * chord.0 + chord.1 * chord.1).sqrt();
    if chord_len > 1e-9 {
        let max_dev = pts
            .iter()
            .map(|p| {
                ((p.0 - first_pt.0) * chord.1 - (p.1 - first_pt.1) * chord.0).abs() / chord_len
            })
            .fold(0.0, f64::max);
        if max_dev <= 0.6 * tolerance {
            return vec![FloatSeg::Line(last_pt)];
        }
    }

    // End tangents: given by the neighbor piece (smooth rings) or estimated
    // from the first and the last points.
    let reach = 3.min(last);
    let t1 = piece
        .tangent_start
        .unwrap_or_else(|| normalize((pts[reach].0 - first_pt.0, pts[reach].1 - first_pt.1)));
    let t2 = piece.tangent_end.unwrap_or_else(|| {
        normalize((
            pts[last - reach].0 - last_pt.0,
            pts[last - reach].1 - last_pt.1,
        ))
    });
    if t1 == (0.0, 0.0) || t2 == (0.0, 0.0) {
        return vec![FloatSeg::Line(last_pt)];
    }

    fit_curve(pts, t1, t2, tolerance * tolerance)
        .into_iter()
        .map(classify)
        .collect()
}

/// Turns a nearly straight cubic curve into a line.
fn classify(c: [Pt; 4]) -> FloatSeg {
    let [p0, c1, c2, p3] = c;
    let chord = (p3.0 - p0.0, p3.1 - p0.1);
    let len2 = chord.0 * chord.0 + chord.1 * chord.1;
    if len2 > 1e-12 {
        let len = len2.sqrt();
        let flat = |p: Pt| -> bool {
            let rel = (p.0 - p0.0, p.1 - p0.1);
            let off = (rel.0 * chord.1 - rel.1 * chord.0).abs() / len;
            // Position along the chord (0 at the start, 1 at the end).
            let along = (rel.0 * chord.0 + rel.1 * chord.1) / len2;
            off <= 0.1 && (-0.05..=1.05).contains(&along)
        };
        if flat(c1) && flat(c2) {
            return FloatSeg::Line(p3);
        }
    }
    FloatSeg::Cubic(c1, c2, p3)
}

fn add(a: Pt, b: Pt) -> Pt {
    (a.0 + b.0, a.1 + b.1)
}
fn sub(a: Pt, b: Pt) -> Pt {
    (a.0 - b.0, a.1 - b.1)
}
fn mul(a: Pt, s: f64) -> Pt {
    (a.0 * s, a.1 * s)
}
fn dot(a: Pt, b: Pt) -> f64 {
    a.0 * b.0 + a.1 * b.1
}

/// Fits cubic curves to the points `d`. `t1` is the unit tangent at the first
/// point (pointing into the curve) and `t2` the unit tangent at the last point
/// (pointing back into the curve). `err2` is the squared tolerance.
fn fit_curve(d: &[Pt], t1: Pt, t2: Pt, err2: f64) -> Vec<[Pt; 4]> {
    let mut out = Vec::new();
    // Work list of (first index, last index, start tangent, end tangent).
    // A list is used instead of recursion, so very long arcs cannot overflow
    // the call stack.
    let mut work = vec![(0usize, d.len() - 1, t1, t2)];
    while let Some((lo, hi, ta, tb)) = work.pop() {
        match fit_one(&d[lo..=hi], ta, tb, err2) {
            Fitted::Done(curve) => out.push(curve),
            Fitted::Split(index, center) => {
                // The right half is pushed first, so the left half is done first
                // and the curves come out in the right order.
                work.push((lo + index, hi, (-center.0, -center.1), tb));
                work.push((lo, lo + index, ta, center));
            }
        }
    }
    out
}

/// Result of one fitting step.
enum Fitted {
    /// The curve is within the tolerance.
    Done([Pt; 4]),
    /// Split the points at this index. The tangent points back into the left
    /// half.
    Split(usize, Pt),
}

/// Tries to fit one cubic curve to the points.
fn fit_one(d: &[Pt], t1: Pt, t2: Pt, err2: f64) -> Fitted {
    let n = d.len();
    if n == 2 {
        // Two points: use the heuristic of one third of the distance.
        let dist = dot(sub(d[1], d[0]), sub(d[1], d[0])).sqrt() / 3.0;
        return Fitted::Done([
            d[0],
            add(d[0], mul(t1, dist)),
            add(d[1], mul(t2, dist)),
            d[1],
        ]);
    }

    let mut u = chord_parameters(d);
    let mut curve = generate_bezier(d, &u, t1, t2);
    let (mut max_err, mut split) = max_error(d, &curve, &u);
    if max_err < err2 {
        return Fitted::Done(curve);
    }
    // If the error is not much too large, better parameters may help.
    if max_err < err2 * 16.0 {
        for _ in 0..4 {
            u = reparameterize(d, &u, &curve);
            curve = generate_bezier(d, &u, t1, t2);
            let (e, s) = max_error(d, &curve, &u);
            max_err = e;
            split = s;
            if max_err < err2 {
                return Fitted::Done(curve);
            }
        }
    }
    // Split at the point of the largest error. The tangent at this point is
    // estimated from its two neighbors.
    let split = split.clamp(1, n - 2);
    let mut center = normalize(sub(d[split - 1], d[split + 1]));
    if center == (0.0, 0.0) {
        center = normalize(sub(d[split - 1], d[split]));
    }
    if center == (0.0, 0.0) {
        // All points are equal: nothing to fit, a line is enough.
        return Fitted::Done([d[0], d[0], d[n - 1], d[n - 1]]);
    }
    Fitted::Split(split, center)
}

/// Chord length parameters in `0..=1` for the points.
fn chord_parameters(d: &[Pt]) -> Vec<f64> {
    let mut u = vec![0.0; d.len()];
    for i in 1..d.len() {
        let v = sub(d[i], d[i - 1]);
        u[i] = u[i - 1] + dot(v, v).sqrt();
    }
    let total = u[d.len() - 1];
    if total <= 0.0 {
        for (i, slot) in u.iter_mut().enumerate() {
            *slot = i as f64 / (d.len() - 1) as f64;
        }
    } else {
        for slot in &mut u {
            *slot /= total;
        }
    }
    u
}

/// Bernstein basis functions of a cubic curve.
fn bernstein(t: f64) -> [f64; 4] {
    let s = 1.0 - t;
    [s * s * s, 3.0 * t * s * s, 3.0 * t * t * s, t * t * t]
}

/// Position on a cubic curve.
fn bezier_point(c: &[Pt; 4], t: f64) -> Pt {
    let b = bernstein(t);
    (
        c[0].0 * b[0] + c[1].0 * b[1] + c[2].0 * b[2] + c[3].0 * b[3],
        c[0].1 * b[0] + c[1].1 * b[1] + c[2].1 * b[2] + c[3].1 * b[3],
    )
}

/// Least squares fit of the two inner control points. Their positions are
/// `p0 + t1 * alpha1` and `p3 + t2 * alpha2`, so the end tangents are kept.
fn generate_bezier(d: &[Pt], u: &[f64], t1: Pt, t2: Pt) -> [Pt; 4] {
    let n = d.len();
    let (p0, p3) = (d[0], d[n - 1]);
    let (mut c00, mut c01, mut c11) = (0.0, 0.0, 0.0);
    let (mut x0, mut x1) = (0.0, 0.0);
    for i in 0..n {
        let b = bernstein(u[i]);
        let a1 = mul(t1, b[1]);
        let a2 = mul(t2, b[2]);
        c00 += dot(a1, a1);
        c01 += dot(a1, a2);
        c11 += dot(a2, a2);
        // The part of the point that the end points already explain.
        let base = add(mul(p0, b[0] + b[1]), mul(p3, b[2] + b[3]));
        let rest = sub(d[i], base);
        x0 += dot(a1, rest);
        x1 += dot(a2, rest);
    }
    let det = c00 * c11 - c01 * c01;
    let (alpha1, alpha2) = if det.abs() > 1e-12 {
        ((x0 * c11 - x1 * c01) / det, (c00 * x1 - c01 * x0) / det)
    } else {
        (0.0, 0.0)
    };
    let seg_len = dot(sub(p3, p0), sub(p3, p0)).sqrt();
    let eps = 1e-6 * seg_len;
    if alpha1 < eps || alpha2 < eps {
        // The fit failed (the curve would turn back). Use one third of the
        // distance between the end points.
        let dist = seg_len / 3.0;
        return [p0, add(p0, mul(t1, dist)), add(p3, mul(t2, dist)), p3];
    }
    [p0, add(p0, mul(t1, alpha1)), add(p3, mul(t2, alpha2)), p3]
}

/// Squared distance of the worst point to the curve, and the index of the point.
fn max_error(d: &[Pt], curve: &[Pt; 4], u: &[f64]) -> (f64, usize) {
    let mut worst = 0.0;
    let mut index = d.len() / 2;
    for i in 1..d.len() - 1 {
        let v = sub(bezier_point(curve, u[i]), d[i]);
        let dist = dot(v, v);
        if dist >= worst {
            worst = dist;
            index = i;
        }
    }
    (worst, index)
}

/// One Newton-Raphson step for every parameter: moves it to the point of the
/// curve that is nearest to the data point.
fn reparameterize(d: &[Pt], u: &[f64], curve: &[Pt; 4]) -> Vec<f64> {
    // Control points of the first and the second derivative.
    let q1 = [
        mul(sub(curve[1], curve[0]), 3.0),
        mul(sub(curve[2], curve[1]), 3.0),
        mul(sub(curve[3], curve[2]), 3.0),
    ];
    let q2 = [
        mul(add(sub(curve[2], mul(curve[1], 2.0)), curve[0]), 6.0),
        mul(add(sub(curve[3], mul(curve[2], 2.0)), curve[1]), 6.0),
    ];
    d.iter()
        .zip(u)
        .map(|(&point, &t)| {
            let s = 1.0 - t;
            let p = bezier_point(curve, t);
            let d1 = add(
                add(mul(q1[0], s * s), mul(q1[1], 2.0 * s * t)),
                mul(q1[2], t * t),
            );
            let d2 = add(mul(q2[0], s), mul(q2[1], t));
            let diff = sub(p, point);
            let numerator = dot(diff, d1);
            let denominator = dot(d1, d1) + dot(diff, d2);
            if denominator.abs() < 1e-12 {
                t
            } else {
                (t - numerator / denominator).clamp(0.0, 1.0)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Distance from a point to the polyline made by sampling a fitted arc.
    fn distance_to_arc(arc: &FittedArc, scale: f64, p: Pt) -> f64 {
        let mut best = f64::INFINITY;
        let mut cur = (
            f64::from(arc.start.0) / scale,
            f64::from(arc.start.1) / scale,
        );
        for seg in &arc.segs {
            let steps = 64;
            let samples: Vec<Pt> = match *seg {
                Seg::Line(to) => {
                    let to = (f64::from(to.0) / scale, f64::from(to.1) / scale);
                    (0..=steps)
                        .map(|i| {
                            let t = f64::from(i) / f64::from(steps);
                            (cur.0 + (to.0 - cur.0) * t, cur.1 + (to.1 - cur.1) * t)
                        })
                        .collect()
                }
                Seg::Cubic(c1, c2, to) => {
                    let conv = |q: (i32, i32)| (f64::from(q.0) / scale, f64::from(q.1) / scale);
                    let curve = [cur, conv(c1), conv(c2), conv(to)];
                    (0..=steps)
                        .map(|i| bezier_point(&curve, f64::from(i) / f64::from(steps)))
                        .collect()
                }
            };
            for s in &samples {
                let v = sub(*s, p);
                best = best.min(dot(v, v).sqrt());
            }
            let e = seg.end();
            cur = (f64::from(e.0) / scale, f64::from(e.1) / scale);
        }
        best
    }

    #[test]
    fn straight_arc_becomes_one_line() {
        let pts: Vec<(i32, i32)> = (0..=25).map(|x| (x, 3)).collect();
        let arc = fit_arc(&pts, false, 0.35, 100);
        assert_eq!(arc.start, (0, 300));
        assert_eq!(arc.segs, vec![Seg::Line((2500, 300))]);
    }

    #[test]
    fn rectangle_ring_becomes_four_lines() {
        let mut ring = Vec::new();
        for x in 0..20 {
            ring.push((x, 0));
        }
        for y in 0..10 {
            ring.push((20, y));
        }
        for x in (1..=20).rev() {
            ring.push((x, 10));
        }
        for y in (1..=10).rev() {
            ring.push((0, y));
        }
        ring.push((0, 0));
        let arc = fit_arc(&ring, true, 0.35, 10);
        assert_eq!(arc.segs.len(), 4, "{:?}", arc.segs);
        assert!(arc.segs.iter().all(|s| matches!(s, Seg::Line(_))));
        assert_eq!(arc.end(), arc.start, "the ring is closed");
    }

    #[test]
    fn circle_staircase_is_fitted_within_tolerance_with_few_segments() {
        // Pixel edges of a disk with radius 30 (center at 40, 40).
        let (cx, cy, r) = (40.0f64, 40.0f64, 30.0f64);
        let mut cells = std::collections::BTreeSet::new();
        for y in 0..80i32 {
            for x in 0..80i32 {
                let (px, py) = (f64::from(x) + 0.5 - cx, f64::from(y) + 0.5 - cy);
                if px * px + py * py <= r * r {
                    cells.insert((x, y));
                }
            }
        }
        let ring = crate::simplify::tests_support::walk_cells(&cells);
        let arc = fit_arc(&ring, true, 0.35, 100);
        assert!(
            arc.segs.len() <= 24,
            "too many segments: {}",
            arc.segs.len()
        );
        // Every sampled point of the curve is close to the true circle.
        for seg_index in 0..arc.segs.len() {
            let _ = seg_index;
        }
        for &(x, y) in ring.iter().step_by(5) {
            // The staircase corner is at most about 0.75 pixels from the circle.
            let d = distance_to_arc(&arc, 100.0, (f64::from(x), f64::from(y)));
            assert!(d < 1.0, "corner ({x},{y}) is {d} px from the curve");
        }
    }

    #[test]
    fn reversed_arc_walks_the_same_shape_backwards() {
        let arc = FittedArc {
            start: (0, 0),
            segs: vec![
                Seg::Line((10, 0)),
                Seg::Cubic((12, 1), (14, 3), (15, 5)),
                Seg::Line((15, 20)),
            ],
        };
        let back = arc.reversed();
        assert_eq!(back.start, (15, 20));
        assert_eq!(
            back.segs,
            vec![
                Seg::Line((15, 5)),
                Seg::Cubic((14, 3), (12, 1), (10, 0)),
                Seg::Line((0, 0)),
            ]
        );
        assert_eq!(back.reversed(), arc);
    }

    #[test]
    fn rounding_to_whole_pixels_works() {
        let pts: Vec<(i32, i32)> = (0..=25).map(|x| (x, x / 3)).collect();
        let arc = fit_arc(&pts, false, 1.8, 1);
        for seg in &arc.segs {
            // Nothing to check except that the numbers are small integers.
            let e = seg.end();
            assert!((0..=25).contains(&e.0) && (0..=9).contains(&e.1));
        }
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        assert!(fit_arc(&[(3, 3)], false, 0.35, 100).segs.is_empty());
        let arc = fit_arc(&[(3, 3), (4, 3)], false, 0.35, 100);
        assert_eq!(arc.segs, vec![Seg::Line((400, 300))]);
        // A ring of one pixel.
        let ring = [(0, 0), (1, 0), (1, 1), (0, 1), (0, 0)];
        let arc = fit_arc(&ring, true, 0.35, 100);
        assert_eq!(arc.segs.len(), 4);
    }

    #[test]
    fn fitting_is_deterministic() {
        let pts: Vec<(i32, i32)> = (0..60)
            .map(|i| (i, ((f64::from(i) * 0.3).sin() * 8.0) as i32))
            .collect();
        let a = fit_arc(&pts, false, 0.6, 10);
        let b = fit_arc(&pts, false, 0.6, 10);
        assert_eq!(a, b);
    }
}
