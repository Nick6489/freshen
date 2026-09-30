//! Signed release fixtures shared by the C harness test and the example programs
//! that language bindings use to build their own tests.
//!
//! Layout written by [`write_fixtures`] (all served under `https://test.invalid/`
//! by the test transports): two signed releases (1.5.0 and 2.0.0) for product
//! `ffi-test`, channel `stable`, target `test-target`; an index listing both; the
//! package archive; a signature over the wrong bytes; a 2 MiB `huge` file; and the
//! raw public key.
#![allow(
    dead_code,
    reason = "each consumer of this module uses a different subset"
)]

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

/// Seed of the fixture signing key. Test-only; never used for real releases.
pub const KEY_SEED: [u8; 32] = [7; 32];

/// Builds the freshen-ffi library with cargo (a no-op when it is current) and returns
/// the profile directory that holds `libfreshen_ffi.*`. Call it from an integration
/// test: it derives the target directory and profile from the test executable's path.
pub fn library_dir() -> PathBuf {
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

/// The name the packaged application is installed under. Windows cannot launch a file
/// without an extension, so a Windows test that restarts the installed application must
/// use a name ending in `.exe`; everywhere else the plain name is used.
pub const APPLICATION_NAME: &str = if cfg!(windows) {
    "application.exe"
} else {
    "application"
};

pub fn manifest(
    version: &str,
    notes: &str,
    package: &[u8],
    application: &[u8],
    name: &str,
) -> Vec<u8> {
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
                path: name.into(),
                sha256: hex::encode(Sha256::digest(application)),
                size: application.len() as u64,
                executable: true,
            }],
        }],
    };
    serde_json::to_vec(&document).unwrap()
}

pub fn sign(document: &[u8]) -> String {
    STANDARD.encode(
        SigningKey::from_bytes(&KEY_SEED)
            .sign(&signing_message(document))
            .to_bytes(),
    )
}

/// Writes every fixture a test transport serves. `application` is the file
/// packaged as the new version's executable, named [`APPLICATION_NAME`].
pub fn write_fixtures(dir: &Path, application: &[u8]) {
    write_fixtures_named(dir, application, APPLICATION_NAME);
}

/// Like [`write_fixtures`] with an explicit installed file name. The name is also written
/// to a small `appname` file so a test program that only has the fixtures directory (a
/// separate process, in another language) can find out what it is.
pub fn write_fixtures_named(dir: &Path, application: &[u8], name: &str) {
    fs::write(dir.join("appname"), name).unwrap();
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            name,
            zip::write::SimpleFileOptions::default().unix_permissions(0o755),
        )
        .unwrap();
    writer.write_all(application).unwrap();
    let package = writer.finish().unwrap().into_inner();
    fs::write(dir.join("package.zip"), &package).unwrap();

    for (version, notes) in [("1.5.0", "notes one"), ("2.0.0", "notes two")] {
        let document = manifest(version, notes, &package, application, name);
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

/// Runs the in-process checks and every update scenario with `harness` as parent,
/// helper and restarted application. `environment` is passed to every process.
pub fn exercise(harness: &Path, scratch: &Path, environment: &[(&str, &str)]) {
    let fixtures = scratch.join("fixtures");
    fs::create_dir(&fixtures).unwrap();
    write_fixtures(&fixtures, &fs::read(harness).unwrap());

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
