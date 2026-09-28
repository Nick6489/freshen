use crate::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Write},
    sync::Arc,
};

pub(crate) fn signed(document: &ReleaseManifest) -> SignedManifest {
    let document = serde_json::to_vec(document).unwrap();
    let key = SigningKey::from_bytes(&[7; 32]);
    SignedManifest {
        signature: STANDARD.encode(key.sign(&signing_message(&document)).to_bytes()),
        document,
    }
}
pub(crate) fn trust() -> TrustStore {
    TrustStore::new(vec![
        SigningKey::from_bytes(&[7; 32]).verifying_key().to_bytes(),
    ])
    .unwrap()
}
pub(crate) fn manifest() -> ReleaseManifest {
    ReleaseManifest {
        schema: 1,
        product: "test-app".into(),
        channel: "stable".into(),
        version: "2.0.0".parse().unwrap(),
        notes: "Fix things".into(),
        artifacts: vec![Artifact {
            target: "test-target".into(),
            kind: PackageKind::Files,
            url: "https://example.test/package.zip".parse().unwrap(),
            sha256: "0".repeat(64),
            size: 1,
            files: vec![PackageFile {
                path: "app.exe".into(),
                sha256: hex::encode(Sha256::digest(b"new")),
                size: 3,
                executable: true,
            }],
        }],
    }
}

#[derive(Clone)]
struct MemoryTransport(Arc<BTreeMap<String, Vec<u8>>>);
impl Transport for MemoryTransport {
    fn download(
        &self,
        url: &url::Url,
        output: &mut dyn Write,
        limit: u64,
        cancel: &Cancellation,
        events: &mut dyn FnMut(Event),
    ) -> Result<u64> {
        cancel.check()?;
        let bytes = self
            .0
            .get(url.as_str())
            .ok_or_else(|| Error::Invalid("missing mock response".into()))?;
        if bytes.len() as u64 > limit {
            return Err(Error::SizeLimit);
        }
        output.write_all(bytes)?;
        events(Event::Downloading {
            received: bytes.len() as u64,
            total: Some(bytes.len() as u64),
        });
        Ok(bytes.len() as u64)
    }
}

fn fixture(zip_name: &str, bytes: &[u8]) -> (Updater<MemoryTransport>, ReleaseSource) {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(zip_name, zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(bytes).unwrap();
    let archive = zip.finish().unwrap().into_inner();
    let mut manifest = manifest();
    manifest.artifacts[0].size = archive.len() as u64;
    manifest.artifacts[0].sha256 = hex::encode(Sha256::digest(&archive));
    let signed = signed(&manifest);
    let transport = MemoryTransport(Arc::new(BTreeMap::from([
        ("https://example.test/manifest".into(), signed.document),
        (
            "https://example.test/signature".into(),
            signed.signature.into_bytes(),
        ),
        ("https://example.test/package.zip".into(), archive),
    ])));
    (
        Updater {
            product: "test-app".into(),
            channel: "stable".into(),
            current_version: "1.0.0".parse().unwrap(),
            target: "test-target".into(),
            trust: trust(),
            transport,
            max_download: 1024 * 1024,
            max_unpacked: 1024,
        },
        ReleaseSource::Manifest {
            document: "https://example.test/manifest".parse().unwrap(),
            signature: "https://example.test/signature".parse().unwrap(),
        },
    )
}

#[test]
fn signature_authenticates_exact_bytes() {
    let mut release = signed(&manifest());
    assert!(trust().verify(&release).is_ok());
    release.document.push(b' ');
    assert!(matches!(trust().verify(&release), Err(Error::Signature)));
}

#[test]
fn signature_from_another_key_is_rejected() {
    let unknown = TrustStore::new(vec![
        SigningKey::from_bytes(&[8; 32]).verifying_key().to_bytes(),
    ])
    .unwrap();
    assert!(matches!(
        unknown.verify(&signed(&manifest())),
        Err(Error::Signature)
    ));
}

#[test]
fn unsafe_portable_paths_and_aliases_are_rejected() {
    for path in [
        "../evil",
        "/evil",
        "a/../../evil",
        "C:/evil",
        "a\\evil",
        "file:stream",
        "NUL.txt",
        "COM1",
        "a.",
        "a ",
        "a//b",
        ".freshen/plan",
        "Å.exe",
    ] {
        assert!(crate::manifest::check_path(path).is_err(), "{path}");
    }
    let mut release = manifest();
    let mut duplicate = release.artifacts[0].files[0].clone();
    duplicate.path = "APP.exe".into();
    release.artifacts[0].files.push(duplicate);
    assert!(release.validate().is_err());
}

#[test]
fn product_channel_target_and_downgrade_are_filtered() {
    let (mut updater, source) = fixture("app.exe", b"new");
    for (product, channel, target, version) in [
        ("other", "stable", "test-target", "1.0.0"),
        ("test-app", "beta", "test-target", "1.0.0"),
        ("test-app", "stable", "other", "1.0.0"),
        ("test-app", "stable", "test-target", "2.0.0"),
        ("test-app", "stable", "test-target", "3.0.0"),
    ] {
        updater.product = product.into();
        updater.channel = channel.into();
        updater.target = target.into();
        updater.current_version = version.parse().unwrap();
        assert!(
            updater
                .check(&source, &Cancellation::default(), &mut |_| {})
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn preparation_is_verified_and_disposable() {
    let (updater, source) = fixture("app.exe", b"new");
    let temp = tempfile::tempdir().unwrap();
    let cancel = Cancellation::default();
    let candidate = updater
        .check(&source, &cancel, &mut |_| {})
        .unwrap()
        .unwrap();
    let prepared = updater
        .prepare(candidate, temp.path(), &cancel, &mut |_| {})
        .unwrap();
    let path = prepared.temporary.path().to_owned();
    assert_eq!(fs::read(path.join("stage/app.exe")).unwrap(), b"new");
    drop(prepared);
    assert!(!path.exists());
}

#[test]
fn signed_archive_with_traversal_is_rejected() {
    let (updater, source) = fixture("../escaped", b"new");
    let temp = tempfile::tempdir().unwrap();
    let cancel = Cancellation::default();
    let candidate = updater
        .check(&source, &cancel, &mut |_| {})
        .unwrap()
        .unwrap();
    assert!(matches!(
        updater.prepare(candidate, temp.path(), &cancel, &mut |_| {}),
        Err(Error::UnsafePath(_))
    ));
    assert!(!temp.path().join("escaped").exists());
}

#[test]
fn wrong_file_hash_is_rejected_even_in_a_signed_archive() {
    let (updater, source) = fixture("app.exe", b"bad");
    let temp = tempfile::tempdir().unwrap();
    let cancel = Cancellation::default();
    let candidate = updater
        .check(&source, &cancel, &mut |_| {})
        .unwrap()
        .unwrap();
    assert!(matches!(
        updater.prepare(candidate, temp.path(), &cancel, &mut |_| {}),
        Err(Error::HashMismatch(_))
    ));
}

#[test]
fn extraction_and_download_limits_and_cancellation_are_enforced() {
    let (mut updater, source) = fixture("app.exe", b"new");
    let temp = tempfile::tempdir().unwrap();
    let cancel = Cancellation::default();
    let candidate = updater
        .check(&source, &cancel, &mut |_| {})
        .unwrap()
        .unwrap();
    updater.max_unpacked = 2;
    assert!(matches!(
        updater.prepare(candidate.clone(), temp.path(), &cancel, &mut |_| {}),
        Err(Error::SizeLimit)
    ));
    updater.max_download = 1;
    assert!(matches!(
        updater.prepare(candidate, temp.path(), &cancel, &mut |_| {}),
        Err(Error::SizeLimit)
    ));
    cancel.cancel();
    assert!(matches!(
        updater.check(&source, &cancel, &mut |_| {}),
        Err(Error::Cancelled)
    ));
}

#[test]
fn transport_rejects_plain_http() {
    let transport = HttpTransport::new(std::time::Duration::from_secs(1)).unwrap();
    assert!(matches!(
        transport.download(
            &"http://localhost/".parse().unwrap(),
            &mut Vec::new(),
            10,
            &Cancellation::default(),
            &mut |_| {}
        ),
        Err(Error::Invalid(_))
    ));
}

fn history_fixture(releases: &[ReleaseManifest]) -> (Updater<MemoryTransport>, ReleaseSource) {
    let (mut updater, _) = fixture("app.exe", b"new");
    let mut responses = (*updater.transport.0).clone();
    let mut locations = Vec::new();
    let mut github = Vec::new();
    for (i, release) in releases.iter().enumerate() {
        let document = format!("https://example.test/{i}/manifest");
        let signature = format!("https://example.test/{i}/signature");
        let signed = signed(release);
        responses.insert(document.clone(), signed.document);
        responses.insert(signature.clone(), signed.signature.into_bytes());
        locations.push(serde_json::json!({"document": document, "signature": signature}));
        github.push(serde_json::json!({
            "draft": false, "prerelease": false,
            "assets": [
                {"name": "freshen-manifest.json", "browser_download_url": document},
                {"name": "freshen-manifest.json.sig", "browser_download_url": signature}
            ]
        }));
    }
    responses.insert(
        "https://example.test/index".into(),
        serde_json::to_vec(&locations).unwrap(),
    );
    responses.insert(
        "https://api.github.com/repos/test/app/releases?per_page=100".into(),
        serde_json::to_vec(&github).unwrap(),
    );
    updater.transport = MemoryTransport(Arc::new(responses));
    (
        updater,
        ReleaseSource::Index {
            document: "https://example.test/index".parse().unwrap(),
        },
    )
}

#[test]
fn history_is_authenticated_sorted_filtered_and_provider_independent() {
    let mut latest = manifest();
    latest.version = "3.0.0".parse().unwrap();
    latest.notes = "Latest changes".into();
    let mut intermediate = manifest();
    intermediate.notes.clear();
    let mut installed = manifest();
    installed.version = "1.0.0".parse().unwrap();
    let mut wrong_product = manifest();
    wrong_product.product = "another-app".into();
    let mut wrong_channel = manifest();
    wrong_channel.channel = "beta".into();
    let mut wrong_target = manifest();
    wrong_target.version = "9.0.0".parse().unwrap();
    wrong_target.artifacts[0].target = "other-target".into();
    let mut prerelease = manifest();
    prerelease.version = "4.0.0-beta.1".parse().unwrap();
    let (updater, index) = history_fixture(&[
        latest.clone(),
        wrong_product,
        intermediate.clone(),
        installed,
        wrong_channel,
        wrong_target,
        prerelease,
        intermediate,
    ]);
    let locations = serde_json::from_slice(
        updater
            .transport
            .0
            .get("https://example.test/index")
            .unwrap(),
    )
    .unwrap();
    for source in [
        index,
        ReleaseSource::Manifests(locations),
        ReleaseSource::GitHub {
            owner: "test".into(),
            repository: "app".into(),
        },
    ] {
        let candidate = updater
            .check(&source, &Cancellation::default(), &mut |_| {})
            .unwrap()
            .unwrap();
        assert_eq!(candidate.release().version, latest.version);
        assert_eq!(
            candidate.release_history(),
            &[
                ReleaseNotes {
                    version: "2.0.0".parse().unwrap(),
                    notes: String::new()
                },
                ReleaseNotes {
                    version: latest.version.clone(),
                    notes: latest.notes.clone()
                },
            ]
        );
        assert_eq!(
            trust().verify(candidate.signed_manifest()).unwrap().version,
            latest.version
        );
    }
}

#[test]
fn history_failures_never_return_a_partial_candidate() {
    let mut latest = manifest();
    latest.version = "3.0.0".parse().unwrap();
    let (updater, source) = history_fixture(&[latest.clone(), manifest()]);
    for failure in ["signature", "missing", "oversize"] {
        let (mut broken, _) = history_fixture(&[latest.clone(), manifest()]);
        let responses = Arc::make_mut(&mut broken.transport.0);
        match failure {
            "signature" => {
                responses.insert(
                    "https://example.test/1/signature".into(),
                    b"invalid".to_vec(),
                );
            }
            "missing" => {
                responses.remove("https://example.test/1/manifest");
            }
            _ => {
                responses.insert(
                    "https://example.test/1/manifest".into(),
                    vec![b' '; 1024 * 1024 + 1],
                );
            }
        }
        assert!(
            broken
                .check(&source, &Cancellation::default(), &mut |_| {})
                .is_err()
        );
    }
    assert_eq!(
        updater
            .check(&source, &Cancellation::default(), &mut |_| {})
            .unwrap()
            .unwrap()
            .release_history()
            .len(),
        2
    );
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(matches!(
        updater.check(&source, &cancel, &mut |_| {}),
        Err(Error::Cancelled)
    ));
}

#[test]
fn index_limits_and_conflicting_history_are_rejected() {
    let (mut updater, source) = history_fixture(&[]);
    assert!(
        updater
            .check(&source, &Cancellation::default(), &mut |_| {})
            .unwrap()
            .is_none()
    );
    for content in [
        b"not JSON".to_vec(),
        br#"[{"document":"relative.json","signature":"relative.sig"}]"#.to_vec(),
        serde_json::to_vec(&vec![serde_json::json!({"document":"https://example.test/m", "signature":"https://example.test/s"}); 101]).unwrap(),
        vec![b' '; 1024 * 1024 + 1],
    ] {
        Arc::make_mut(&mut updater.transport.0).insert("https://example.test/index".into(), content);
        assert!(updater.check(&source, &Cancellation::default(), &mut |_| {}).is_err());
    }
    let mut conflicting = manifest();
    conflicting.notes = "Different notes".into();
    let (updater, source) = history_fixture(&[manifest(), conflicting]);
    assert!(matches!(
        updater.check(&source, &Cancellation::default(), &mut |_| {}),
        Err(Error::Invalid(_))
    ));
    let locations = vec![
        ManifestLocation {
            document: "https://example.test/m".parse().unwrap(),
            signature: "https://example.test/s".parse().unwrap()
        };
        101
    ];
    assert!(matches!(
        updater.check(
            &ReleaseSource::Manifests(locations),
            &Cancellation::default(),
            &mut |_| {}
        ),
        Err(Error::SizeLimit)
    ));
}

#[test]
fn single_manifest_history_survives_preparation() {
    let (mut updater, source) = fixture("app.exe", b"new");
    let cancel = Cancellation::default();
    let candidate = updater
        .check(&source, &cancel, &mut |_| {})
        .unwrap()
        .unwrap();
    let history = candidate.release_history().to_vec();
    assert_eq!(history.len(), 1);
    let directory = tempfile::tempdir().unwrap();
    let prepared = updater
        .prepare(candidate, directory.path(), &cancel, &mut |_| {})
        .unwrap();
    assert_eq!(prepared.release_history(), history);
    updater.current_version = "2.0.0".parse().unwrap();
    assert!(
        updater
            .check(&source, &cancel, &mut |_| {})
            .unwrap()
            .is_none()
    );
}

#[test]
fn nonstable_history_includes_prereleases_and_respects_current_version() {
    let mut older = manifest();
    older.channel = "beta".into();
    older.version = "2.0.0-beta.1".parse().unwrap();
    let mut newer = older.clone();
    newer.version = "2.0.0-beta.2".parse().unwrap();
    let (mut updater, source) = history_fixture(&[newer.clone(), older.clone()]);
    updater.channel = "beta".into();
    updater.current_version = older.version;
    let candidate = updater
        .check(&source, &Cancellation::default(), &mut |_| {})
        .unwrap()
        .unwrap();
    assert_eq!(
        candidate.release_history(),
        &[ReleaseNotes {
            version: newer.version,
            notes: newer.notes
        }]
    );
}

#[test]
fn github_discovery_excludes_drafts_prereleases_and_unsigned_releases() {
    let (mut updater, _) = history_fixture(&[manifest()]);
    let url = "https://api.github.com/repos/test/app/releases?per_page=100";
    let original: Vec<serde_json::Value> =
        serde_json::from_slice(updater.transport.0.get(url).unwrap()).unwrap();
    for field in ["draft", "prerelease", "assets"] {
        let mut releases = original.clone();
        releases[0][field] = if field == "assets" {
            serde_json::json!([])
        } else {
            serde_json::json!(true)
        };
        Arc::make_mut(&mut updater.transport.0)
            .insert(url.into(), serde_json::to_vec(&releases).unwrap());
        let source = ReleaseSource::GitHub {
            owner: "test".into(),
            repository: "app".into(),
        };
        assert!(
            updater
                .check(&source, &Cancellation::default(), &mut |_| {})
                .unwrap()
                .is_none()
        );
    }
}
