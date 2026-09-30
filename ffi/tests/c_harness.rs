//! Builds signed fixtures, compiles `tests/c/harness.c` against the real shared
//! library through the generated header, and runs the ABI checks plus the same
//! parent / helper / restarted-application scenarios as `tests/helper_process.rs`
//! (commit, cancel, failed startup rollback, unconfirmed timeout, busy lock) --
//! but driven entirely through the C ABI.
#![cfg(unix)]

mod support;

use freshen::{
    Error, InstallState, discard_finished_installation, installation_status, recover_installation,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn library_dir() -> PathBuf {
    let exe = env::current_exe().unwrap();
    let profile_dir = exe.parent().unwrap().parent().unwrap().to_owned();
    let target_dir = profile_dir.parent().unwrap().to_owned();
    let mut build = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    build
        .args(["build", "-p", "freshen-ffi", "--target-dir"])
        .arg(&target_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"));
    if profile_dir
        .file_name()
        .is_some_and(|name| name == "release")
    {
        build.arg("--release");
    }
    assert!(
        build.status().unwrap().success(),
        "building freshen-ffi failed"
    );
    profile_dir
}

fn compile_harness(library: &Path, output: &Path) {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let status = Command::new(env::var("CC").unwrap_or_else(|_| "cc".into()))
        .args([
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic",
            "-O1",
        ])
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

#[test]
fn c_abi_conformance_and_process_scenarios() {
    let library = library_dir();
    let temp = tempfile::tempdir().unwrap();
    let harness = temp.path().join("harness");
    compile_harness(&library, &harness);
    let fixtures = temp.path().join("fixtures");
    fs::create_dir(&fixtures).unwrap();
    support::write_fixtures(&fixtures, &fs::read(&harness).unwrap());

    let output = Command::new(&harness)
        .arg("selftest")
        .arg(&fixtures)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "selftest failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    for mode in ["success", "cancel", "fail", "timeout", "busy"] {
        let scenario = tempfile::tempdir_in(temp.path()).unwrap();
        let root = scenario.path().join("installation");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("application"), b"old application").unwrap();
        fs::write(root.join("user-settings"), b"keep me").unwrap();
        let output = Command::new(&harness)
            .arg("parent")
            .arg(&fixtures)
            .arg(&root)
            .arg(mode)
            .arg(&harness)
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
        let deadline = Instant::now() + Duration::from_secs(60);
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
