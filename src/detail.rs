//! Table of internal parameters for every detail level (`--detail 1..100`).
//!
//! The table has five anchor rows (detail 1, 25, 50, 75, 100). Levels between
//! two anchors are found by linear interpolation. Integer parameters are
//! rounded to the nearest whole number.
//!
//! The numbers come from the requirements document (section 8). They were
//! checked with the quality tests in `tests/quality.rs`. Two values differ from
//! the first draft of the table, because measurements showed a better result:
//!
//! * At detail 100 the coordinates have one decimal place, not two. The image
//!   quality (SSIM) is the same, and the files are about 10 percent smaller.
//! * The extra column `color_merge` is new. It joins palette colors that
//!   people cannot tell apart.

/// Parameters of the conversion that depend on the detail level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// Maximum number of colors in the palette (including alpha variants).
    pub max_colors: u32,
    /// Regions with fewer pixels than this are merged into a neighbor.
    pub min_area: u32,
    /// Maximum distance (in pixels) between a fitted curve and the smoothed
    /// boundary of the region.
    pub tolerance: f64,
    /// Number of decimal places in the coordinates of the SVG.
    pub precision: u32,
    /// Number of levels of `fill-opacity` (the highest level is fully opaque).
    pub alpha_levels: u32,
    /// Palette colors that are closer than this (distance in the Oklab color
    /// space) are merged. This removes shades that people cannot see.
    pub color_merge: f64,
}

/// One anchor row of the table.
struct Anchor {
    detail: u32,
    max_colors: f64,
    min_area: f64,
    tolerance: f64,
    precision: f64,
    alpha_levels: f64,
    color_merge: f64,
}

/// The anchor rows, sorted by detail.
const ANCHORS: [Anchor; 5] = [
    Anchor {
        detail: 1,
        max_colors: 4.0,
        min_area: 128.0,
        tolerance: 3.5,
        precision: 0.0,
        alpha_levels: 2.0,
        color_merge: 0.12,
    },
    Anchor {
        detail: 25,
        max_colors: 12.0,
        min_area: 48.0,
        tolerance: 2.4,
        precision: 0.0,
        alpha_levels: 2.0,
        color_merge: 0.075,
    },
    Anchor {
        detail: 50,
        max_colors: 32.0,
        min_area: 16.0,
        tolerance: 1.4,
        precision: 1.0,
        alpha_levels: 4.0,
        color_merge: 0.045,
    },
    Anchor {
        detail: 75,
        max_colors: 64.0,
        min_area: 6.0,
        tolerance: 0.9,
        precision: 1.0,
        alpha_levels: 8.0,
        color_merge: 0.025,
    },
    Anchor {
        detail: 100,
        max_colors: 256.0,
        min_area: 2.0,
        tolerance: 0.6,
        precision: 1.0,
        alpha_levels: 16.0,
        color_merge: 0.012,
    },
];

/// Returns the parameters for a detail level. Values outside 1..=100 are
/// clamped to that range.
pub fn params_for(detail: u32) -> Params {
    let detail = detail.clamp(1, 100);
    // Find the two anchors that surround the level.
    let mut upper = 1;
    while upper < ANCHORS.len() - 1 && ANCHORS[upper].detail < detail {
        upper += 1;
    }
    let (lo, hi) = (&ANCHORS[upper - 1], &ANCHORS[upper]);
    let t = f64::from(detail - lo.detail) / f64::from(hi.detail - lo.detail);
    let mix = |a: f64, b: f64| a + (b - a) * t;
    let params = Params {
        max_colors: round_u32(mix(lo.max_colors, hi.max_colors)),
        min_area: round_u32(mix(lo.min_area, hi.min_area)),
        tolerance: mix(lo.tolerance, hi.tolerance),
        precision: round_u32(mix(lo.precision, hi.precision)),
        alpha_levels: round_u32(mix(lo.alpha_levels, hi.alpha_levels)),
        color_merge: mix(lo.color_merge, hi.color_merge),
    };
    // Keep the numbers inside their legal ranges even after rounding.
    Params {
        max_colors: params.max_colors.clamp(2, 256),
        min_area: params.min_area.max(1),
        precision: params.precision.min(3),
        alpha_levels: params.alpha_levels.max(2),
        ..params
    }
}

/// Rounds a non-negative number to the nearest integer. Halves go up.
fn round_u32(v: f64) -> u32 {
    (v + 0.5).floor().max(0.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_return_the_table_values() {
        let p = params_for(100);
        assert_eq!(
            (p.max_colors, p.min_area, p.precision, p.alpha_levels),
            (256, 2, 1, 16)
        );
        assert!((p.tolerance - 0.6).abs() < 1e-12);
        let p = params_for(50);
        assert_eq!(
            (p.max_colors, p.min_area, p.precision, p.alpha_levels),
            (32, 16, 1, 4)
        );
        let p = params_for(1);
        assert_eq!(
            (p.max_colors, p.min_area, p.precision, p.alpha_levels),
            (4, 128, 0, 2)
        );
    }

    #[test]
    fn intermediate_values_are_interpolated() {
        // Halfway between 50 (32 colors) and 75 (64 colors) is about 48 colors.
        let p = params_for(62);
        assert!(p.max_colors > 32 && p.max_colors < 64);
        assert!(p.tolerance < 1.4 && p.tolerance > 0.9);
    }

    #[test]
    fn parameters_change_monotonically_with_detail() {
        let mut previous = params_for(1);
        for detail in 2..=100 {
            let p = params_for(detail);
            assert!(p.max_colors >= previous.max_colors, "colors at {detail}");
            assert!(p.min_area <= previous.min_area, "area at {detail}");
            assert!(
                p.tolerance <= previous.tolerance + 1e-12,
                "tolerance at {detail}"
            );
            assert!(p.precision >= previous.precision, "precision at {detail}");
            assert!(p.alpha_levels >= previous.alpha_levels, "alpha at {detail}");
            assert!(
                p.color_merge <= previous.color_merge + 1e-12,
                "merge at {detail}"
            );
            previous = p;
        }
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        assert_eq!(params_for(0), params_for(1));
        assert_eq!(params_for(500), params_for(100));
    }
}
