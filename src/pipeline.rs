//! The conversion pipeline: decoded image in, SVG text out.
//!
//! ```text
//! RGBA pixels -> quantize (transparency, palette) -> segment (regions, noise)
//!   -> trace (shared boundary arcs) -> fit (lines and curves)
//!   -> layers (drawing order, underlay) -> SVG text
//! ```

use std::time::{Duration, Instant};

use crate::detail::{params_for, Params};
use crate::png::Image;
use crate::quantize::{count_distinct, quantize, remove_background};
use crate::segment::segment;
use crate::svg::write_svg;
use crate::trace::trace;
use crate::unblend::remove_antialiasing;
use crate::vectorize::vectorize;

/// An image with more distinct colors than this has anti-aliased edges, and the
/// blend pixels are removed before the quantization.
const ANTIALIAS_MIN_COLORS: usize = 24;

/// Settings of the conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConvertOptions {
    /// Detail level 1..=100.
    pub detail: u32,
    /// Explicit maximum number of colors (overrides the detail level).
    pub colors: Option<u32>,
    /// Explicit number of decimal places (overrides the detail level).
    pub precision: Option<u32>,
    /// Make the border color transparent.
    pub remove_background: bool,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        ConvertOptions {
            detail: 100,
            colors: None,
            precision: None,
            remove_background: false,
        }
    }
}

/// Numbers about a finished conversion (printed by `--verbose`).
#[derive(Debug, Clone, Default)]
pub struct Stats {
    /// Image width in pixels.
    pub width: usize,
    /// Image height in pixels.
    pub height: usize,
    /// Number of anti-aliasing pixels that were replaced.
    pub blend_pixels: usize,
    /// Number of colors in the palette (with alpha levels).
    pub colors: usize,
    /// `true` if the colors were not reduced.
    pub exact_colors: bool,
    /// Number of regions.
    pub regions: usize,
    /// Number of shared boundary arcs.
    pub arcs: usize,
    /// Number of closed outlines in the SVG.
    pub contours: usize,
    /// Number of holes that were filled (underlay).
    pub filled_holes: usize,
    /// Number of `<path>` elements.
    pub paths: usize,
    /// Size of the SVG text in bytes.
    pub svg_bytes: usize,
    /// Time of every step.
    pub timings: Vec<(&'static str, Duration)>,
}

impl Stats {
    /// Multi-line text for the `--verbose` option.
    pub fn report(&self) -> String {
        let mut text = format!(
            "image: {}x{} px\nblend pixels removed: {}\ncolors: {}{}\nregions: {}\ncontours: {} (arcs: {}, filled holes: {})\npaths: {}\nsvg size: {} bytes\n",
            self.width,
            self.height,
            self.blend_pixels,
            self.colors,
            if self.exact_colors { " (exact)" } else { " (reduced)" },
            self.regions,
            self.contours,
            self.arcs,
            self.filled_holes,
            self.paths,
            self.svg_bytes,
        );
        let total: Duration = self.timings.iter().map(|t| t.1).sum();
        for (name, time) in &self.timings {
            text.push_str(&format!(
                "time {name}: {:.1} ms\n",
                time.as_secs_f64() * 1000.0
            ));
        }
        text.push_str(&format!(
            "time total: {:.1} ms\n",
            total.as_secs_f64() * 1000.0
        ));
        text
    }
}

/// Applies `--colors` and `--precision` on top of the detail table.
pub fn effective_params(options: &ConvertOptions) -> Params {
    let mut params = params_for(options.detail);
    if let Some(colors) = options.colors {
        params.max_colors = colors.clamp(2, 256);
    }
    if let Some(precision) = options.precision {
        params.precision = precision.min(3);
    }
    params
}

/// Converts an image to SVG text.
pub fn convert(image: &Image, options: &ConvertOptions) -> (String, Stats) {
    let params = effective_params(options);
    let mut stats = Stats {
        width: image.width,
        height: image.height,
        ..Stats::default()
    };
    let mut clock = Instant::now();
    let mut lap = |name: &'static str, stats: &mut Stats| {
        stats.timings.push((name, clock.elapsed()));
        clock = Instant::now();
    };

    // Pictures that were drawn by a program have soft (anti-aliased) edges, and
    // such pictures have many different shades. Images with a small hand made
    // palette (pixel art) are left as they are.
    let smooth_edges = count_distinct(image, 255) > ANTIALIAS_MIN_COLORS;
    let cleaned;
    let source = if smooth_edges {
        let (result, count) = remove_antialiasing(image);
        stats.blend_pixels = count;
        cleaned = result;
        &cleaned
    } else {
        image
    };
    let mut q = quantize(
        source,
        params.max_colors,
        params.alpha_levels,
        params.color_merge,
    );
    if options.remove_background {
        remove_background(&mut q);
    }
    stats.colors = q.palette.len();
    stats.exact_colors = q.exact;
    lap("quantize", &mut stats);

    // Pictures with a small hand made palette (pixel art) keep every pixel at the
    // highest detail, even a single pixel of its own color. Other pictures use
    // the minimum region area of the detail table.
    let min_area = if !smooth_edges && options.detail >= 100 && options.colors.is_none() {
        1
    } else {
        params.min_area
    };
    let regions = segment(&mut q, min_area);
    stats.regions = regions.count();
    lap("segment", &mut stats);

    let topology = trace(&regions);
    lap("trace", &mut stats);

    // Pictures with a small hand made palette (pixel art) keep the exact pixel
    // edges. They are not smoothed, so their outlines are polygons with
    // whole-number coordinates (this is also the shortest way to write them).
    let mut fit_params = params;
    if !smooth_edges {
        fit_params.tolerance = 0.0;
    }
    let vector = vectorize(&q, &regions, &topology, &fit_params, params.precision);
    stats.arcs = vector.stats.arcs;
    stats.contours = vector.stats.contours;
    stats.filled_holes = vector.stats.filled_holes;
    stats.paths = vector.layers.len();
    lap("fit", &mut stats);

    let svg = write_svg(image.width, image.height, &vector);
    stats.svg_bytes = svg.len();
    lap("svg", &mut stats);
    (svg, stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: usize, height: usize, rgba: [u8; 4]) -> Image {
        Image {
            width,
            height,
            rgba: rgba
                .iter()
                .copied()
                .cycle()
                .take(width * height * 4)
                .collect(),
        }
    }

    #[test]
    fn solid_color_image_is_one_rectangle() {
        let (svg, stats) = convert(&solid(10, 6, [255, 0, 0, 255]), &ConvertOptions::default());
        assert_eq!(
            svg,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 10 6\" width=\"10\" height=\"6\"><path fill=\"#f00\" d=\"M0 0V6H10V0z\"/></svg>"
        );
        assert_eq!((stats.paths, stats.colors, stats.regions), (1, 1, 1));
    }

    #[test]
    fn fully_transparent_image_gives_an_empty_svg() {
        let (svg, _) = convert(&solid(4, 4, [10, 20, 30, 0]), &ConvertOptions::default());
        assert!(svg.ends_with("/>") && !svg.contains("<path"), "{svg}");
    }

    #[test]
    fn one_by_one_image_works() {
        let (svg, _) = convert(&solid(1, 1, [0, 0, 0, 255]), &ConvertOptions::default());
        assert!(svg.contains("<path d=\"M0 0V1H1V0z\"/>"), "{svg}");
    }

    #[test]
    fn explicit_overrides_win() {
        let params = effective_params(&ConvertOptions {
            detail: 100,
            colors: Some(7),
            precision: Some(0),
            remove_background: false,
        });
        assert_eq!((params.max_colors, params.precision), (7, 0));
    }

    #[test]
    fn report_contains_the_main_numbers() {
        let (_, stats) = convert(&solid(3, 3, [1, 2, 3, 255]), &ConvertOptions::default());
        let text = stats.report();
        for needle in [
            "image: 3x3",
            "colors: 1",
            "regions: 1",
            "svg size:",
            "time total",
        ] {
            assert!(text.contains(needle), "{needle} in {text}");
        }
    }
}
