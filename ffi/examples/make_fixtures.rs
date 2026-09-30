//! Writes the signed test release fixtures used by the language-binding tests.
//!
//! ```text
//! cargo run -p freshen-ffi --example make_fixtures -- OUTPUT_DIR APPLICATION_FILE
//! ```
//!
//! `APPLICATION_FILE` is packaged as the new version's executable. The signing key
//! is a fixed, test-only key.

#[path = "../tests/support/mod.rs"]
mod support;

use std::{env, fs, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    let (Some(directory), Some(application), None) =
        (arguments.next(), arguments.next(), arguments.next())
    else {
        eprintln!("usage: make_fixtures OUTPUT_DIR APPLICATION_FILE");
        return ExitCode::from(64);
    };
    let directory = PathBuf::from(directory);
    let application = match fs::read(&application) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!(
                "cannot read {}: {error}",
                PathBuf::from(application).display()
            );
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = fs::create_dir_all(&directory) {
        eprintln!("cannot create {}: {error}", directory.display());
        return ExitCode::FAILURE;
    }
    support::write_fixtures(&directory, &application);
    ExitCode::SUCCESS
}
