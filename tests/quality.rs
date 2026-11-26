//! Quality tests for the conversion result (requirements QA-01 to QA-07,
//! TEST-04 and TEST-05).
//!
//! The SVG is drawn again with the small rasterizer of `tests/common` and
//! compared with the source image.

mod common;

use std::collections::HashMap;
use std::fs;

use png2svg::pipeline::{convert, ConvertOptions};
use png2svg::png::{self, Image};

/// Loads a test image.
fn load(name: &str) -> (Image, usize) {
    let bytes = fs::read(common::fixture(&format!("{name}.png"))).expect("fixture file");
    (png::decode(&bytes).expect("fixture decodes"), bytes.len())
}

/// Converts with a detail level and returns the SVG text.
fn svg_at(image: &Image, detail: u32) -> String {
    convert(
        image,
        &ConvertOptions {
            detail,
            ..ConvertOptions::default()
        },
    )
    .0
}

/// Converts, draws the SVG again and returns the SSIM to the source image.
fn ssim_at(name: &str, detail: u32) -> f64 {
    let (image, _) = load(name);
    let svg = svg_at(&image, detail);
    let picture = common::rasterize(&common::parse_svg(&svg));
    common::ssim_rgba(&image.rgba, &picture, image.width, image.height)
}

#[test]
fn fixtures_exist_and_are_small_enough() {
    // QA-01: seven kinds of images, each one at most 1 MB.
    for name in common::FIXTURES {
        let size = fs::metadata(common::fixture(&format!("{name}.png")))
            .expect(name)
            .len();
        assert!(size > 0 && size <= 1_000_000, "{name}: {size} bytes");
    }
}

#[test]
fn every_output_is_well_formed_xml_with_the_right_root() {
    // TEST-05, SVG-01, SVG-04, SVG-05, SVG-08.
    for name in common::FIXTURES {
        let (image, _) = load(name);
        for detail in [1, 50, 100] {
            let svg = svg_at(&image, detail);
            common::check_xml(&svg).unwrap_or_else(|e| panic!("{name} at {detail}: {e}"));
            let expected_root = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w} {h}\" width=\"{w}\" height=\"{h}\"",
                w = image.width,
                h = image.height
            );
            assert!(
                svg.starts_with(&expected_root),
                "{name}: {}",
                &svg[..120.min(svg.len())]
            );
            assert!(!svg.contains('\n') && !svg.contains("<?xml") && !svg.contains("<!--"));
            assert!(!svg.contains("<metadata") && !svg.contains("fill-rule"));
            assert!(svg.is_ascii());
        }
    }
}

#[test]
fn flat_color_images_are_close_to_the_source_at_full_detail() {
    // QA-03: SSIM of at least 0.97 for flat color images.
    for name in ["logo", "icon", "text", "screenshot", "pixelart"] {
        let ssim = ssim_at(name, 100);
        assert!(ssim >= 0.97, "{name}: SSIM {ssim:.4}");
    }
}

#[test]
fn photo_and_gradient_are_reasonably_close_at_full_detail() {
    // QA-04: SSIM of at least 0.85 for the photo and the gradient (the photo
    // has sensor noise, which a vector image cannot copy).
    for name in ["photo", "gradient"] {
        let ssim = ssim_at(name, 100);
        assert!(ssim >= 0.85, "{name}: SSIM {ssim:.4}");
    }
}

#[test]
fn flat_color_svg_is_smaller_than_the_png_at_full_detail() {
    // QA-05. Pixel art is not in this list: a small sprite is stored by PNG in
    // a few hundred bytes, and no vector format can be shorter (see README).
    for name in ["logo", "icon", "text", "screenshot"] {
        let (image, png_size) = load(name);
        let svg = svg_at(&image, 100);
        assert!(
            svg.len() < png_size,
            "{name}: svg {} bytes, png {png_size} bytes",
            svg.len()
        );
    }
}

#[test]
fn size_does_not_grow_when_the_detail_level_goes_down() {
    // QA-06 (and section 8 of the requirements).
    for name in common::FIXTURES {
        let (image, _) = load(name);
        let sizes: Vec<usize> = [25, 50, 75, 100]
            .iter()
            .map(|&d| svg_at(&image, d).len())
            .collect();
        for pair in sizes.windows(2) {
            assert!(
                pair[0] <= pair[1],
                "{name}: sizes by detail 25/50/75/100 are {sizes:?}"
            );
        }
    }
}

#[test]
fn opaque_images_have_no_gaps() {
    // QA-07: if the source has no transparency, the result has no transparent
    // pixel anywhere. A gap would show the background through a thin line.
    for name in ["screenshot", "photo", "gradient"] {
        let (image, _) = load(name);
        assert!(
            image.rgba.chunks(4).all(|p| p[3] == 255),
            "{name} must be opaque"
        );
        for detail in [100, 50, 10] {
            let svg = svg_at(&image, detail);
            let picture = common::rasterize(&common::parse_svg(&svg));
            let gaps: Vec<usize> = picture
                .chunks(4)
                .enumerate()
                .filter(|(_, p)| p[3] < 250)
                .map(|(i, _)| i)
                .collect();
            assert!(
                gaps.is_empty(),
                "{name} at detail {detail}: {} pixels are not covered, first at {:?}",
                gaps.len(),
                gaps.first().map(|i| (i % image.width, i / image.width))
            );
        }
    }
}

#[test]
fn transparent_pixels_stay_transparent() {
    // SVG-02 and SVG-03: no background, and partly transparent edges.
    for name in ["logo", "icon", "text"] {
        let (image, _) = load(name);
        let svg = svg_at(&image, 100);
        assert!(
            !svg.contains("<rect"),
            "{name}: there must be no background rectangle"
        );
        let picture = common::rasterize(&common::parse_svg(&svg));
        // The corners of these images are transparent in the source.
        for corner in [
            0,
            image.width - 1,
            image.width * (image.height - 1),
            image.width * image.height - 1,
        ] {
            if image.rgba[corner * 4 + 3] == 0 {
                assert!(
                    picture[corner * 4 + 3] < 8,
                    "{name}: corner pixel {corner} is not transparent"
                );
            }
        }
        // The share of transparent pixels is about the same.
        let count = |rgba: &[u8]| rgba.chunks(4).filter(|p| p[3] < 128).count() as f64;
        let (a, b) = (count(&image.rgba), count(&picture));
        assert!((a - b).abs() / a.max(1.0) < 0.03, "{name}: {a} against {b}");
    }
}

#[test]
fn icon_shadow_uses_fill_opacity() {
    let (image, _) = load("icon");
    let svg = svg_at(&image, 100);
    assert!(
        svg.contains("fill-opacity="),
        "the soft shadow needs fill-opacity"
    );
}

#[test]
fn fully_transparent_image_gives_an_empty_svg() {
    // SVG-06
    let image = Image {
        width: 7,
        height: 5,
        rgba: vec![0; 7 * 5 * 4],
    };
    let svg = svg_at(&image, 100);
    assert_eq!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 7 5\" width=\"7\" height=\"5\"/>"
    );
    common::check_xml(&svg).unwrap();
}

#[test]
fn one_pixel_image_works() {
    // IN-12
    let image = Image {
        width: 1,
        height: 1,
        rgba: vec![10, 200, 30, 255],
    };
    let svg = svg_at(&image, 100);
    common::check_xml(&svg).unwrap();
    assert!(svg.contains("fill=\"#0ac81e\""), "{svg}");
}

#[test]
fn result_is_identical_on_every_run() {
    // TEST-04, SVG-07, ALG-14: the same input gives the same bytes.
    for name in common::FIXTURES {
        let (image, _) = load(name);
        for detail in [30, 100] {
            assert_eq!(
                svg_at(&image, detail),
                svg_at(&image, detail),
                "{name} at {detail}"
            );
        }
    }
}

#[test]
fn result_does_not_depend_on_the_thread_count_of_the_machine() {
    // The curve fitting uses threads for big images. The photo is big enough
    // to use them, and the output must still be the same as a single run.
    let (image, _) = load("photo");
    let first = svg_at(&image, 100);
    let handles: Vec<_> = (0..3)
        .map(|_| {
            let image = image.clone();
            std::thread::spawn(move || svg_at(&image, 100))
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().unwrap(), first);
    }
}

#[test]
fn explicit_color_limit_is_respected() {
    let (image, _) = load("photo");
    let (svg, stats) = convert(
        &image,
        &ConvertOptions {
            detail: 100,
            colors: Some(8),
            ..ConvertOptions::default()
        },
    );
    assert!(stats.colors <= 8, "{} colors", stats.colors);
    let fills: std::collections::HashSet<&str> = svg
        .split("fill=\"")
        .skip(1)
        .map(|s| s.split('"').next().unwrap())
        .collect();
    assert!(fills.len() <= 8);
}

#[test]
fn precision_option_changes_the_number_format() {
    let (image, _) = load("logo");
    let with = |precision| {
        convert(
            &image,
            &ConvertOptions {
                detail: 100,
                precision: Some(precision),
                ..ConvertOptions::default()
            },
        )
        .0
    };
    let (p0, p3) = (with(0), with(3));
    // With 0 decimals there is no dot in any number of the paths.
    let numbers_with_dot = |svg: &str| {
        svg.split(" d=\"")
            .skip(1)
            .any(|d| d.split('"').next().unwrap().contains('.'))
    };
    assert!(!numbers_with_dot(&p0));
    assert!(numbers_with_dot(&p3));
    assert!(p0.len() < p3.len());
}

#[test]
fn remove_background_makes_the_border_color_transparent() {
    // A white page with a red square: the white border color becomes transparent.
    let (w, h) = (40usize, 30usize);
    let mut rgba = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let inside = (10..30).contains(&x) && (8..22).contains(&y);
            rgba.extend_from_slice(if inside {
                &[200, 20, 20, 255]
            } else {
                &[255, 255, 255, 255]
            });
        }
    }
    let image = Image {
        width: w,
        height: h,
        rgba,
    };
    let plain = svg_at(&image, 100);
    let (svg, _) = convert(
        &image,
        &ConvertOptions {
            remove_background: true,
            ..ConvertOptions::default()
        },
    );
    assert!(plain.contains("fill=\"#fff\"") || plain.contains("<path d="));
    assert!(!svg.contains("#fff"), "{svg}");
    let picture = common::rasterize(&common::parse_svg(&svg));
    assert_eq!(picture[3], 0, "the corner is transparent");
    assert_eq!(picture[(15 * w + 20) * 4 + 3], 255, "the square stays");
}

#[test]
fn colors_of_flat_images_are_kept_exactly() {
    // A flat color is written as the same color in the SVG (#rrggbb -> #rgb
    // only when the digits repeat).
    let (image, _) = load("logo");
    let svg = svg_at(&image, 100);
    let mut counts: HashMap<[u8; 3], usize> = HashMap::new();
    for p in image.rgba.chunks(4).filter(|p| p[3] == 255) {
        *counts.entry([p[0], p[1], p[2]]).or_insert(0) += 1;
    }
    let (main, _) = counts.iter().max_by_key(|(_, &n)| n).unwrap();
    let hex = format!("#{:02x}{:02x}{:02x}", main[0], main[1], main[2]);
    assert!(svg.contains(&hex), "the main color {hex} is missing");
}

/// A small deterministic random number generator for the tests.
struct TestRng(u64);

impl TestRng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[test]
fn random_mosaics_are_reproduced_pixel_by_pixel() {
    // A mosaic with a few colors (and transparent pixels) has exact colors
    // and straight pixel edges. The regions, the boundary tracing, the holes,
    // the drawing order and the path writer must give back every single pixel.
    let palette: [[u8; 4]; 6] = [
        [0, 0, 0, 0],
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 255, 255],
        [17, 34, 51, 255],
    ];
    let mut rng = TestRng(0x1234_5678_9ABC_DEF1);
    for case in 0..120 {
        let width = 1 + rng.below(14) as usize;
        let height = 1 + rng.below(14) as usize;
        let colors = 1 + rng.below(6) as usize;
        // Some cases are blocky (large regions with holes), others are noisy.
        let block = 1 + rng.below(4) as usize;
        let mut rgba = Vec::new();
        let cells: Vec<usize> = (0..(width / block + 1) * (height / block + 1))
            .map(|_| rng.below(colors as u64) as usize)
            .collect();
        for y in 0..height {
            for x in 0..width {
                let cell = if case % 2 == 0 {
                    cells[(y / block) * (width / block + 1) + x / block]
                } else {
                    rng.below(colors as u64) as usize
                };
                rgba.extend_from_slice(&palette[cell]);
            }
        }
        let image = Image {
            width,
            height,
            rgba,
        };
        let svg = svg_at(&image, 100);
        common::check_xml(&svg).unwrap_or_else(|e| panic!("case {case}: {e}\n{svg}"));
        let picture = common::rasterize_centers(&common::parse_svg(&svg));
        for (i, (a, b)) in image.rgba.chunks(4).zip(picture.chunks(4)).enumerate() {
            let same = if a[3] == 0 { b[3] == 0 } else { a == b };
            assert!(
                same,
                "case {case} ({width}x{height}, {colors} colors): pixel ({}, {}) is {b:?}, expected {a:?}\n{svg}",
                i % width,
                i / width
            );
        }
    }
}

#[test]
fn random_images_of_any_kind_never_break_the_pipeline() {
    // Noise, strange sizes and random transparency: no panic, valid XML.
    let mut rng = TestRng(42);
    for case in 0..60 {
        let (width, height) = match case % 6 {
            0 => (1, 1 + rng.below(40) as usize),
            1 => (1 + rng.below(40) as usize, 1),
            2 => (2, 2),
            _ => (1 + rng.below(40) as usize, 1 + rng.below(40) as usize),
        };
        let mut rgba = Vec::new();
        for _ in 0..width * height {
            let alpha = match rng.below(4) {
                0 => 0,
                1 => rng.below(256) as u8,
                _ => 255,
            };
            rgba.extend_from_slice(&[
                rng.below(256) as u8,
                rng.below(256) as u8,
                rng.below(256) as u8,
                alpha,
            ]);
        }
        let image = Image {
            width,
            height,
            rgba,
        };
        for detail in [1, 37, 100] {
            let svg = svg_at(&image, detail);
            common::check_xml(&svg).unwrap_or_else(|e| panic!("case {case} detail {detail}: {e}"));
            let _ = common::rasterize(&common::parse_svg(&svg));
        }
    }
}

#[test]
fn extreme_inputs_work() {
    // A checkerboard (the most boundaries possible), a single color, and a
    // very long thin image.
    let checker = Image {
        width: 30,
        height: 30,
        rgba: (0..900)
            .flat_map(|i| {
                if (i % 30 + i / 30) % 2 == 0 {
                    [0, 0, 0, 255]
                } else {
                    [255, 255, 255, 255]
                }
            })
            .collect(),
    };
    let svg = svg_at(&checker, 100);
    let picture = common::rasterize_centers(&common::parse_svg(&svg));
    assert_eq!(
        picture, checker.rgba,
        "a checkerboard must be reproduced exactly"
    );

    let thin = Image {
        width: 600,
        height: 1,
        rgba: (0..600)
            .flat_map(|i| {
                if i % 7 < 3 {
                    [200, 0, 0, 255]
                } else {
                    [0, 0, 0, 0]
                }
            })
            .collect(),
    };
    common::check_xml(&svg_at(&thin, 100)).unwrap();
}

#[test]
fn thin_lines_survive_at_full_detail() {
    // ALG-12: lines that are 1 or 2 pixels wide must not disappear. The image
    // has a smooth gradient block too, so it is treated as a smooth picture
    // (with anti-aliasing removal and curve fitting), not as pixel art.
    let (w, h) = (96usize, 64usize);
    let mut rgba = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let mut px = [255u8, 255, 255, 255];
            if x >= 60 {
                // A gradient block on the right side.
                let v = (x - 60) as u32 * 7 + (y as u32 % 5);
                px = [v as u8, (255 - v.min(255)) as u8, 128, 255];
            }
            // A 1 pixel dark line and a 2 pixel red line.
            if y == 20 && x < 55 {
                px = [20, 20, 20, 255];
            }
            if (30..32).contains(&x) && y > 30 {
                px = [200, 0, 0, 255];
            }
            rgba.extend_from_slice(&px);
        }
    }
    let image = Image {
        width: w,
        height: h,
        rgba,
    };
    let svg = svg_at(&image, 100);
    let picture = common::rasterize(&common::parse_svg(&svg));
    let at = |x: usize, y: usize| &picture[(y * w + x) * 4..(y * w + x) * 4 + 4];
    for x in [5, 25, 50] {
        assert!(
            at(x, 20)[0] < 80,
            "1 pixel line is missing at x={x}: {:?}",
            at(x, 20)
        );
    }
    for y in [35, 45, 60] {
        assert!(
            at(30, y)[0] > 150 && at(30, y)[1] < 60,
            "2 pixel line is missing at y={y}: {:?}",
            at(30, y)
        );
    }
}
