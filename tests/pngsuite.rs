//! Decoder tests with the PngSuite corpus (requirement TEST-02).
//!
//! * Every valid file must decode to the same pixels as an independent decoder
//!   (ImageMagick). The expected pixels are stored in `tests/pngsuite/expected.bin`,
//!   see `tools/gen_pngsuite_expected.py`.
//! * Every file whose name starts with `x` is damaged on purpose. The decoder
//!   must return an error and must not panic.

use std::fs;
use std::path::PathBuf;

use png2svg::png;

fn suite_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("pngsuite")
}

#[test]
fn valid_files_decode_to_the_expected_pixels() {
    let dir = suite_dir();
    let index = fs::read_to_string(dir.join("expected.txt")).expect("expected.txt");
    let blob = fs::read(dir.join("expected.bin")).expect("expected.bin");
    let mut checked = 0;
    let mut failures = Vec::new();
    for line in index.lines() {
        let parts: Vec<&str> = line.split(' ').collect();
        let (name, w, h, offset): (&str, usize, usize, usize) = (
            parts[0],
            parts[1].parse().unwrap(),
            parts[2].parse().unwrap(),
            parts[3].parse().unwrap(),
        );
        let bytes = fs::read(dir.join(name)).unwrap();
        match png::decode(&bytes) {
            Ok(image) => {
                let expected = &blob[offset..offset + w * h * 4];
                if image.width != w || image.height != h || image.rgba != expected {
                    failures.push(format!("{name}: pixels differ"));
                }
            }
            Err(e) => failures.push(format!("{name}: {e}")),
        }
        checked += 1;
    }
    assert!(checked >= 150, "too few files were checked: {checked}");
    assert!(failures.is_empty(), "failures:\n{}", failures.join("\n"));
}

#[test]
fn damaged_files_give_an_error() {
    let dir = suite_dir();
    let mut checked = 0;
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !name.starts_with('x') || !name.ends_with(".png") {
            continue;
        }
        let bytes = fs::read(&path).unwrap();
        assert!(png::decode(&bytes).is_err(), "{name} must be rejected");
        checked += 1;
    }
    assert_eq!(checked, 14);
}

#[test]
fn truncated_and_flipped_suite_files_never_panic() {
    let dir = suite_dir();
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().map(|e| e != "png").unwrap_or(true) {
            continue;
        }
        let bytes = fs::read(&path).unwrap();
        // Cut the file at a few places and flip a few bytes.
        for cut in [
            0,
            7,
            8,
            20,
            33,
            bytes.len() / 2,
            bytes.len().saturating_sub(1),
        ] {
            let _ = png::decode(&bytes[..cut.min(bytes.len())]);
        }
        for i in (0..bytes.len()).step_by(7) {
            let mut copy = bytes.clone();
            copy[i] ^= 0x55;
            let _ = png::decode(&copy);
        }
    }
}
