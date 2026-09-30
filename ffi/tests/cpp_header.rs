//! The generated header must be valid C++ and pleasant to use from C++ (Qt and Win32
//! applications). Compiles `tests/cpp/consumer.cpp` under strict warnings for several
//! language standards, then builds it against the real shared library and runs it
//! against signed fixtures, once plain and once under the sanitizers.
#![cfg(unix)]

mod support;

use std::{env, fs, path::Path, process::Command};

fn cxx() -> String {
    env::var("CXX").unwrap_or_else(|_| "c++".into())
}

fn compile(standard: &str, extra: &[&str], mode: &[&str], output: &Path) {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output_of = Command::new(cxx())
        .arg(format!("-std={standard}"))
        .args(["-Wall", "-Wextra", "-Werror", "-pedantic"])
        .args(extra)
        .arg("-I")
        .arg(manifest.join("include"))
        .arg(manifest.join("tests/cpp/consumer.cpp"))
        // Libraries must follow the object that needs them, or --as-needed drops them.
        .args(mode)
        .arg("-o")
        .arg(output)
        .output()
        .expect("a C++ compiler is required for the C++ consumer test");
    assert!(
        output_of.status.success(),
        "consumer.cpp failed to compile as {standard}:\n{}",
        String::from_utf8_lossy(&output_of.stderr)
    );
}

#[test]
fn header_is_valid_cpp_for_common_standards() {
    let temp = tempfile::tempdir().unwrap();
    // Older standards matter: Qt and long-lived Win32 code bases are often not on C++20.
    for standard in ["c++11", "c++14", "c++17", "c++20"] {
        compile(
            standard,
            &["-fsyntax-only"],
            &[],
            &temp.path().join("unused"),
        );
    }
}

fn run_consumer(standard: &str, sanitized: bool) {
    let library = support::library_dir();
    let temp = tempfile::tempdir().unwrap();
    let fixtures = temp.path().join("fixtures");
    fs::create_dir(&fixtures).unwrap();
    support::write_fixtures(&fixtures, b"cpp test application");
    let work = temp.path().join("work");
    fs::create_dir(&work).unwrap();

    let program = temp.path().join("consumer");
    let link = [
        "-L".to_string(),
        library.display().to_string(),
        "-lfreshen_ffi".to_string(),
        format!("-Wl,-rpath,{}", library.display()),
    ];
    let link: Vec<&str> = link.iter().map(String::as_str).collect();
    let mut mode: Vec<&str> = link.clone();
    if sanitized {
        mode.extend([
            "-fsanitize=address,undefined",
            "-fno-sanitize-recover=undefined",
            "-fno-omit-frame-pointer",
            "-g",
        ]);
    }
    compile(standard, &[], &mode, &program);

    let mut run = Command::new(&program);
    run.arg(&fixtures).arg(&work);
    if sanitized {
        run.env(
            "ASAN_OPTIONS",
            "detect_leaks=1:halt_on_error=1:abort_on_error=0",
        )
        .env("UBSAN_OPTIONS", "print_stacktrace=1:halt_on_error=1");
    }
    let output = run.output().unwrap();
    assert!(
        output.status.success(),
        "C++ consumer ({standard}, sanitized={sanitized}) failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // Dropping the prepared update at the end of the program removed its staged files.
    assert_eq!(fs::read_dir(&work).unwrap().count(), 0);
}

#[test]
fn cpp_consumer_runs_against_the_real_library() {
    run_consumer("c++17", false);
}

#[test]
fn cpp_consumer_runs_clean_under_sanitizers() {
    // Skips itself on toolchains that cannot run sanitized programs.
    let probe = tempfile::tempdir().unwrap();
    let source = probe.path().join("probe.cpp");
    fs::write(&source, "int main() { return 0; }\n").unwrap();
    let program = probe.path().join("probe");
    let usable = Command::new(cxx())
        .args(["-fsanitize=address,undefined"])
        .arg(&source)
        .arg("-o")
        .arg(&program)
        .status()
        .is_ok_and(|status| status.success())
        && Command::new(&program)
            .status()
            .is_ok_and(|status| status.success());
    if !usable {
        eprintln!("skipped: this C++ toolchain cannot build and run sanitized programs");
        return;
    }
    run_consumer("c++17", true);
}
