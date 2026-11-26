//! Developer tool: converts a PNG, draws the SVG again with the test
//! rasterizer, and writes the result as a PNG (to check the quality by eye).
//!
//! Usage: `cargo run --release --example render -- <in.png> <out.png> [detail]`

use std::fs;

use png2svg::pipeline::{convert, ConvertOptions};
use png2svg::png;

#[path = "../tests/common/mod.rs"]
mod common;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: render <in.png> <out.png> [detail]");
        std::process::exit(2);
    }
    let image = png::decode(&fs::read(&args[0]).expect("read")).expect("decode");
    let detail = args.get(2).and_then(|d| d.parse().ok()).unwrap_or(100);
    let (svg, _) = convert(
        &image,
        &ConvertOptions {
            detail,
            ..ConvertOptions::default()
        },
    );
    let raster = common::rasterize(&common::parse_svg(&svg));
    fs::write(
        &args[1],
        common::write_png_rgba(image.width, image.height, &raster),
    )
    .expect("write");
}
