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
