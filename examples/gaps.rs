//! Developer tool: lists the pixels that the SVG leaves uncovered when the
//! source image is opaque. Usage: `cargo run --release --example gaps -- <png> [detail]`
use std::fs;

use png2svg::pipeline::{convert, ConvertOptions};
use png2svg::png;

#[path = "../tests/common/mod.rs"]
mod common;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let image = png::decode(&fs::read(&args[0]).expect("read")).expect("decode");
    let detail = args.get(1).and_then(|d| d.parse().ok()).unwrap_or(100);
    let (svg, _) = convert(
        &image,
        &ConvertOptions {
            detail,
            ..ConvertOptions::default()
        },
    );
    let picture = common::rasterize(&common::parse_svg(&svg));
    let mut shown = 0;
    for (i, p) in picture.chunks(4).enumerate() {
        if p[3] < 250 && shown < 15 {
            println!(
                "gap at ({}, {}) alpha {}",
                i % image.width,
                i / image.width,
                p[3]
            );
            shown += 1;
        }
    }
}
