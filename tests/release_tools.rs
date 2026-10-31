use base64::{Engine, engine::general_purpose::STANDARD};
use freshen::{SignedManifest, TrustStore};
use std::{fs, path::Path, process::Command};

fn run(directory: &Path, args: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_freshen-release"))
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn publisher_tools_create_verifiable_multi_target_releases() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("dist")).unwrap();
    fs::write(temp.path().join("dist/application.exe"), b"example program").unwrap();
    run(temp.path(), &["keygen", "publisher.key", "publisher.pub"]);
    for (target, archive, manifest) in [
        ("target-one", "one.zip", "one.json"),
        ("target-two", "two.zip", "two.json"),
    ] {
        run(
            temp.path(),
            &[
                "pack",
                "dist",
                archive,
                &format!("https://example.test/{archive}"),
                target,
                "files",
                "my-app",
                "1.2.3",
                "stable",
                manifest,
            ],
        );
    }
    run(
        temp.path(),
        &["merge", "freshen-manifest.json", "one.json", "two.json"],
    );
    run(
        temp.path(),
        &[
            "sign",
            "publisher.key",
            "freshen-manifest.json",
            "freshen-manifest.json.sig",
        ],
    );
    let key: [u8; 32] = STANDARD
        .decode(fs::read_to_string(temp.path().join("publisher.pub")).unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let trust = TrustStore::new(vec![key]).unwrap();
    let signed = SignedManifest {
        document: fs::read(temp.path().join("freshen-manifest.json")).unwrap(),
        signature: fs::read_to_string(temp.path().join("freshen-manifest.json.sig")).unwrap(),
    };
    let release = trust.verify(&signed).unwrap();
    assert_eq!(release.artifacts.len(), 2);
    assert_eq!(release.version.to_string(), "1.2.3");
    let duplicate = Command::new(env!("CARGO_BIN_EXE_freshen-release"))
        .current_dir(temp.path())
        .args(["keygen", "publisher.key", "publisher.pub"])
        .output()
        .unwrap();
    assert!(!duplicate.status.success());
    assert!(trust.verify(&signed).is_ok());
    run(
        temp.path(),
        &[
            "pack",
            "dist",
            "inno.zip",
            "https://example.test/inno.zip",
            "target-one",
            "inno",
            "my-app",
            "1.2.3",
            "stable",
            "inno.json",
        ],
    );
    run(
        temp.path(),
        &["merge", "mixed.json", "one.json", "inno.json"],
    );
    run(
        temp.path(),
        &["sign", "publisher.key", "mixed.json", "mixed.sig"],
    );
    let mixed = trust
        .verify(&SignedManifest {
            document: fs::read(temp.path().join("mixed.json")).unwrap(),
            signature: fs::read_to_string(temp.path().join("mixed.sig")).unwrap(),
        })
        .unwrap();
    assert_eq!(mixed.artifacts.len(), 2);
    assert_eq!(mixed.artifacts[0].target, mixed.artifacts[1].target);
    assert_ne!(mixed.artifacts[0].kind, mixed.artifacts[1].kind);
    let duplicate = Command::new(env!("CARGO_BIN_EXE_freshen-release"))
        .current_dir(temp.path())
        .args(["merge", "duplicate.json", "one.json", "one.json"])
        .status()
        .unwrap();
    assert!(!duplicate.success(), "same target/kind pair must not merge");
}

#[test]
fn direct_inno_tool_preserves_installer_and_signs_its_inventory() {
    use freshen::{PackageFormat, PackageKind, ReleaseManifest};
    let temp = tempfile::tempdir().unwrap();
    let installer = temp.path().join("Product-2.0.exe");
    fs::write(&installer, b"original installer bytes").unwrap();
    run(
        temp.path(),
        &[
            "pack-inno",
            "Product-2.0.exe",
            "https://example.test/download",
            "target-one",
            "my-app",
            "2.0.0",
            "stable",
            "inno.json",
        ],
    );
    let release: ReleaseManifest =
        serde_json::from_slice(&fs::read(temp.path().join("inno.json")).unwrap()).unwrap();
    release.validate().unwrap();
    let a = &release.artifacts[0];
    assert_eq!(a.kind, PackageKind::Inno);
    assert_eq!(a.format, PackageFormat::Raw);
    assert_eq!(a.files[0].path, "setup.exe");
    assert_eq!(a.files[0].sha256, a.sha256);
    assert_eq!(a.files[0].size, a.size);
    assert_eq!(fs::read(&installer).unwrap(), b"original installer bytes");
    assert_eq!(
        fs::read_dir(temp.path()).unwrap().count(),
        2,
        "no wrapper ZIP"
    );
    run(temp.path(), &["keygen", "key", "pub"]);
    run(temp.path(), &["sign", "key", "inno.json", "sig"]);
    let key: [u8; 32] = STANDARD
        .decode(fs::read_to_string(temp.path().join("pub")).unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let signed = SignedManifest {
        document: fs::read(temp.path().join("inno.json")).unwrap(),
        signature: fs::read_to_string(temp.path().join("sig")).unwrap(),
    };
    assert_eq!(
        TrustStore::new(vec![key])
            .unwrap()
            .verify(&signed)
            .unwrap()
            .artifacts[0]
            .format,
        PackageFormat::Raw
    );
    let before = fs::read(temp.path().join("inno.json")).unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_freshen-release"))
        .current_dir(temp.path())
        .args([
            "pack-inno",
            "Product-2.0.exe",
            "https://example.test/download",
            "target-one",
            "my-app",
            "2.0.0",
            "stable",
            "inno.json",
        ])
        .status()
        .unwrap();
    assert!(!status.success());
    assert_eq!(fs::read(temp.path().join("inno.json")).unwrap(), before);
    fs::write(temp.path().join("not-an-installer.txt"), b"bytes").unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_freshen-release"))
        .current_dir(temp.path())
        .args([
            "pack-inno",
            "not-an-installer.txt",
            "https://example.test/download",
            "target-one",
            "my-app",
            "2.0.0",
            "stable",
            "bad.json",
        ])
        .status()
        .unwrap();
    assert!(!status.success());
    assert!(!temp.path().join("bad.json").exists());
}
