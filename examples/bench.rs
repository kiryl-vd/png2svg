//! Quality and size table for the test images (requirement QA-08).
//!
//! Usage:
//!
//! ```text
//! cargo run --release --example bench              # detail 100
//! cargo run --release --example bench -- 100 75 50 # several detail levels
//! ```
//!
//! For every image in `tests/fixtures` the program converts the PNG, draws the
//! SVG again with the small test rasterizer, and prints the PNG size, the SVG
//! size, the SSIM similarity (1.0 = identical) and the conversion time.

use std::fs;
use std::time::Instant;

use png2svg::pipeline::{convert, ConvertOptions};
use png2svg::png;

#[path = "../tests/common/mod.rs"]
mod common;

fn main() {
    let mut levels: Vec<u32> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    if levels.is_empty() {
        levels.push(100);
    }
    println!("| image | detail | PNG bytes | SVG bytes | SVG/PNG | SSIM | paths | time ms |");
    println!("|---|---|---|---|---|---|---|---|");
    for name in common::FIXTURES {
        let path = common::fixture(&format!("{name}.png"));
        let bytes = fs::read(&path).expect("fixture");
        let image = png::decode(&bytes).expect("decode");
        for &detail in &levels {
            let options = ConvertOptions {
                detail,
                ..ConvertOptions::default()
            };
            let start = Instant::now();
            let (svg, stats) = convert(&image, &options);
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            let parsed = common::parse_svg(&svg);
            let picture = common::rasterize(&parsed);
            let ssim = common::ssim_rgba(&image.rgba, &picture, image.width, image.height);
            println!(
                "| {name} | {detail} | {} | {} | {:.2} | {:.3} | {} | {:.0} |",
                bytes.len(),
                svg.len(),
                svg.len() as f64 / bytes.len() as f64,
                ssim,
                stats.paths,
                elapsed
            );
        }
    }
}
