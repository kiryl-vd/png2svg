//! Rules for the path of the result file: default name, numeric suffixes
//! (`-02`, `-03`, ...), `--force` and safe writing.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Largest number that is used in a suffix (`-9999`). If all names up to this
/// number exist, the program reports an error.
pub const MAX_SUFFIX: u32 = 9999;

/// Builds the default result path from the path of the PNG file.
///
/// The last extension is replaced with `.svg`. This is true for `.png` in any
/// letter case and also for any other extension. If the name has no extension,
/// `.svg` is added to the end. The directory does not change.
/// Examples: `a.PNG` -> `a.svg`, `my.logo.png` -> `my.logo.svg`,
/// `image.dat` -> `image.svg`, `image` -> `image.svg`.
pub fn default_output(input: &Path) -> PathBuf {
    // `Path::with_extension` replaces the last extension, or adds one when the
    // name has none. Only the last extension counts (`my.logo.png`).
    input.with_extension("svg")
}

/// Returns the candidate path with the given attempt number.
///
/// Attempt 1 is the path itself. Attempt `n >= 2` adds the suffix `-NN` (at
/// least two digits) to the file name, before the extension. A path without an
/// extension gets the suffix at the end. The suffix is added to the name as it
/// is, so `a-02.svg` becomes `a-02-02.svg`.
pub fn numbered(path: &Path, attempt: u32) -> PathBuf {
    if attempt <= 1 {
        return path.to_path_buf();
    }
    let suffix = format!("-{attempt:02}");
    let file_name = match path.file_name() {
        Some(n) => n.to_os_string(),
        None => return path.to_path_buf(),
    };
    // Split the file name into stem and extension. A dot at position 0 (hidden
    // file) is not an extension separator. `Path` already handles this.
    let stem = Path::new(&file_name)
        .file_stem()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    let extension = Path::new(&file_name).extension().map(|s| s.to_os_string());
    let mut new_name = stem;
    new_name.push(&suffix);
    if let Some(ext) = extension {
        new_name.push(".");
        new_name.push(ext);
    }
    path.with_file_name(new_name)
}

/// Checks the result path before the conversion starts, so the user does not
/// wait for the result of a conversion that cannot be saved.
///
/// Errors: the directory does not exist (OUT-14), the path is a directory
/// (OUT-15), the path is the source file (OUT-16).
pub fn validate_target(target: &Path, input: &Path) -> Result<(), String> {
    if target.as_os_str().is_empty() {
        return Err("the result path is empty".to_string());
    }
    // A path that ends with a separator can only mean a directory.
    let text = target.to_string_lossy();
    if text.ends_with('/') || text.ends_with(std::path::MAIN_SEPARATOR) {
        return Err(format!(
            "{}: the result path is a directory",
            target.display()
        ));
    }
    if target.is_dir() {
        return Err(format!(
            "{}: the result path is a directory",
            target.display()
        ));
    }
    let dir = parent_dir(target);
    if !dir.is_dir() {
        return Err(format!(
            "{}: the directory does not exist (directories are not created automatically)",
            dir.display()
        ));
    }
    if is_same_file(target, input) {
        return Err(format!(
            "{}: the result path is the same as the source file",
            target.display()
        ));
    }
    Ok(())
}

/// Returns the directory part of a path ("." for a bare file name).
fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Checks if two paths point to the same existing file. If the target does not
/// exist yet, the real directory of the target is compared with the file name.
fn is_same_file(target: &Path, input: &Path) -> bool {
    let input_real = match fs::canonicalize(input) {
        Ok(p) => p,
        Err(_) => return false,
    };
    if let Ok(target_real) = fs::canonicalize(target) {
        return target_real == input_real;
    }
    // The target does not exist, so it cannot be the same as an existing input.
    false
}

/// Saves the SVG data.
///
/// * Without `force`, the first free name is used (`name.svg`, `name-02.svg`,
///   `name-03.svg`, ...). The file is created with `create_new`, so an existing
///   file is never overwritten, even if it appears while the program runs.
/// * With `force`, the data is written to a temporary file in the same
///   directory and renamed to the target. If anything fails, the old file stays.
///
/// If a write fails, the incomplete file is removed (OUT-17).
/// Returns the path that was really written.
pub fn write_output(
    target: &Path,
    input: &Path,
    force: bool,
    data: &[u8],
) -> Result<PathBuf, String> {
    validate_target(target, input)?;
    if force {
        write_atomic(target, data)?;
        return Ok(target.to_path_buf());
    }
    for attempt in 1..=MAX_SUFFIX {
        let candidate = numbered(target, attempt);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                return match write_and_sync(&mut file, data) {
                    Ok(()) => Ok(candidate),
                    Err(e) => {
                        // Do not leave a partly written file.
                        drop(file);
                        let _ = fs::remove_file(&candidate);
                        Err(format!(
                            "{}: cannot write the file: {e}",
                            candidate.display()
                        ))
                    }
                };
            }
            // The name is taken. Try the next one.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "{}: cannot create the file: {e}",
                    candidate.display()
                ));
            }
        }
    }
    Err(format!(
        "{}: all names up to suffix -{MAX_SUFFIX} are in use",
        target.display()
    ))
}

/// Writes all data and asks the system to flush it to the disk.
fn write_and_sync(file: &mut File, data: &[u8]) -> io::Result<()> {
    file.write_all(data)?;
    file.flush()?;
    file.sync_all()
}

/// Writes through a temporary file and renames it over the target.
fn write_atomic(target: &Path, data: &[u8]) -> Result<(), String> {
    let dir = parent_dir(target);
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "output".to_string());
    // Try a few temporary names in case an old one is left over.
    for n in 0..100u32 {
        let temp = dir.join(format!(".{name}.{}.{n}.tmp", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(mut file) => {
                let result = write_and_sync(&mut file, data).and_then(|_| {
                    drop(file);
                    fs::rename(&temp, target)
                });
                return match result {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        let _ = fs::remove_file(&temp);
                        Err(format!("{}: cannot write the file: {e}", target.display()))
                    }
                };
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("{}: cannot create the file: {e}", temp.display())),
        }
    }
    Err(format!(
        "{}: cannot create a temporary file",
        target.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates an empty unique directory for a test.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("png2svg-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn default_name_replaces_the_png_extension() {
        assert_eq!(default_output(Path::new("a.png")), PathBuf::from("a.svg"));
        assert_eq!(default_output(Path::new("a.PNG")), PathBuf::from("a.svg"));
        assert_eq!(
            default_output(Path::new("dir/a.PnG")),
            PathBuf::from("dir/a.svg")
        );
        assert_eq!(
            default_output(Path::new("my.logo.png")),
            PathBuf::from("my.logo.svg")
        );
    }

    #[test]
    fn default_name_handles_other_or_missing_extensions() {
        assert_eq!(
            default_output(Path::new("image")),
            PathBuf::from("image.svg")
        );
        assert_eq!(
            default_output(Path::new("image.dat")),
            PathBuf::from("image.svg")
        );
        assert_eq!(
            default_output(Path::new("/x/y/image")),
            PathBuf::from("/x/y/image.svg")
        );
    }

    #[test]
    fn default_name_works_with_unicode_and_spaces() {
        assert_eq!(
            default_output(Path::new("мой файл/значок 1.png")),
            PathBuf::from("мой файл/значок 1.svg")
        );
    }

    #[test]
    fn numbered_names() {
        let p = Path::new("dir/a.svg");
        assert_eq!(numbered(p, 1), PathBuf::from("dir/a.svg"));
        assert_eq!(numbered(p, 2), PathBuf::from("dir/a-02.svg"));
        assert_eq!(numbered(p, 9), PathBuf::from("dir/a-09.svg"));
        assert_eq!(numbered(p, 10), PathBuf::from("dir/a-10.svg"));
        assert_eq!(numbered(p, 100), PathBuf::from("dir/a-100.svg"));
        assert_eq!(numbered(p, 9999), PathBuf::from("dir/a-9999.svg"));
    }

    #[test]
    fn numbered_names_without_extension_and_with_existing_suffix() {
        assert_eq!(numbered(Path::new("out"), 2), PathBuf::from("out-02"));
        assert_eq!(
            numbered(Path::new("a-02.svg"), 2),
            PathBuf::from("a-02-02.svg")
        );
        assert_eq!(
            numbered(Path::new("my.logo.svg"), 3),
            PathBuf::from("my.logo-03.svg")
        );
    }

    #[test]
    fn first_free_name_is_used() {
        let dir = temp_dir("free");
        let input = dir.join("in.png");
        fs::write(&input, b"x").unwrap();
        let target = dir.join("a.svg");
        fs::write(&target, b"old").unwrap();
        fs::write(dir.join("a-03.svg"), b"old").unwrap();
        let written = write_output(&target, &input, false, b"new").unwrap();
        assert_eq!(written, dir.join("a-02.svg"));
        assert_eq!(fs::read(&target).unwrap(), b"old");
        // The next call skips a-02 and a-03 and uses a-04.
        let written = write_output(&target, &input, false, b"new2").unwrap();
        assert_eq!(written, dir.join("a-04.svg"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_file_is_never_overwritten_without_force() {
        let dir = temp_dir("keep");
        let input = dir.join("in.png");
        fs::write(&input, b"x").unwrap();
        let target = dir.join("a.svg");
        fs::write(&target, b"old").unwrap();
        write_output(&target, &input, false, b"new").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"old");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn force_overwrites_exactly_the_target() {
        let dir = temp_dir("force");
        let input = dir.join("in.png");
        fs::write(&input, b"x").unwrap();
        let target = dir.join("a.svg");
        fs::write(&target, b"old").unwrap();
        let written = write_output(&target, &input, true, b"new").unwrap();
        assert_eq!(written, target);
        assert_eq!(fs::read(&target).unwrap(), b"new");
        // No temporary file is left.
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_missing_directory_directory_target_and_same_file() {
        let dir = temp_dir("errors");
        let input = dir.join("in.png");
        fs::write(&input, b"x").unwrap();
        // OUT-14
        let missing = dir.join("nope").join("a.svg");
        assert!(validate_target(&missing, &input)
            .unwrap_err()
            .contains("directory does not exist"));
        // OUT-15
        let subdir = dir.join("sub");
        fs::create_dir(&subdir).unwrap();
        assert!(validate_target(&subdir, &input)
            .unwrap_err()
            .contains("is a directory"));
        // OUT-16, also with force
        assert!(write_output(&input, &input, true, b"data")
            .unwrap_err()
            .contains("same as the source"));
        assert!(write_output(&input, &input, false, b"data").is_err());
        assert_eq!(fs::read(&input).unwrap(), b"x");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_file_is_detected_through_a_different_spelling() {
        let dir = temp_dir("spelling");
        let input = dir.join("in.png");
        fs::write(&input, b"x").unwrap();
        let other = dir.join(".").join("in.png");
        assert!(validate_target(&other, &input).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn works_with_spaces_and_unicode_in_the_path() {
        let dir = temp_dir("юникод и пробелы");
        let input = dir.join("файл 1.png");
        fs::write(&input, b"x").unwrap();
        let target = default_output(&input);
        let written = write_output(&target, &input, false, b"svg").unwrap();
        assert_eq!(written, dir.join("файл 1.svg"));
        let _ = fs::remove_dir_all(&dir);
    }
}
