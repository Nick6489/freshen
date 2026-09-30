//! Builds signed fixtures, compiles `tests/c/harness.c` against the real shared
//! library through the generated header, and runs the ABI checks plus the same
//! parent / helper / restarted-application scenarios as `tests/helper_process.rs`
//! (commit, cancel, failed startup rollback, unconfirmed timeout, busy lock) --
//! but driven entirely through the C ABI.
//!
//! The same scenarios run a second time with the C side built under AddressSanitizer
//! and UndefinedBehaviorSanitizer, which catches memory misuse across the C/Rust
//! boundary: use after free, double free, freeing memory with the wrong allocator,
//! leaks, out-of-bounds reads of the structs and buffers the API hands out, and
//! undefined behavior in the C code.
#![cfg(unix)]

mod support;

use std::{env, fs, path::Path, process::Command};

const SANITIZER_FLAGS: [&str; 4] = [
    "-fsanitize=address,undefined",
    "-fno-sanitize-recover=undefined",
    "-fno-omit-frame-pointer",
    "-g",
];

fn cc() -> String {
    env::var("CC").unwrap_or_else(|_| "cc".into())
}

fn compile_harness(library: &Path, output: &Path, extra_flags: &[&str]) {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let status = Command::new(cc())
        .args([
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic",
            "-O1",
        ])
        .args(extra_flags)
        .arg("-I")
        .arg(manifest.join("include"))
        .arg(manifest.join("tests/c/harness.c"))
        .arg("-L")
        .arg(library)
        .arg("-lfreshen_ffi")
        .arg(format!("-Wl,-rpath,{}", library.display()))
        .arg("-o")
        .arg(output)
        .status()
        .expect("a C compiler is required for the C ABI tests");
    assert!(
        status.success(),
        "harness.c failed to compile against include/freshen.h"
    );
}

/// Whether this toolchain can build and run a sanitized program at all.
fn sanitizers_available(scratch: &Path) -> bool {
    let source = scratch.join("probe.c");
    let program = scratch.join("probe");
    fs::write(&source, "int main(void) { return 0; }\n").unwrap();
    Command::new(cc())
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
fn c_abi_conformance_and_process_scenarios() {
    let library = support::library_dir();
    let temp = tempfile::tempdir().unwrap();
    let harness = temp.path().join("harness");
    compile_harness(&library, &harness, &[]);
    support::exercise(&harness, temp.path(), &[]);
}

#[test]
fn c_abi_under_address_and_undefined_behavior_sanitizers() {
    let library = support::library_dir();
    let temp = tempfile::tempdir().unwrap();
    if !sanitizers_available(temp.path()) {
        eprintln!("skipped: this C toolchain cannot build and run sanitized programs");
        return;
    }
    let harness = temp.path().join("harness-sanitized");
    compile_harness(&library, &harness, &SANITIZER_FLAGS);
    // Any sanitizer report aborts the process with a nonzero status, which fails
    // the scenario. Leak detection is on, so a leak in either the C harness or the
    // library through the API is a failure too.
    support::exercise(
        &harness,
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
