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

use freshen::{
    Error, InstallState, discard_finished_installation, installation_status, recover_installation,
};
use std::{
    env, fs,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};

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

/// Runs the in-process checks and every update scenario with `harness` as parent,
/// helper and restarted application. `environment` is passed to every process.
fn exercise(harness: &Path, scratch: &Path, environment: &[(&str, &str)]) {
    let fixtures = scratch.join("fixtures");
    fs::create_dir(&fixtures).unwrap();
    support::write_fixtures(&fixtures, &fs::read(harness).unwrap());

    let output = Command::new(harness)
        .arg("selftest")
        .arg(&fixtures)
        .envs(environment.iter().copied())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "selftest failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    for mode in ["success", "cancel", "fail", "timeout", "busy"] {
        let scenario = tempfile::tempdir_in(scratch).unwrap();
        let root = scenario.path().join("installation");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("application"), b"old application").unwrap();
        fs::write(root.join("user-settings"), b"keep me").unwrap();
        let output = Command::new(harness)
            .arg("parent")
            .arg(&fixtures)
            .arg(&root)
            .arg(mode)
            .arg(harness)
            .envs(environment.iter().copied())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{mode}: parent failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let expected = match mode {
            "success" => InstallState::Committed,
            "cancel" | "busy" => InstallState::Prepared,
            "fail" => InstallState::RolledBack,
            _ => InstallState::Failed,
        };
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let status = installation_status(&root).unwrap().unwrap();
            if status.state == expected {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{mode}: timed out with {status:?}; {:?}",
                fs::read_to_string(root.join(".freshen/transaction/helper-error.txt"))
            );
            thread::sleep(Duration::from_millis(100));
        }
        if mode == "timeout" {
            // An unconfirmed but running process is left alone; wait for the
            // fixture to exit, then recovery must roll back.
            thread::sleep(Duration::from_secs(3));
            assert_eq!(
                recover_installation(&root).unwrap().state,
                InstallState::RolledBack
            );
        }
        assert_eq!(fs::read(root.join("user-settings")).unwrap(), b"keep me");
        if mode == "success" {
            assert_ne!(
                fs::read(root.join("application")).unwrap(),
                b"old application"
            );
        } else {
            assert_eq!(
                fs::read(root.join("application")).unwrap(),
                b"old application",
                "{mode}: the old application must be intact"
            );
        }
        // A committed status may be observed just before the helper drops its lock.
        loop {
            match discard_finished_installation(&root) {
                Ok(()) => break,
                Err(Error::Busy) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(100))
                }
                result => panic!("{mode}: cleanup failed: {result:?}"),
            }
        }
    }
}

#[test]
fn c_abi_conformance_and_process_scenarios() {
    let library = support::library_dir();
    let temp = tempfile::tempdir().unwrap();
    let harness = temp.path().join("harness");
    compile_harness(&library, &harness, &[]);
    exercise(&harness, temp.path(), &[]);
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
    exercise(
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
