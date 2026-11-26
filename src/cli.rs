//! Command line parsing and help text.
//!
//! The parser is written by hand (no external crate). It supports:
//! `--detail 50`, `--detail=50`, `-d 50`, `-d50`, grouped flags like `-fv`,
//! options and positional arguments in any order, and `--` to end the options.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

/// Name of the program, used in messages.
pub const PROGRAM: &str = "png2svg";

/// Default detail level (maximum detail).
pub const DEFAULT_DETAIL: u32 = 100;

/// Settings chosen by the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Path of the source PNG file.
    pub input: PathBuf,
    /// Path of the result file given by the user (positional or `--output`).
    pub output: Option<PathBuf>,
    /// Detail level, 1..=100.
    pub detail: u32,
    /// Overwrite an existing result file.
    pub force: bool,
    /// Print statistics to stderr.
    pub verbose: bool,
    /// Print nothing except errors.
    pub quiet: bool,
    /// Explicit maximum number of palette colors (2..=256).
    pub colors: Option<u32>,
    /// Explicit number of decimal places in SVG coordinates (0..=3).
    pub precision: Option<u32>,
    /// Make the background color (the color of the image border) transparent.
    pub remove_background: bool,
}

/// What the program has to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Convert a file.
    Run(Options),
    /// Print the help text.
    Help,
    /// Print the version.
    Version,
}

/// A mistake in the command line. The program exits with code 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError(pub String);

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for UsageError {}

/// Builds a [`UsageError`] result.
fn usage<T>(text: impl Into<String>) -> Result<T, UsageError> {
    Err(UsageError(text.into()))
}

/// Text for `--help`.
pub fn help_text() -> String {
    format!(
        "{PROGRAM} {version}
Convert a PNG image to a compact SVG image with a transparent background.

USAGE:
    {PROGRAM} [OPTIONS] <INPUT> [OUTPUT]

ARGUMENTS:
    <INPUT>     Path of the source PNG file.
    [OUTPUT]    Path of the result SVG file. By default the extension of INPUT
                is replaced with .svg. If the file exists, the name gets the
                suffix -02, -03 and so on (use --force to overwrite).

OPTIONS:
    -d, --detail <1..100>   Level of detail. 100 = maximum detail (default),
                            1 = smallest file.
    -f, --force             Overwrite the result file if it exists.
    -o, --output <PATH>     Another way to give the result path.
    -v, --verbose           Print statistics and timing to stderr.
    -q, --quiet             Print nothing except errors.
        --colors <2..256>   Maximum number of colors (overrides --detail).
        --precision <0..3>  Decimal places in coordinates (overrides --detail).
        --remove-background Make the color of the image border transparent.
    -h, --help              Print this help and exit.
    -V, --version           Print the version and exit.

EXIT CODES:
    0  success
    1  runtime error (file not found, damaged PNG, write error)
    2  usage error (wrong arguments)

EXAMPLES:
    {PROGRAM} logo.png                    # writes logo.svg (or logo-02.svg)
    {PROGRAM} logo.png out/logo.svg       # explicit result path
    {PROGRAM} -d 40 photo.png             # less detail, smaller file
    {PROGRAM} --force logo.png logo.svg   # overwrite the existing file
",
        version = env!("CARGO_PKG_VERSION")
    )
}

/// Text for `--version`.
pub fn version_text() -> String {
    format!("{PROGRAM} {}", env!("CARGO_PKG_VERSION"))
}

/// Parses the arguments (without the program name).
pub fn parse<I>(args: I) -> Result<Command, UsageError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut iter = args.into_iter();
    let mut positional: Vec<OsString> = Vec::new();
    let mut output_option: Option<PathBuf> = None;
    let mut detail = DEFAULT_DETAIL;
    let mut force = false;
    let mut verbose = false;
    let mut quiet = false;
    let mut colors = None;
    let mut precision = None;
    let mut remove_background = false;
    let mut options_ended = false;

    while let Some(arg) = iter.next() {
        // After "--" every argument is a file name.
        if options_ended {
            positional.push(arg);
            continue;
        }
        let text = match arg.to_str() {
            Some(t) => t,
            None => {
                // A name that is not valid UTF-8 can only be a file name.
                positional.push(arg);
                continue;
            }
        };
        if text == "--" {
            options_ended = true;
        } else if let Some(long) = text.strip_prefix("--") {
            // Long option, with the value after "=" or in the next argument.
            let (name, inline_value) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (long, None),
            };
            match name {
                "help" => return no_value(name, &inline_value).map(|_| Command::Help),
                "version" => return no_value(name, &inline_value).map(|_| Command::Version),
                "force" => {
                    no_value(name, &inline_value)?;
                    force = true;
                }
                "verbose" => {
                    no_value(name, &inline_value)?;
                    verbose = true;
                }
                "quiet" => {
                    no_value(name, &inline_value)?;
                    quiet = true;
                }
                "remove-background" => {
                    no_value(name, &inline_value)?;
                    remove_background = true;
                }
                "detail" => {
                    detail = parse_range(
                        "--detail",
                        &take_value(name, inline_value, &mut iter)?,
                        1,
                        100,
                    )?
                }
                "colors" => {
                    colors = Some(parse_range(
                        "--colors",
                        &take_value(name, inline_value, &mut iter)?,
                        2,
                        256,
                    )?)
                }
                "precision" => {
                    precision = Some(parse_range(
                        "--precision",
                        &take_value(name, inline_value, &mut iter)?,
                        0,
                        3,
                    )?)
                }
                "output" => {
                    set_output(
                        &mut output_option,
                        take_value(name, inline_value, &mut iter)?,
                    )?;
                }
                _ => return usage(format!("unknown option '--{name}'")),
            }
        } else if text.len() > 1 && text.starts_with('-') {
            // A group of short options, for example "-fv" or "-d50".
            let flags: Vec<char> = text[1..].chars().collect();
            let mut i = 0;
            while i < flags.len() {
                let c = flags[i];
                i += 1;
                match c {
                    'h' => return Ok(Command::Help),
                    'V' => return Ok(Command::Version),
                    'f' => force = true,
                    'v' => verbose = true,
                    'q' => quiet = true,
                    'd' | 'o' => {
                        // The value is the rest of this argument or the next argument.
                        let rest: String = flags[i..].iter().collect();
                        i = flags.len();
                        let inline = if rest.is_empty() { None } else { Some(rest) };
                        let name = if c == 'd' { "detail" } else { "output" };
                        let value = take_value(name, inline, &mut iter)?;
                        if c == 'd' {
                            detail = parse_range("--detail", &value, 1, 100)?;
                        } else {
                            set_output(&mut output_option, value)?;
                        }
                    }
                    other => return usage(format!("unknown option '-{other}'")),
                }
            }
        } else {
            positional.push(arg);
        }
    }

    if verbose && quiet {
        return usage("options --verbose and --quiet cannot be used together");
    }
    let mut positional = positional.into_iter();
    let input = match positional.next() {
        Some(p) => PathBuf::from(p),
        None => return usage("the input file is missing"),
    };
    let output_positional = positional.next().map(PathBuf::from);
    if positional.next().is_some() {
        return usage("too many arguments");
    }
    if output_positional.is_some() && output_option.is_some() {
        return usage("the result path is given twice (as an argument and with --output)");
    }
    Ok(Command::Run(Options {
        input,
        output: output_positional.or(output_option),
        detail,
        force,
        verbose,
        quiet,
        colors,
        precision,
        remove_background,
    }))
}

/// Fails if a flag that has no value got one (`--force=1`).
fn no_value(name: &str, inline: &Option<String>) -> Result<(), UsageError> {
    if inline.is_some() {
        return usage(format!("option '--{name}' does not take a value"));
    }
    Ok(())
}

/// Gets the value of an option: the inline one, or the next argument.
fn take_value(
    name: &str,
    inline: Option<String>,
    iter: &mut impl Iterator<Item = OsString>,
) -> Result<String, UsageError> {
    if let Some(v) = inline {
        return Ok(v);
    }
    match iter.next() {
        Some(v) => match v.into_string() {
            Ok(s) => Ok(s),
            Err(_) => usage(format!("the value of '--{name}' is not valid text")),
        },
        None => usage(format!("option '--{name}' needs a value")),
    }
}

/// Stores the `--output` value and rejects a second one.
fn set_output(slot: &mut Option<PathBuf>, value: String) -> Result<(), UsageError> {
    if slot.is_some() {
        return usage("option '--output' is given twice");
    }
    *slot = Some(PathBuf::from(value));
    Ok(())
}

/// Parses an integer option value and checks its range.
fn parse_range(name: &str, text: &str, min: u32, max: u32) -> Result<u32, UsageError> {
    match text.trim().parse::<u32>() {
        Ok(v) if (min..=max).contains(&v) => Ok(v),
        Ok(_) => usage(format!(
            "the value of {name} must be from {min} to {max}, got '{text}'"
        )),
        Err(_) => usage(format!(
            "the value of {name} must be a whole number, got '{text}'"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Command, UsageError> {
        parse(args.iter().map(OsString::from))
    }

    fn options(args: &[&str]) -> Options {
        match run(args).unwrap() {
            Command::Run(o) => o,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn defaults() {
        let o = options(&["a.png"]);
        assert_eq!(o.input, PathBuf::from("a.png"));
        assert_eq!(o.output, None);
        assert_eq!(o.detail, 100);
        assert!(!o.force && !o.verbose && !o.quiet && !o.remove_background);
    }

    #[test]
    fn detail_forms() {
        assert_eq!(options(&["--detail", "50", "a.png"]).detail, 50);
        assert_eq!(options(&["--detail=7", "a.png"]).detail, 7);
        assert_eq!(options(&["-d", "40", "a.png"]).detail, 40);
        assert_eq!(options(&["-d40", "a.png"]).detail, 40);
    }

    #[test]
    fn invalid_detail_is_an_error() {
        for bad in ["0", "101", "abc", "-5", "5.5", ""] {
            assert!(run(&["-d", bad, "a.png"]).is_err(), "value {bad:?}");
        }
        assert!(run(&["a.png", "-d"]).is_err());
    }

    #[test]
    fn positional_output_and_option_output() {
        assert_eq!(
            options(&["a.png", "b.svg"]).output,
            Some(PathBuf::from("b.svg"))
        );
        assert_eq!(
            options(&["-o", "b.svg", "a.png"]).output,
            Some(PathBuf::from("b.svg"))
        );
        assert!(run(&["-o", "b.svg", "a.png", "c.svg"]).is_err());
        assert!(run(&["a.png", "b.svg", "c.svg"]).is_err());
    }

    #[test]
    fn options_and_arguments_in_any_order() {
        let o = options(&["a.png", "-f", "out.svg", "--verbose"]);
        assert!(o.force && o.verbose);
        assert_eq!(o.output, Some(PathBuf::from("out.svg")));
        assert!(options(&["-fv", "a.png"]).force);
    }

    #[test]
    fn double_dash_ends_options() {
        let o = options(&["--", "-weird.png", "-out.svg"]);
        assert_eq!(o.input, PathBuf::from("-weird.png"));
        assert_eq!(o.output, Some(PathBuf::from("-out.svg")));
    }

    #[test]
    fn help_and_version() {
        assert_eq!(run(&["--help"]), Ok(Command::Help));
        assert_eq!(run(&["a.png", "-h"]), Ok(Command::Help));
        assert_eq!(run(&["-V"]), Ok(Command::Version));
        assert_eq!(run(&["--version"]), Ok(Command::Version));
    }

    #[test]
    fn usage_errors() {
        assert!(run(&[]).is_err());
        assert!(run(&["--nope", "a.png"]).is_err());
        assert!(run(&["-x", "a.png"]).is_err());
        assert!(run(&["--force=1", "a.png"]).is_err());
        assert!(run(&["-v", "-q", "a.png"]).is_err());
    }

    #[test]
    fn colors_and_precision_ranges() {
        assert_eq!(options(&["--colors", "16", "a.png"]).colors, Some(16));
        assert_eq!(options(&["--precision=0", "a.png"]).precision, Some(0));
        assert!(run(&["--colors", "1", "a.png"]).is_err());
        assert!(run(&["--colors", "257", "a.png"]).is_err());
        assert!(run(&["--precision", "4", "a.png"]).is_err());
    }

    #[test]
    fn help_text_mentions_every_option() {
        let help = help_text();
        for needle in [
            "--detail",
            "--force",
            "--output",
            "--verbose",
            "--quiet",
            "--colors",
            "--precision",
            "--remove-background",
            "--help",
            "--version",
        ] {
            assert!(help.contains(needle), "{needle}");
        }
    }
}
