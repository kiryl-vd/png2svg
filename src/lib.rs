//! # png2svg
//!
//! Converts a PNG image into a compact SVG image with a transparent background.
//! The library has no external dependencies. The PNG decoder, the DEFLATE
//! decompressor, the vectorization and the SVG writer are all written with the
//! Rust standard library only.
//!
//! The conversion pipeline (see [`pipeline`]):
//!
//! ```text
//! PNG file -> png (decoder) -> quantize (colors, alpha) -> segment (regions)
//!          -> trace (shared boundaries) -> simplify + bezier (curves)
//!          -> vectorize (layers) -> svg (text)
//! ```
//!
//! The command line program in `main.rs` uses [`cli`] and [`output_path`].

#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod bezier;
pub mod cli;
pub mod color;
pub mod detail;
pub mod output_path;
pub mod pipeline;
pub mod png;
pub mod quantize;
pub mod segment;
pub mod simplify;
pub mod svg;
pub mod trace;
pub mod unblend;
pub mod vectorize;
