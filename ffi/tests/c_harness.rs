//! Builds signed fixtures, compiles `tests/c/harness.c` against the real shared
//! library through the generated header, and runs the ABI checks plus the same
//! parent / helper / restarted-application scenarios as `tests/helper_process.rs`
//! (commit, cancel, failed startup rollback, unconfirmed timeout, busy lock) --
//! but driven entirely through the C ABI.
#![cfg(unix)]

use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use freshen::{
    Artifact, Error, InstallState, PackageFile, PackageKind, ReleaseManifest,
    discard_finished_installation, installation_status, recover_installation, signing_message,
};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

const KEY_SEED: [u8; 32] = [7; 32];

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

fn manifest(version: &str, notes: &str, package: &[u8], application: &[u8]) -> Vec<u8> {
    let document = ReleaseManifest {
        schema: 1,
        product: "ffi-test".into(),
        channel: "stable".into(),
        version: version.parse().unwrap(),
        notes: notes.into(),
        artifacts: vec![Artifact {
            target: "test-target".into(),
            kind: PackageKind::Files,
            url: "https://test.invalid/package".parse().unwrap(),
            sha256: hex::encode(Sha256::digest(package)),
            size: package.len() as u64,
            files: vec![PackageFile {
                path: "application".into(),
                sha256: hex::encode(Sha256::digest(application)),
                size: application.len() as u64,
                executable: true,
            }],
        }],
    };
    serde_json::to_vec(&document).unwrap()
}

fn sign(document: &[u8]) -> String {
    STANDARD.encode(
        SigningKey::from_bytes(&KEY_SEED)
            .sign(&signing_message(document))
            .to_bytes(),
    )
}

/// Writes every fixture the harness serves. `application` is the executable
/// packaged as the new version (the harness itself, so it can run as the
/// restarted application).
fn write_fixtures(dir: &Path, application: &[u8]) {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "application",
            zip::write::SimpleFileOptions::default().unix_permissions(0o755),
        )
        .unwrap();
    writer.write_all(application).unwrap();
    let package = writer.finish().unwrap().into_inner();
    fs::write(dir.join("package.zip"), &package).unwrap();

    for (version, notes) in [("1.5.0", "notes one"), ("2.0.0", "notes two")] {
        let document = manifest(version, notes, &package, application);
        fs::write(dir.join(format!("v{version}.sig")), sign(&document)).unwrap();
        fs::write(dir.join(format!("v{version}.manifest")), document).unwrap();
    }
    // A structurally valid signature over different bytes must not verify.
    fs::write(dir.join("tampered.sig"), sign(b"not the manifest")).unwrap();
    fs::write(
        dir.join("pubkey"),
        SigningKey::from_bytes(&KEY_SEED).verifying_key().to_bytes(),
    )
    .unwrap();
    let entry = |version: &str| {
        serde_json::json!({
            "document": format!("https://test.invalid/m/{version}"),
            "signature": format!("https://test.invalid/s/{version}"),
        })
    };
    fs::write(
        dir.join("index.json"),
        serde_json::to_vec(&[entry("1.5.0"), entry("2.0.0")]).unwrap(),
    )
    .unwrap();
    fs::write(dir.join("huge"), vec![0u8; 2 * 1024 * 1024]).unwrap();
}

#[test]
fn c_abi_conformance_and_process_scenarios() {
    let library = library_dir();
    let temp = tempfile::tempdir().unwrap();
    let harness = temp.path().join("harness");
    compile_harness(&library, &harness);
    let fixtures = temp.path().join("fixtures");
    fs::create_dir(&fixtures).unwrap();
    write_fixtures(&fixtures, &fs::read(&harness).unwrap());

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
