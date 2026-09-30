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
use freshen::{Artifact, PackageFile, PackageKind, ReleaseManifest, signing_message};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    process::Command,
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

pub fn manifest(version: &str, notes: &str, package: &[u8], application: &[u8]) -> Vec<u8> {
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

pub fn sign(document: &[u8]) -> String {
    STANDARD.encode(
        SigningKey::from_bytes(&KEY_SEED)
            .sign(&signing_message(document))
            .to_bytes(),
    )
}

/// Writes every fixture a test transport serves. `application` is the file
/// packaged as the new version's executable.
pub fn write_fixtures(dir: &Path, application: &[u8]) {
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
