//! Boundary tracing on the grid of pixel corners.
//!
//! The boundaries between regions follow the edges of pixels, not the pixel
//! centers (requirement ALG-05). Every unit edge between two pixels of
//! different regions belongs to exactly one *arc*. An arc is a chain of unit
//! edges between two *junctions*. A junction is a grid point where three or
//! four edges meet. An isolated ring that touches no junction is a *closed arc*.
//!
//! Because an arc is shared by the two regions on its sides, it is simplified
//! and fitted with curves **once**. Both regions then use the same geometry, so
//! there are no gaps and no overlaps between them (requirement ALG-06).
//!
//! A region *contour* is a closed sequence of arcs, each used forward or
//! backward, with the region always on the left side. The contour order is the
//! outer boundary first, and the holes after it. A contour with negative area
//! is an outer boundary and one with positive area is a hole (the y axis points
//! down).

use std::collections::HashMap;

use crate::segment::{Regions, OUTSIDE};

/// The region id that is used for the space outside of the image.
pub const NONE: u32 = OUTSIDE;

/// Directions: 0 = east (+x), 1 = south (+y), 2 = west, 3 = north.
const DX: [i32; 4] = [1, 0, -1, 0];
/// See [`DX`].
const DY: [i32; 4] = [0, 1, 0, -1];

/// A chain of boundary edges between two regions.
#[derive(Debug, Clone)]
pub struct Arc {
    /// Grid points of the chain. Neighbor points differ by one unit step.
    /// For a closed arc the first and the last point are equal.
    pub points: Vec<(i32, i32)>,
    /// Region on the left side when walking along `points` (or [`NONE`]).
    pub left: u32,
    /// Region on the right side when walking along `points` (or [`NONE`]).
    pub right: u32,
    /// `true` for a ring without junctions.
    pub closed: bool,
}

/// An arc used in one of its two directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirArc {
    /// Index of the arc.
    pub arc: u32,
    /// `true` if the arc is walked from its last point to its first point.
    pub reversed: bool,
}

/// A closed boundary of a region.
#[derive(Debug, Clone)]
pub struct Contour {
    /// The region that lies on the left side of the contour.
    pub region: u32,
    /// The arcs of the contour in walking order.
    pub arcs: Vec<DirArc>,
    /// `true` if the contour is a hole of the region (not its outer boundary).
    pub is_hole: bool,
}

/// All boundaries of an image.
#[derive(Debug, Clone, Default)]
pub struct Topology {
    /// The shared arcs.
    pub arcs: Vec<Arc>,
    /// The contours of all regions that are not outside of the image.
    pub contours: Vec<Contour>,
}

impl Topology {
    /// Lists the neighbor regions of every region (without duplicates, sorted).
    pub fn adjacency(&self, region_count: usize) -> Vec<Vec<u32>> {
        let mut adj: Vec<Vec<u32>> = vec![Vec::new(); region_count];
        for arc in &self.arcs {
            if arc.left != NONE && arc.right != NONE {
                adj[arc.left as usize].push(arc.right);
                adj[arc.right as usize].push(arc.left);
            }
        }
        for list in &mut adj {
            list.sort_unstable();
            list.dedup();
        }
        adj
    }
}

/// A set of bits that is used to mark visited edges.
struct BitSet(Vec<u64>);

impl BitSet {
    fn new(len: usize) -> BitSet {
        BitSet(vec![0; len.div_ceil(64)])
    }
    fn get(&self, i: usize) -> bool {
        self.0[i / 64] >> (i % 64) & 1 == 1
    }
    fn set(&mut self, i: usize) {
        self.0[i / 64] |= 1 << (i % 64);
    }
}

/// Helper that answers questions about pixels and edges of the region map.
struct Grid<'a> {
    ids: &'a [u32],
    w: i32,
    h: i32,
}

impl Grid<'_> {
    /// Region id of a pixel, or [`NONE`] outside of the image.
    fn px(&self, x: i32, y: i32) -> u32 {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            NONE
        } else {
            self.ids[(y * self.w + x) as usize]
        }
    }

    /// `true` if there is a boundary edge that starts at grid point `(x, y)`
    /// and goes in direction `d`.
    fn edge(&self, x: i32, y: i32, d: usize) -> bool {
        let (l, r) = self.sides(x, y, d);
        l != r
    }

    /// Regions on the left and on the right of the edge that starts at
    /// `(x, y)` in direction `d`.
    fn sides(&self, x: i32, y: i32, d: usize) -> (u32, u32) {
        match d {
            0 => (self.px(x, y - 1), self.px(x, y)),
            1 => (self.px(x, y), self.px(x - 1, y)),
            2 => (self.px(x - 1, y), self.px(x - 1, y - 1)),
            _ => (self.px(x - 1, y - 1), self.px(x, y - 1)),
        }
    }

    /// Number of boundary edges at a grid point.
    fn degree(&self, x: i32, y: i32) -> usize {
        (0..4).filter(|&d| self.edge(x, y, d)).count()
    }

    /// Pixel in the quadrant of the grid point `(x, y)` given by the signs.
    fn quadrant(&self, x: i32, y: i32, sx: i32, sy: i32) -> u32 {
        self.px(
            if sx > 0 { x } else { x - 1 },
            if sy > 0 { y } else { y - 1 },
        )
    }

    /// Index that identifies an undirected edge (for the visited marks).
    fn edge_index(&self, x: i32, y: i32, d: usize) -> usize {
        let w = self.w as usize;
        match d {
            // Horizontal edges first: row y, column x.
            0 => y as usize * w + x as usize,
            2 => y as usize * w + (x - 1) as usize,
            // Vertical edges after them: row y, column x (w + 1 columns).
            1 => (self.h as usize + 1) * w + y as usize * (w + 1) + x as usize,
            _ => (self.h as usize + 1) * w + (y - 1) as usize * (w + 1) + x as usize,
        }
    }
}

/// Finds all arcs and contours of the regions.
pub fn trace(regions: &Regions) -> Topology {
    let (w, h) = (regions.width as i32, regions.height as i32);
    let grid = Grid {
        ids: &regions.ids,
        w,
        h,
    };
    let edge_count = (h as usize + 1) * w as usize + h as usize * (w as usize + 1);
    let mut visited = BitSet::new(edge_count);
    let mut arcs: Vec<Arc> = Vec::new();

    // Phase 1: arcs that start at junctions.
    for y in 0..=h {
        for x in 0..=w {
            if grid.degree(x, y) < 3 {
                continue;
            }
            for d in 0..4 {
                if grid.edge(x, y, d) && !visited.get(grid.edge_index(x, y, d)) {
                    arcs.push(walk(&grid, &mut visited, x, y, d));
                }
            }
        }
    }
    // Phase 2: rings that have no junction. Every edge that is still unvisited
    // belongs to such a ring.
    for y in 0..=h {
        for x in 0..=w {
            for d in [0usize, 1] {
                if grid.edge(x, y, d) && !visited.get(grid.edge_index(x, y, d)) {
                    arcs.push(walk(&grid, &mut visited, x, y, d));
                }
            }
        }
    }

    let contours = build_contours(&grid, &arcs);
    Topology { arcs, contours }
}

/// Follows the boundary from a grid point until the next junction (or until
/// the starting point is reached again for a ring).
fn walk(
    grid: &Grid<'_>,
    visited: &mut BitSet,
    start_x: i32,
    start_y: i32,
    first_dir: usize,
) -> Arc {
    let (left, right) = grid.sides(start_x, start_y, first_dir);
    let mut points = vec![(start_x, start_y)];
    let (mut x, mut y, mut d) = (start_x, start_y, first_dir);
    loop {
        visited.set(grid.edge_index(x, y, d));
        x += DX[d];
        y += DY[d];
        points.push((x, y));
        if grid.degree(x, y) != 2 {
            break; // Reached a junction.
        }
        if x == start_x && y == start_y {
            break; // A ring is closed (the start point has two edges only).
        }
        // A grid point with two edges: the way continues straight or turns.
        // The reverse direction is the edge we came from, so it is skipped.
        let next = [(d + 3) % 4, d, (d + 1) % 4]
            .into_iter()
            .find(|&nd| grid.edge(x, y, nd));
        match next {
            Some(nd) => d = nd,
            None => break, // Cannot happen. Stop to be safe.
        }
    }
    let closed = points.first() == points.last() && grid.degree(start_x, start_y) == 2;
    Arc {
        points,
        left,
        right,
        closed,
    }
}

/// Direction number of a unit step.
fn dir_of(a: (i32, i32), b: (i32, i32)) -> usize {
    match (b.0 - a.0, b.1 - a.1) {
        (1, 0) => 0,
        (0, 1) => 1,
        (-1, 0) => 2,
        _ => 3,
    }
}

/// Twice the signed area of a closed polyline (shoelace formula).
fn area2(points: &[(i32, i32)]) -> i64 {
    points
        .windows(2)
        .map(|p| i64::from(p[0].0) * i64::from(p[1].1) - i64::from(p[1].0) * i64::from(p[0].1))
        .sum()
}

/// Connects the arcs to closed contours of the regions.
fn build_contours(grid: &Grid<'_>, arcs: &[Arc]) -> Vec<Contour> {
    // Properties of the directed arcs: index = arc * 2 + reversed.
    // end point, last direction and the region on the left side.
    struct Info {
        end: (i32, i32),
        last_dir: usize,
        left: u32,
    }
    let mut infos: Vec<Info> = Vec::with_capacity(arcs.len() * 2);
    let mut starts: HashMap<u64, u32> = HashMap::new();
    let key = |p: (i32, i32), d: usize| -> u64 {
        ((p.1 as u64) * (grid.w as u64 + 1) + p.0 as u64) * 4 + d as u64
    };
    for (i, arc) in arcs.iter().enumerate() {
        let n = arc.points.len();
        let first = dir_of(arc.points[0], arc.points[1]);
        let last = dir_of(arc.points[n - 2], arc.points[n - 1]);
        infos.push(Info {
            end: arc.points[n - 1],
            last_dir: last,
            left: arc.left,
        });
        infos.push(Info {
            end: arc.points[0],
            last_dir: (first + 2) % 4,
            left: arc.right,
        });
        if !arc.closed {
            starts.insert(key(arc.points[0], first), (i * 2) as u32);
            starts.insert(key(arc.points[n - 1], (last + 2) % 4), (i * 2 + 1) as u32);
        }
    }

    let mut used = vec![false; infos.len()];
    let mut contours = Vec::new();
    for first in 0..infos.len() {
        if used[first] || infos[first].left == NONE {
            continue;
        }
        let region = infos[first].left;
        let mut chain = vec![first];
        used[first] = true;
        let mut current = first;
        loop {
            if arcs[current / 2].closed {
                break; // A ring is a whole contour.
            }
            let info = &infos[current];
            // Choose the next edge so that the region stays on the left side.
            let d = info.last_dir;
            let (vx, vy) = info.end;
            let (lx, ly) = (DX[d] + DX[(d + 3) % 4], DY[d] + DY[(d + 3) % 4]);
            let (rx, ry) = (DX[d] + DX[(d + 1) % 4], DY[d] + DY[(d + 1) % 4]);
            let ahead_left = grid.quadrant(vx, vy, lx, ly);
            let ahead_right = grid.quadrant(vx, vy, rx, ry);
            let next_dir = if ahead_left != region {
                (d + 3) % 4
            } else if ahead_right != region {
                d
            } else {
                (d + 1) % 4
            };
            let next = match starts.get(&key((vx, vy), next_dir)) {
                Some(&n) => n as usize,
                None => break, // Cannot happen. Stop to be safe.
            };
            if next == first {
                break;
            }
            if used[next] {
                break; // Cannot happen. Stop to be safe.
            }
            used[next] = true;
            chain.push(next);
            current = next;
        }

        // The sign of the area tells apart outer boundaries and holes.
        let mut total = 0i64;
        for &da in &chain {
            let pts = &arcs[da / 2].points;
            let a = area2(pts);
            total += if da % 2 == 1 { -a } else { a };
        }
        contours.push(Contour {
            region,
            arcs: chain
                .into_iter()
                .map(|da| DirArc {
                    arc: (da / 2) as u32,
                    reversed: da % 2 == 1,
                })
                .collect(),
            is_hole: total > 0,
        });
    }
    contours
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::label_components;

    /// Makes regions from rows of digits (equal digits that touch are one region).
    fn regions(rows: &[&str]) -> Regions {
        let labels: Vec<u16> = rows
            .iter()
            .flat_map(|r| r.chars().map(|c| c.to_digit(10).unwrap() as u16))
            .collect();
        label_components(&labels, rows[0].len(), rows.len())
    }

    /// Checks the main invariants of a topology.
    fn check(rows: &[&str]) -> (Regions, Topology) {
        let r = regions(rows);
        let t = trace(&r);
        // 1. Every boundary edge is in exactly one arc: count the edges.
        let (w, h) = (r.width as i32, r.height as i32);
        let grid = Grid { ids: &r.ids, w, h };
        let mut expected = 0;
        for y in 0..=h {
            for x in 0..=w {
                expected += usize::from(grid.edge(x, y, 0)) + usize::from(grid.edge(x, y, 1));
            }
        }
        let in_arcs: usize = t.arcs.iter().map(|a| a.points.len() - 1).sum();
        assert_eq!(in_arcs, expected, "edges in arcs");
        // 2. The signed area of the contours of a region gives its area.
        let mut sum = vec![0i64; r.count()];
        for c in &t.contours {
            let mut a = 0;
            for da in &c.arcs {
                let v = area2(&t.arcs[da.arc as usize].points);
                a += if da.reversed { -v } else { v };
            }
            sum[c.region as usize] += a;
            assert_eq!(a > 0, c.is_hole);
        }
        for (i, s) in sum.iter().enumerate() {
            assert_eq!(-s, 2 * i64::from(r.area[i]), "area of region {i}");
        }
        // 3. Contours are connected: the end of an arc is the start of the next.
        for c in &t.contours {
            for k in 0..c.arcs.len() {
                let a = c.arcs[k];
                let b = c.arcs[(k + 1) % c.arcs.len()];
                let end = |d: DirArc| {
                    let p = &t.arcs[d.arc as usize].points;
                    if d.reversed {
                        p[0]
                    } else {
                        p[p.len() - 1]
                    }
                };
                let start = |d: DirArc| {
                    let p = &t.arcs[d.arc as usize].points;
                    if d.reversed {
                        p[p.len() - 1]
                    } else {
                        p[0]
                    }
                };
                assert_eq!(end(a), start(b));
            }
        }
        (r, t)
    }

    #[test]
    fn single_pixel_is_one_closed_contour() {
        let (_, t) = check(&["1"]);
        assert_eq!(t.arcs.len(), 1);
        assert!(t.arcs[0].closed);
        assert_eq!(t.arcs[0].points.len(), 5);
        assert_eq!(t.contours.len(), 1);
        assert!(!t.contours[0].is_hole);
    }

    #[test]
    fn square_with_a_hole_has_two_contours() {
        let (r, t) = check(&["111", "121", "111"]);
        assert_eq!(r.count(), 2);
        let ring_contours: Vec<_> = t.contours.iter().filter(|c| c.region == 0).collect();
        assert_eq!(ring_contours.len(), 2);
        assert_eq!(ring_contours.iter().filter(|c| c.is_hole).count(), 1);
    }

    #[test]
    fn neighbor_regions_share_the_same_arc() {
        let (_, t) = check(&["12"]);
        // Arcs: outer ring of region 0 is made of one shared arc and the
        // outside arc, junctions are at the two ends of the shared arc.
        let shared: Vec<_> = t
            .arcs
            .iter()
            .filter(|a| a.left != NONE && a.right != NONE)
            .collect();
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].points.len(), 2);
    }

    #[test]
    fn three_regions_meet_in_a_junction() {
        let (_, t) = check(&["12", "13"]);
        assert!(t.arcs.len() >= 3);
    }

    #[test]
    fn checkerboard_pixels_are_not_joined_diagonally() {
        let (r, _) = check(&["12", "21"]);
        assert_eq!(r.count(), 4);
    }

    #[test]
    fn same_region_touching_itself_at_a_corner_is_handled() {
        // The 1s are connected through the top row, so they are one region
        // that touches itself at the center corner.
        check(&["111", "121", "212"]);
        check(&["1112", "1212", "1222"]);
    }

    #[test]
    fn complex_shapes_keep_all_invariants() {
        check(&["1111111", "1222221", "1233321", "1222221", "1111111"]);
        check(&["1212121", "2121212", "1212121"]);
        check(&["11223344", "11223344", "55667788", "55667788"]);
        check(&["1.1".replace('.', "2").as_str(), "212", "121"]);
    }

    #[test]
    fn pseudo_random_images_keep_all_invariants() {
        let mut seed = 7u32;
        for size in [3usize, 5, 8, 13] {
            for _ in 0..30 {
                let mut rows = Vec::new();
                for _ in 0..size {
                    let mut row = String::new();
                    for _ in 0..size {
                        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        row.push(char::from(b'0' + ((seed >> 24) % 3) as u8));
                    }
                    rows.push(row);
                }
                let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
                check(&refs);
            }
        }
    }

    #[test]
    fn adjacency_lists_neighbors() {
        let r = regions(&["123"]);
        let t = trace(&r);
        let adj = t.adjacency(r.count());
        assert_eq!(adj[0], vec![1]);
        assert_eq!(adj[1], vec![0, 2]);
        assert_eq!(adj[2], vec![1]);
    }
}
