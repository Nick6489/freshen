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
