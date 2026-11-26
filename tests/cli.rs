//! Tests of the command line program (requirement TEST-03).
//!
//! Every test starts the real binary in its own temporary directory.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The compiled program.
const BIN: &str = env!("CARGO_BIN_EXE_png2svg");

/// A temporary directory that is removed at the end of the test.
struct Sandbox(PathBuf);

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let dir = std::env::temp_dir().join(format!("png2svg-cli-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Sandbox(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// Runs the program in the sandbox directory.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(BIN)
            .args(args)
            .current_dir(&self.0)
            .output()
            .expect("start program")
    }

    /// Writes a small test image: a red square on a transparent background.
    fn write_png(&self, name: &str) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, test_png()).unwrap();
        path
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A 12x8 image: a red rectangle with a blue dot, the rest is transparent.
fn test_png() -> Vec<u8> {
    let (w, h) = (12usize, 8usize);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 1..7 {
        for x in 2..10 {
            let color = if (x, y) == (5, 3) {
                [0, 0, 255, 255]
            } else {
                [255, 0, 0, 255]
            };
            rgba[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&color);
        }
    }
    common::write_png_rgba(w, h, &rgba)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("exit code")
}

/// The printed result path, which must be one line.
fn printed_path(output: &Output) -> PathBuf {
    let text = stdout(output);
    assert_eq!(text.lines().count(), 1, "stdout: {text:?}");
    PathBuf::from(text.trim_end())
}

fn names(dir: &Path) -> Vec<String> {
    let mut list: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    list.sort();
    list
}

#[test]
fn default_name_replaces_png_with_svg_and_prints_the_path() {
    let sb = Sandbox::new("default");
    sb.write_png("logo.png");
    let out = sb.run(&["logo.png"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(printed_path(&out), PathBuf::from("logo.svg"));
    assert!(sb.path("logo.svg").is_file());
    let svg = fs::read_to_string(sb.path("logo.svg")).unwrap();
    common::check_xml(&svg).unwrap();
    assert!(stderr(&out).is_empty(), "no extra output: {}", stderr(&out));
}

#[test]
fn uppercase_extension_is_replaced() {
    let sb = Sandbox::new("upper");
    sb.write_png("ICON.PNG");
    let out = sb.run(&["ICON.PNG"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(printed_path(&out), PathBuf::from("ICON.svg"));
}

#[test]
fn other_or_missing_extension_gets_svg() {
    let sb = Sandbox::new("ext");
    sb.write_png("image.dat");
    sb.write_png("plain");
    sb.write_png("my.logo.png");
    assert_eq!(
        printed_path(&sb.run(&["image.dat"])),
        PathBuf::from("image.svg")
    );
    assert_eq!(
        printed_path(&sb.run(&["plain"])),
        PathBuf::from("plain.svg")
    );
    assert_eq!(
        printed_path(&sb.run(&["my.logo.png"])),
        PathBuf::from("my.logo.svg")
    );
}

#[test]
fn the_format_is_detected_by_the_signature_not_by_the_name() {
    let sb = Sandbox::new("signature");
    sb.write_png("picture.jpg");
    let out = sb.run(&["picture.jpg"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    fs::write(sb.path("fake.png"), b"this is not an image at all").unwrap();
    let out = sb.run(&["fake.png"]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("not a PNG"), "{}", stderr(&out));
    assert!(stderr(&out).contains("fake.png"));
}

#[test]
fn repeated_runs_add_numeric_suffixes() {
    let sb = Sandbox::new("suffix");
    sb.write_png("in.png");
    let expected = ["in.svg", "in-02.svg", "in-03.svg", "in-04.svg"];
    for name in expected {
        let out = sb.run(&["in.png"]);
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        assert_eq!(printed_path(&out), PathBuf::from(name));
    }
    assert_eq!(
        names(&sb.0),
        ["in-02.svg", "in-03.svg", "in-04.svg", "in.png", "in.svg"]
    );
}

#[test]
fn first_free_name_is_used() {
    let sb = Sandbox::new("first-free");
    sb.write_png("a.png");
    fs::write(sb.path("a.svg"), "old").unwrap();
    fs::write(sb.path("a-03.svg"), "old").unwrap();
    let out = sb.run(&["a.png"]);
    assert_eq!(printed_path(&out), PathBuf::from("a-02.svg"));
    let out = sb.run(&["a.png"]);
    assert_eq!(printed_path(&out), PathBuf::from("a-04.svg"));
    assert_eq!(fs::read_to_string(sb.path("a.svg")).unwrap(), "old");
    assert_eq!(fs::read_to_string(sb.path("a-03.svg")).unwrap(), "old");
}

#[test]
fn numbers_above_99_and_the_limit() {
    let sb = Sandbox::new("limit");
    sb.write_png("a.png");
    fs::write(sb.path("a.svg"), "old").unwrap();
    for n in 2..=99 {
        fs::write(sb.path(&format!("a-{n:02}.svg")), "old").unwrap();
    }
    let out = sb.run(&["a.png"]);
    assert_eq!(printed_path(&out), PathBuf::from("a-100.svg"));
    // All names up to -9999 are taken: the program must report an error.
    for n in 100..=9999 {
        fs::write(sb.path(&format!("a-{n}.svg")), "old").unwrap_or(());
    }
    let out = sb.run(&["a.png"]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("9999"), "{}", stderr(&out));
}

#[test]
fn suffix_rule_applies_to_an_explicit_output_path() {
    let sb = Sandbox::new("explicit");
    sb.write_png("in.png");
    fs::create_dir(sb.path("out")).unwrap();
    let first = sb.run(&["in.png", "out/result.svg"]);
    assert_eq!(printed_path(&first), PathBuf::from("out/result.svg"));
    let second = sb.run(&["in.png", "out/result.svg"]);
    assert_eq!(printed_path(&second), PathBuf::from("out/result-02.svg"));
    // The name is used as it is: no parsing of an existing suffix (OUT-10).
    let third = sb.run(&["in.png", "out/result-02.svg"]);
    assert_eq!(printed_path(&third), PathBuf::from("out/result-02-02.svg"));
    // Without extension the suffix goes to the end (OUT-09).
    assert_eq!(
        printed_path(&sb.run(&["in.png", "bare"])),
        PathBuf::from("bare")
    );
    assert_eq!(
        printed_path(&sb.run(&["in.png", "bare"])),
        PathBuf::from("bare-02")
    );
}

#[test]
fn output_option_is_the_same_as_the_second_argument() {
    let sb = Sandbox::new("o-option");
    sb.write_png("in.png");
    let out = sb.run(&["-o", "x.svg", "in.png"]);
    assert_eq!(printed_path(&out), PathBuf::from("x.svg"));
    let out = sb.run(&["--output=y.svg", "in.png"]);
    assert_eq!(printed_path(&out), PathBuf::from("y.svg"));
    let out = sb.run(&["-o", "x.svg", "in.png", "z.svg"]);
    assert_eq!(code(&out), 2);
}

#[test]
fn force_overwrites_exactly_the_target() {
    let sb = Sandbox::new("force");
    sb.write_png("in.png");
    fs::write(sb.path("in.svg"), "old content").unwrap();
    let out = sb.run(&["--force", "in.png", "in.svg"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(printed_path(&out), PathBuf::from("in.svg"));
    let svg = fs::read_to_string(sb.path("in.svg")).unwrap();
    assert!(svg.starts_with("<svg"), "{svg}");
    assert_eq!(
        names(&sb.0),
        ["in.png", "in.svg"],
        "no suffix file and no temporary file"
    );
    // -f with the default name, and when the target does not exist yet.
    let out = sb.run(&["-f", "in.png"]);
    assert_eq!(printed_path(&out), PathBuf::from("in.svg"));
    fs::remove_file(sb.path("in.svg")).unwrap();
    assert_eq!(
        printed_path(&sb.run(&["-f", "in.png"])),
        PathBuf::from("in.svg")
    );
}

#[test]
fn output_equal_to_input_is_an_error_even_with_force() {
    let sb = Sandbox::new("same");
    let input = sb.write_png("in.png");
    let before = fs::read(&input).unwrap();
    for args in [
        &["in.png", "in.png"][..],
        &["--force", "in.png", "in.png"],
        &["-f", "in.png", "./in.png"],
    ] {
        let out = sb.run(args);
        assert_eq!(code(&out), 1, "{args:?}");
        assert!(
            stderr(&out).contains("same as the source"),
            "{}",
            stderr(&out)
        );
        assert_eq!(
            fs::read(&input).unwrap(),
            before,
            "the source must not change"
        );
    }
}

#[test]
fn missing_output_directory_and_directory_as_output_are_errors() {
    let sb = Sandbox::new("dirs");
    sb.write_png("in.png");
    let out = sb.run(&["in.png", "nope/out.svg"]);
    assert_eq!(code(&out), 1);
    assert!(
        stderr(&out).contains("directory does not exist"),
        "{}",
        stderr(&out)
    );
    assert!(!sb.path("nope").exists(), "directories must not be created");
    fs::create_dir(sb.path("folder")).unwrap();
    for args in [
        &["in.png", "folder"][..],
        &["--force", "in.png", "folder"],
        &["in.png", "folder/"],
    ] {
        let out = sb.run(args);
        assert_eq!(code(&out), 1, "{args:?}");
        assert!(stderr(&out).contains("is a directory"), "{}", stderr(&out));
    }
}

#[test]
fn missing_input_and_directory_input_are_errors() {
    let sb = Sandbox::new("input-errors");
    let out = sb.run(&["absent.png"]);
    assert_eq!(code(&out), 1);
    assert!(
        stderr(&out).contains("absent.png") && stderr(&out).contains("not found"),
        "{}",
        stderr(&out)
    );
    assert!(stdout(&out).is_empty());
    fs::create_dir(sb.path("dir.png")).unwrap();
    assert_eq!(code(&sb.run(&["dir.png"])), 1);
}

#[test]
fn damaged_png_files_give_an_error_and_no_panic() {
    let sb = Sandbox::new("damaged");
    let good = test_png();
    // Cut file, flipped bytes, and only the signature.
    fs::write(sb.path("cut.png"), &good[..good.len() - 20]).unwrap();
    let mut flipped = good.clone();
    let middle = flipped.len() / 2;
    flipped[middle] ^= 0xFF;
    fs::write(sb.path("flip.png"), &flipped).unwrap();
    fs::write(sb.path("sig.png"), &good[..8]).unwrap();
    fs::write(sb.path("empty.png"), b"").unwrap();
    for name in ["cut.png", "flip.png", "sig.png", "empty.png"] {
        let out = sb.run(&[name]);
        assert_eq!(code(&out), 1, "{name}");
        let message = stderr(&out);
        assert!(message.contains(name), "{message}");
        assert!(
            !message.contains("panicked") && !message.contains("RUST_BACKTRACE"),
            "{message}"
        );
        assert!(
            !sb.path(&name.replace(".png", ".svg")).exists(),
            "no result file for {name}"
        );
    }
}

#[test]
fn image_that_is_too_large_gives_a_clear_error() {
    let sb = Sandbox::new("large");
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&20_000u32.to_be_bytes());
    ihdr.extend_from_slice(&20_000u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut file = png2svg::png::SIGNATURE.to_vec();
    common::push_chunk(&mut file, b"IHDR", &ihdr);
    common::push_chunk(&mut file, b"IEND", &[]);
    fs::write(sb.path("huge.png"), file).unwrap();
    let out = sb.run(&["huge.png"]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("too large"), "{}", stderr(&out));
}

#[test]
fn usage_errors_exit_with_code_2_and_a_hint() {
    let sb = Sandbox::new("usage");
    sb.write_png("in.png");
    let cases: Vec<Vec<&str>> = vec![
        vec![],
        vec!["--detail", "0", "in.png"],
        vec!["--detail", "101", "in.png"],
        vec!["--detail", "abc", "in.png"],
        vec!["-d", "-5", "in.png"],
        vec!["--detail"],
        vec!["--unknown", "in.png"],
        vec!["-z", "in.png"],
        vec!["in.png", "a.svg", "extra"],
        vec!["--colors", "1", "in.png"],
        vec!["--precision", "9", "in.png"],
        vec!["-v", "-q", "in.png"],
    ];
    for args in cases {
        let out = sb.run(&args);
        assert_eq!(code(&out), 2, "{args:?}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("--help"),
            "{args:?}: {}",
            stderr(&out)
        );
        assert!(stdout(&out).is_empty(), "{args:?}");
    }
    assert_eq!(
        names(&sb.0),
        ["in.png"],
        "nothing is written after a usage error"
    );
}

#[test]
fn help_and_version_exit_with_code_0() {
    let sb = Sandbox::new("help");
    for flag in ["--help", "-h"] {
        let out = sb.run(&[flag]);
        assert_eq!(code(&out), 0);
        let text = stdout(&out);
        assert!(text.contains("USAGE") && text.contains("--detail") && text.contains("--force"));
    }
    for flag in ["--version", "-V"] {
        let out = sb.run(&[flag]);
        assert_eq!(code(&out), 0);
        assert!(stdout(&out).starts_with("png2svg "), "{}", stdout(&out));
    }
}

#[test]
fn detail_forms_and_order_of_arguments() {
    let sb = Sandbox::new("detail");
    sb.write_png("in.png");
    for args in [
        vec!["--detail", "50", "in.png", "a.svg"],
        vec!["--detail=50", "in.png", "b.svg"],
        vec!["-d", "50", "in.png", "c.svg"],
        vec!["in.png", "-d50", "d.svg"],
        vec!["in.png", "e.svg", "--detail", "50"],
    ] {
        let out = sb.run(&args);
        assert_eq!(code(&out), 0, "{args:?}: {}", stderr(&out));
    }
    // The default is the maximum detail: the same as --detail 100.
    let _ = sb.run(&["in.png", "default.svg"]);
    let _ = sb.run(&["-d", "100", "in.png", "full.svg"]);
    assert_eq!(
        fs::read(sb.path("default.svg")).unwrap(),
        fs::read(sb.path("full.svg")).unwrap()
    );
}

#[test]
fn double_dash_allows_names_that_start_with_a_dash() {
    let sb = Sandbox::new("dashdash");
    sb.write_png("-odd.png");
    let out = sb.run(&["--", "-odd.png"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(printed_path(&out), PathBuf::from("-odd.svg"));
}

#[test]
fn quiet_prints_nothing_and_verbose_prints_statistics() {
    let sb = Sandbox::new("quiet");
    sb.write_png("in.png");
    let out = sb.run(&["-q", "in.png"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).is_empty() && stderr(&out).is_empty());
    assert!(sb.path("in.svg").is_file());

    let out = sb.run(&["-v", "in.png"]);
    assert_eq!(code(&out), 0);
    let report = stderr(&out);
    for needle in [
        "image: 12x8",
        "colors:",
        "regions:",
        "paths:",
        "svg size:",
        "time total",
    ] {
        assert!(report.contains(needle), "{needle} in {report}");
    }
    // The result path is still the only thing on stdout.
    assert_eq!(printed_path(&out), PathBuf::from("in-02.svg"));

    // Errors are printed even with --quiet.
    let out = sb.run(&["-q", "absent.png"]);
    assert_eq!(code(&out), 1);
    assert!(!stderr(&out).is_empty());
}

#[test]
fn paths_with_spaces_and_unicode_work() {
    let sb = Sandbox::new("unicode path");
    fs::create_dir(sb.path("папка с пробелами")).unwrap();
    fs::write(sb.path("папка с пробелами/значок 1.png"), test_png()).unwrap();
    let out = sb.run(&["папка с пробелами/значок 1.png"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        printed_path(&out),
        PathBuf::from("папка с пробелами/значок 1.svg")
    );
    assert!(sb.path("папка с пробелами/значок 1.svg").is_file());
}

#[test]
fn absolute_paths_work() {
    let sb = Sandbox::new("absolute");
    let input = sb.write_png("in.png");
    let output = sb.path("result.svg");
    let out = Command::new(BIN).arg(&input).arg(&output).output().unwrap();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(printed_path(&out), output);
}

#[test]
fn result_is_valid_and_has_a_transparent_background() {
    let sb = Sandbox::new("content");
    sb.write_png("in.png");
    assert_eq!(code(&sb.run(&["in.png"])), 0);
    let svg = fs::read_to_string(sb.path("in.svg")).unwrap();
    common::check_xml(&svg).unwrap();
    let picture = common::rasterize(&common::parse_svg(&svg));
    // Corner pixel: transparent. Inside the rectangle: red. The dot: blue.
    assert_eq!(picture[3], 0);
    let at = |x: usize, y: usize| &picture[(y * 12 + x) * 4..(y * 12 + x) * 4 + 4];
    assert_eq!(at(3, 2), [255, 0, 0, 255]);
    assert_eq!(at(5, 3), [0, 0, 255, 255]);
    assert_eq!(at(11, 7)[3], 0);
}

#[test]
fn two_runs_give_identical_files() {
    // TEST-04
    let sb = Sandbox::new("determinism");
    fs::copy(common::fixture("logo.png"), sb.path("logo.png")).unwrap();
    assert_eq!(code(&sb.run(&["logo.png", "one.svg"])), 0);
    assert_eq!(code(&sb.run(&["logo.png", "two.svg"])), 0);
    assert_eq!(
        fs::read(sb.path("one.svg")).unwrap(),
        fs::read(sb.path("two.svg")).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn unwritable_directory_is_a_runtime_error_without_leftovers() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new("readonly");
    sb.write_png("in.png");
    fs::create_dir(sb.path("locked")).unwrap();
    fs::set_permissions(sb.path("locked"), fs::Permissions::from_mode(0o555)).unwrap();
    // A user with special rights (root) can still write: skip the check then.
    let can_write = fs::write(sb.path("locked/probe"), b"x").is_ok();
    if !can_write {
        let out = sb.run(&["in.png", "locked/out.svg"]);
        assert_eq!(code(&out), 1);
        assert!(stderr(&out).contains("locked"), "{}", stderr(&out));
        assert!(names(&sb.path("locked")).is_empty());
    }
    fs::set_permissions(sb.path("locked"), fs::Permissions::from_mode(0o755)).unwrap();
}
