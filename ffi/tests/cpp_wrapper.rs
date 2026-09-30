//! Tests the header-only C++ wrapper (`bindings/cpp/include/freshen/freshen.hpp`).
//!
//! Compiles `bindings/cpp/tests/wrapper_test.cpp` under strict warnings, builds it against
//! the real shared library, and runs its in-process checks plus the same
//! parent / helper / restarted-application scenarios as the C harness, plain and under
//! AddressSanitizer + UndefinedBehaviorSanitizer. Also proves the wrapper works in a
//! program built without exceptions.
#![cfg(unix)]

mod support;

use std::{env, fs, path::Path, process::Command};

const SANITIZER_FLAGS: [&str; 4] = [
    "-fsanitize=address,undefined",
    "-fno-sanitize-recover=undefined",
    "-fno-omit-frame-pointer",
    "-g",
];

fn cxx() -> String {
    env::var("CXX").unwrap_or_else(|_| "c++".into())
}

fn bindings_cpp() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../bindings/cpp")
}

/// Builds `source` (a file under bindings/cpp/tests) into `output`.
fn build(source: &str, standard: &str, flags: &[&str], library: Option<&Path>, output: &Path) {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cpp = bindings_cpp();
    let mut command = Command::new(cxx());
    command
        .arg(format!("-std={standard}"))
        .args(["-Wall", "-Wextra", "-Werror", "-pedantic", "-pthread"])
        .arg("-I")
        .arg(manifest.join("include"))
        .arg("-I")
        .arg(cpp.join("include"))
        .arg("-I")
        .arg(cpp.join("tests"))
        .arg(cpp.join("tests").join(source))
        .args(flags);
    if let Some(library) = library {
        // Libraries must follow the object that needs them, or --as-needed drops them.
        command
            .arg("-L")
            .arg(library)
            .arg("-lfreshen_ffi")
            .arg(format!("-Wl,-rpath,{}", library.display()));
    }
    let output_of = command
        .arg("-o")
        .arg(output)
        .output()
        .expect("a C++ compiler is required for the C++ wrapper tests");
    assert!(
        output_of.status.success(),
        "{source} failed to compile as {standard} ({flags:?}):\n{}",
        String::from_utf8_lossy(&output_of.stderr)
    );
}

fn sanitizers_available(scratch: &Path) -> bool {
    let source = scratch.join("probe.cpp");
    let program = scratch.join("probe");
    fs::write(&source, "int main() { return 0; }\n").unwrap();
    Command::new(cxx())
        .args(SANITIZER_FLAGS)
        .arg(&source)
        .arg("-o")
        .arg(&program)
        .status()
        .is_ok_and(|status| status.success())
        && Command::new(&program)
            .status()
            .is_ok_and(|status| status.success())
}

#[test]
fn wrapper_compiles_as_cpp17_and_cpp20() {
    let temp = tempfile::tempdir().unwrap();
    for standard in ["c++17", "c++20"] {
        build(
            "wrapper_test.cpp",
            standard,
            &["-fsyntax-only"],
            None,
            &temp.path().join("unused"),
        );
    }
}

#[test]
fn wrapper_conformance_and_process_scenarios() {
    let library = support::library_dir();
    let temp = tempfile::tempdir().unwrap();
    let program = temp.path().join("wrapper_test");
    build("wrapper_test.cpp", "c++17", &[], Some(&library), &program);
    support::exercise(&program, temp.path(), &[]);
}

#[test]
fn wrapper_under_address_and_undefined_behavior_sanitizers() {
    let library = support::library_dir();
    let temp = tempfile::tempdir().unwrap();
    if !sanitizers_available(temp.path()) {
        eprintln!("skipped: this C++ toolchain cannot build and run sanitized programs");
        return;
    }
    let program = temp.path().join("wrapper_test_sanitized");
    build(
        "wrapper_test.cpp",
        "c++17",
        &SANITIZER_FLAGS,
        Some(&library),
        &program,
    );
    // Any report aborts the process; leak detection is on.
    support::exercise(
        &program,
        temp.path(),
        &[
            (
                "ASAN_OPTIONS",
                "detect_leaks=1:halt_on_error=1:abort_on_error=0:strict_string_checks=1:detect_stack_use_after_return=1",
            ),
            ("UBSAN_OPTIONS", "print_stacktrace=1:halt_on_error=1"),
        ],
    );
}

#[test]
fn wrapper_works_in_a_program_built_without_exceptions() {
    let library = support::library_dir();
    let temp = tempfile::tempdir().unwrap();
    let program = temp.path().join("no_exceptions");
    build(
        "no_exceptions.cpp",
        "c++17",
        &["-fno-exceptions", "-DFRESHEN_NO_EXCEPTIONS"],
        Some(&library),
        &program,
    );
    let fixtures = temp.path().join("fixtures");
    fs::create_dir(&fixtures).unwrap();
    support::write_fixtures(&fixtures, b"cpp test application");
    let work = temp.path().join("work");
    fs::create_dir(&work).unwrap();

    let output = Command::new(&program)
        .arg(&fixtures)
        .arg(&work)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "no-exceptions consumer failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // Dropping the prepared update at the end of the program removed its staged files.
    assert_eq!(fs::read_dir(&work).unwrap().count(), 0);
}

/// Whether cmake and the Qt 6 development files (Core, Concurrent) are installed.
fn qt_available() -> bool {
    let tool = |program: &str, arguments: &[&str]| {
        Command::new(program)
            .args(arguments)
            .output()
            .is_ok_and(|output| output.status.success())
    };
    tool("cmake", &["--version"]) && tool("pkg-config", &["--exists", "Qt6Core", "Qt6Concurrent"])
}

/// freshen.hpp inside a real Qt 6 program: a QObject exposing the update flow as signals,
/// QtConcurrent workers, progress delivered on the GUI thread, and cancellation from the
/// GUI thread. Skipped when Qt is not installed, unless `FRESHEN_REQUIRE_QT` is set.
#[test]
fn wrapper_works_inside_a_qt6_program() {
    if !qt_available() {
        assert!(
            env::var_os("FRESHEN_REQUIRE_QT").is_none(),
            "FRESHEN_REQUIRE_QT is set but cmake or the Qt 6 development files are missing"
        );
        eprintln!(
            "skipped: cmake or the Qt 6 development files (Core, Concurrent) are not installed"
        );
        return;
    }
    let library = support::library_dir();
    let temp = tempfile::tempdir().unwrap();
    let source = bindings_cpp().join("tests/qt");
    let build_dir = temp.path().join("build");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));

    let mut configure = Command::new("cmake");
    configure
        .arg("-S")
        .arg(&source)
        .arg("-B")
        .arg(&build_dir)
        .arg(format!(
            "-DFRESHEN_C_INCLUDE={}",
            manifest.join("include").display()
        ))
        .arg(format!(
            "-DFRESHEN_CPP_INCLUDE={}",
            bindings_cpp().join("include").display()
        ))
        .arg(format!("-DFRESHEN_LIBRARY_DIR={}", library.display()));
    if Command::new("ninja")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        configure.args(["-G", "Ninja"]);
    }
    let configured = configure.output().unwrap();
    assert!(
        configured.status.success(),
        "cmake configure failed:\n{}\n{}",
        String::from_utf8_lossy(&configured.stdout),
        String::from_utf8_lossy(&configured.stderr)
    );
    let built = Command::new("cmake")
        .arg("--build")
        .arg(&build_dir)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "the Qt test failed to build:\n{}\n{}",
        String::from_utf8_lossy(&built.stdout),
        String::from_utf8_lossy(&built.stderr)
    );

    let fixtures = temp.path().join("fixtures");
    fs::create_dir(&fixtures).unwrap();
    support::write_fixtures(&fixtures, b"qt test application");
    let work = temp.path().join("work");
    fs::create_dir(&work).unwrap();
    let output = Command::new(build_dir.join("qt_test"))
        .arg(&fixtures)
        .arg(&work)
        .env("QT_QPA_PLATFORM", "offscreen")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "the Qt integration test failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // Every prepared update was dropped or cancelled, so nothing is left staged.
    assert_eq!(fs::read_dir(&work).unwrap().count(), 0);
}
