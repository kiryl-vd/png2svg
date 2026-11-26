//! Entry point of the `png2svg` program.
//!
//! This file only connects the parts: it reads the arguments, loads the file,
//! runs the conversion, saves the result and turns every problem into a short
//! message and an exit code (0 = success, 1 = runtime error, 2 = usage error).

#![forbid(unsafe_code)]

use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use png2svg::cli::{self, Command, Options, PROGRAM};
use png2svg::output_path;
use png2svg::pipeline::{convert, ConvertOptions};
use png2svg::png;

fn main() -> ExitCode {
    // A panic must never reach the user as a stack trace (requirement CLI-04).
    // The default message is switched off, and the panic is caught below.
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(real_main);
    match result {
        Ok(code) => code,
        Err(_) => {
            eprintln!("{PROGRAM}: internal error, the conversion was stopped");
            ExitCode::from(1)
        }
    }
}

/// Runs the program and returns the exit code.
fn real_main() -> ExitCode {
    match cli::parse(std::env::args_os().skip(1)) {
        Ok(Command::Help) => {
            print_stdout(&cli::help_text());
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            print_stdout(&format!("{}\n", cli::version_text()));
            ExitCode::SUCCESS
        }
        Ok(Command::Run(options)) => match run(&options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("{PROGRAM}: error: {message}");
                ExitCode::from(1)
            }
        },
        Err(error) => {
            eprintln!("{PROGRAM}: {error}");
            eprintln!("Try '{PROGRAM} --help' for more information.");
            ExitCode::from(2)
        }
    }
}

/// Writes text to stdout. A closed pipe (for example `| head`) is not an error.
fn print_stdout(text: &str) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

/// Converts one file.
fn run(options: &Options) -> Result<(), String> {
    let input = &options.input;
    let target = match &options.output {
        Some(path) => path.clone(),
        None => output_path::default_output(input),
    };

    let bytes = fs::read(input).map_err(|e| read_error(input, &e))?;
    // Check the result path before the slow work starts (OUT-14, 15, 16).
    output_path::validate_target(&target, input)?;
    let image = png::decode(&bytes).map_err(|e| format!("{}: {e}", input.display()))?;
    drop(bytes);

    let (svg, stats) = convert(
        &image,
        &ConvertOptions {
            detail: options.detail,
            colors: options.colors,
            precision: options.precision,
            remove_background: options.remove_background,
        },
    );
    drop(image);

    let written = output_path::write_output(&target, input, options.force, svg.as_bytes())?;

    if options.verbose {
        eprint!("{}", stats.report());
    }
    if !options.quiet {
        // The final path is printed, because it can have a suffix (CLI-01).
        print_stdout(&format!("{}\n", written.display()));
    }
    Ok(())
}

/// Builds a clear message for a file that cannot be read.
fn read_error(path: &Path, error: &io::Error) -> String {
    let reason = match error.kind() {
        io::ErrorKind::NotFound => "file not found".to_string(),
        io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        _ => error.to_string(),
    };
    format!("{}: {reason}", path.display())
}
