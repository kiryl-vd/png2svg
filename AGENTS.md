# AGENTS.md

Instructions for AI agents (and people) who change this project.

## Project

`png2svg` is a command line program written in Rust. It converts a PNG image to
a compact SVG image with a transparent background. Read `README.md` first. It
describes the options, the file name rules, the detail table and the algorithm.

The main goal is a **small SVG file that keeps the details** of the image.
The second goal is that the same input gives the same bytes on every platform.

## Hard rules

1. **No runtime dependencies.** Do not add a crate to `[dependencies]`. If you
   believe one is really needed, stop and ask. Then explain it in the
   `Dependencies` section of `README.md` (why it is needed, why the program
   cannot work without it). Dev dependencies are also not used at this time.
2. **No `unsafe`.** The crates use `#![forbid(unsafe_code)]`.
3. **No panic on any input.** A damaged or strange PNG gives an error message
   and exit code 1. Do not use `unwrap`, `expect`, slice indexing or
   arithmetic that can fail on data that comes from the input file, unless the
   code just checked it. `main` catches panics as the last defense, but this
   must never be needed.
4. **Deterministic output.** Do not use any of these for the result:
   * `f64::powf`, `exp`, `ln`, `cbrt`, `sin`, `cos`, `atan2`, `acos` (the math
     library of the system can differ in the last bit). Use `+ - * /`, `sqrt`,
     and the functions in `src/color.rs`.
   * iteration over a `HashMap` or `HashSet` (the order is random). Sort the
     data first. Look up by key only.
   * random numbers other than `color::Rng` with a fixed seed.
   * threads whose result order depends on timing. Join the results in the order
     of the input blocks (see `vectorize::fit_all_arcs`).
5. **Comments in English.** All code has clear English comments. Every public
   item has a doc comment (`#![warn(missing_docs)]` is on). Comments explain
   *why*, not only *what*.
6. **Writing style.** In documentation, commit messages and comments use simple
   English (ASD-STE100 style): short sentences, no idioms, no phrasal verbs.
   Do not use the long dash. Do not use `;` in normal text.
7. **Do not weaken a test** to make it pass. If a test fails, find the reason.
   If a requirement must change, say so and update the tests and the README.

## Commands

```bash
cargo build --release
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                      # about 1 minute
cargo run --release --example bench -- 25 50 75 100
```

After a big change run all four checks (build, fmt, clippy, test). After a change
of the algorithm run the `bench` example and look at SSIM and size. The tests in
`tests/quality.rs` have limits for them.

If `cargo` is not found, add `~/.cargo/bin` to `PATH`.

## Code map

| Path | Purpose |
|---|---|
| `src/main.rs` | Entry point, exit codes, panic guard |
| `src/cli.rs` | Argument parser and help text |
| `src/output_path.rs` | Result file name, suffixes `-02`, `--force`, safe write |
| `src/png/` | PNG decoder: `checksum`, `inflate`, `filter` (filters and Adam7), `convert` (to RGBA8) |
| `src/pipeline.rs` | Runs all steps, statistics for `--verbose` |
| `src/unblend.rs` | Removes anti-aliasing (blend) pixels |
| `src/color.rs` | Oklab, deterministic roots, random numbers |
| `src/quantize.rs` | Transparency, alpha levels, palette (k-means), background removal |
| `src/segment.rs` | 4-connected regions, noise merge |
| `src/trace.rs` | Boundary arcs on pixel corners, region contours |
| `src/simplify.rs` | Smoothing and corner detection |
| `src/bezier.rs` | Curve fitting (Schneider), lines, rounding |
| `src/vectorize.rs` | Layers, drawing order, hole underlay, base rectangle, overlap stroke |
| `src/svg.rs` | Short SVG text |
| `src/detail.rs` | Table of parameters for `--detail` |
| `tests/` | `cli.rs`, `pngsuite.rs`, `quality.rs`, `common/` (PNG writer, rasterizer, SSIM), fixtures |
| `tools/` | Python and shell helpers for test data and measuring |
| `examples/` | `bench` (table), `render` (SVG back to PNG), `gaps` (find uncovered pixels) |
| `.github/workflows/ci.yml` | Build, format, clippy, tests and MSRV checks |
| `.github/workflows/release.yml` | Generated workflow for locally sent version tags |
| `.github/workflows/publish-release.yml` | Version PR request and publication after owner merge |
| `tools/release_request.py` | Manual request and merged version PR validation |
| `tools/test_release_request.py` | Protected-branch release request regression tests |
| `tools/prepare_release.py` | Stable version validation and Cargo version updates |
| `tools/test_prepare_release.py` | Unit tests for release version preparation |
| `tools/test_release_authorization.py` | Owner-only start and rerun authorization tests |
| `tools/check_release_checksums.py` | Complete archive checksum verification |
| `tools/test_release_checksums.py` | Checksum and manifest transfer regression tests |
| `dist-workspace.toml` | cargo-dist version, targets, installers and hosting |
| `.cargo/config.toml` | Static MSVC C runtime for Windows binaries |
| `release.toml` | cargo-release branch, version commit, tag and push rules |

## Design decisions to keep in mind

* A boundary between two regions is **one arc**, fitted once and used by both
  regions. Do not fit region outlines separately: this makes gaps.
* The geometry is rounded to integers in a grid (`scale` = 10^precision) once
  per arc. Do not round again in the writer.
* Pixel art (24 colors or fewer) uses exact polygons (`tolerance = 0`).
* The overlap stroke is only for the layer that is drawn first of two touching
  layers. The later layer must stay exact.
* A fully transparent area must never be covered. The base rectangle is only
  allowed for fully opaque images.
* If you change `detail.rs`, keep the sizes monotonic: a lower detail level
  must not give a bigger file (`tests/quality.rs` checks it).

## Test data

* `tests/fixtures/*.png` are made by `python3 tools/gen_fixtures.py` (needs
  Pillow). If you change the script, check the numbers in `README.md`.
* `tests/pngsuite/` is the PngSuite corpus. `expected.bin` is made by
  `python3 tools/gen_pngsuite_expected.py` (needs ImageMagick).
* Never put images with an unclear license in the repository.

## Releases

* Read [Releases (for the owner)](README.md#releases-for-the-owner) before
  changing release configuration.
* The owner requests a version with **Publish release** on `main`. A higher
  version is committed to `release/vX.Y.Z` and submitted as a version PR.
  The owner merges it after CI. That merge starts checks and publication
  from the exact merged commit. Ordinary PR merges must not publish releases.
* Never push version commits directly to protected `main`, merge version PRs
  automatically or add a broad GitHub Actions bypass. The legacy local
  cargo-release flow is not compatible with the active PR requirement.
* Do not create version commits, tags, pushes or GitHub Releases unless asked.
  A cargo-release dry run has no `--execute` flag. Do not use `--execute`
  without permission.
* Keep `publish = false` in `Cargo.toml` and `release.toml`. Keep
  `[package.metadata.dist] dist = true` so GitHub binaries can be released
  without publishing the crate to crates.io.
* Publish only macOS Apple Silicon (`aarch64-apple-darwin`), Linux x86_64
  (`x86_64-unknown-linux-gnu`) and Windows x64 (`x86_64-pc-windows-msvc`).
  Keep the paused Intel macOS and Linux ARM64 targets commented in
  `dist-workspace.toml` with an explanation. Do not enable other release
  targets unless the requirements change.
* Do not edit `.github/workflows/release.yml` by hand. Change
  `dist-workspace.toml`, then use the pinned cargo-dist version to run
  `dist generate`. Check `dist generate --check` and `dist plan`.
* Browser releases must pass CI on the exact prepared commit before builds
  and publication. Keep the owner and `main` restrictions, version validation
  and release concurrency guard. Both jobs with write access must check
  `kiryl-vd`, account ID `61699922`, repository, branch and `triggering_actor`.
  A retry can skip preparation, so publication needs its own check.
* The manual workflow uses `GITHUB_TOKEN`. Its branch pushes and PR creation
  do not start CI. Dispatch CI explicitly on the version branch so required
  checks belong to the PR's commit. The owner's merge starts publication.
  Keep publication CI on the exact merged commit.
* Keep the active main ruleset's PR and CI requirements. The workflow only
  creates PRs. The owner must merge them. Do not add a ruleset bypass actor.
* Keep release uploads in a draft until all assets are ready. A retry may
  resume a draft but must not overwrite a published release or change its tag.
* Pass each platform's `*-dist-manifest.json` to packaging. The global checksum
  list needs the checksum metadata from those manifests. Keep them internal,
  and check `sha256.sum` against the full release plan before publication.
* Keep the manual workflow's dist installer version consistent with
  `dist-workspace.toml`. Test version preparation with Python 3.11 or newer:
  `python3 -m unittest discover -s tools -p 'test_*.py'`.
* Local tag releases run separately from CI. Check that CI passed first.
* Never move or delete a published version tag.

## Attribution request

If you create a fork or use code from this repository in another project,
please include a link to the original repository in your README or project
credits: [kiryl-vd/png2svg](https://github.com/kiryl-vd/png2svg).
Thank you for acknowledging the original project. This is a polite request,
not an additional condition of the MIT License.

## When you finish a task

1. Run build, fmt, clippy and test. All must pass.
2. Update `README.md` if you changed options, rules or numbers.
3. Do not create commits or pull requests unless asked.
