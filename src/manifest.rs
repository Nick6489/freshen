use std::collections::{BTreeMap, BTreeSet};

use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, VerifyingKey};
use semver::Version;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{Error, Result};

/// Prefix included in signatures to keep Freshen signatures domain separated.
pub const SIGNING_DOMAIN: &[u8] = b"freshen-manifest-v1\0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackageKind {
    Files,
    MacBundle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageFile {
    /// Portable relative path using forward slashes. Symlinks are not supported.
    pub path: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub executable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    /// Exact Rust target triple. The host selects its distribution target explicitly.
    pub target: String,
    pub kind: PackageKind,
    pub url: Url,
    pub sha256: String,
    pub size: u64,
    /// Complete list of regular files inside the ZIP, including bundle prefixes.
    pub files: Vec<PackageFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    pub schema: u32,
    pub product: String,
    pub channel: String,
    pub version: Version,
    #[serde(default)]
    pub notes: String,
    pub artifacts: Vec<Artifact>,
}

impl ReleaseManifest {
    pub fn validate(&self) -> Result<()> {
        if self.schema != 1 || self.product.is_empty() || self.channel.is_empty() {
            return Err(Error::Invalid("schema, product, or channel".into()));
        }
        let mut targets = BTreeSet::new();
        for artifact in &self.artifacts {
            if artifact.target.is_empty() || !targets.insert(&artifact.target) {
                return Err(Error::Invalid("empty or duplicate target".into()));
            }
            check_hash(&artifact.sha256)?;
            if artifact.size == 0 || artifact.files.is_empty() || artifact.files.len() > 100_000 {
                return Err(Error::Invalid(
                    "empty or excessively large package inventory".into(),
                ));
            }
            let mut paths = BTreeSet::new();
            for file in &artifact.files {
                check_path(&file.path)?;
                check_hash(&file.sha256)?;
                if !paths.insert(file.path.to_ascii_lowercase()) {
                    return Err(Error::Invalid(
                        "case-insensitive duplicate file path".into(),
                    ));
                }
            }
            for path in &paths {
                let mut parent = path.as_str();
                while let Some((prefix, _)) = parent.rsplit_once('/') {
                    if paths.contains(prefix) {
                        return Err(Error::Invalid("file is also an ancestor directory".into()));
                    }
                    parent = prefix;
                }
            }
        }
        Ok(())
    }
}

/// Exact JSON bytes and a base64 detached Ed25519 signature. Do not reserialize
/// JSON between signing and verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedManifest {
    pub document: Vec<u8>,
    pub signature: String,
}

/// Application-supplied public keys. Keeping old and new keys permits rotation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustStore {
    keys: Vec<[u8; 32]>,
}

impl TrustStore {
    pub fn new(keys: Vec<[u8; 32]>) -> Result<Self> {
        if keys.is_empty() || keys.len() > 32 {
            return Err(Error::Invalid(
                "provide between 1 and 32 publisher keys".into(),
            ));
        }
        for bytes in &keys {
            let key = VerifyingKey::from_bytes(bytes).map_err(|_| Error::Signature)?;
            if key.is_weak() {
                return Err(Error::Signature);
            }
        }
        Ok(Self { keys })
    }

    pub fn verify(&self, signed: &SignedManifest) -> Result<ReleaseManifest> {
        if signed.document.len() > 1024 * 1024 || signed.signature.len() > 1024 {
            return Err(Error::SizeLimit);
        }
        let decoded = STANDARD
            .decode(signed.signature.trim())
            .map_err(|_| Error::Signature)?;
        let signature = Signature::from_slice(&decoded).map_err(|_| Error::Signature)?;
        let message = signing_message(&signed.document);
        let verified = self.keys.iter().any(|bytes| {
            VerifyingKey::from_bytes(bytes)
                .is_ok_and(|key| key.verify_strict(&message, &signature).is_ok())
        });
        if !verified {
            return Err(Error::Signature);
        }
        let manifest: ReleaseManifest = serde_json::from_slice(&signed.document)?;
        manifest.validate()?;
        Ok(manifest)
    }
}

pub fn signing_message(document: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(SIGNING_DOMAIN.len() + document.len());
    message.extend_from_slice(SIGNING_DOMAIN);
    message.extend_from_slice(document);
    message
}

pub(crate) fn check_hash(value: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::Invalid(
            "SHA-256 must be 64 hexadecimal characters".into(),
        ));
    }
    Ok(())
}

/// Use one conservative path grammar on all platforms to reject drive names,
/// alternate streams, Windows device names, and filesystem aliases everywhere.
pub(crate) fn check_path(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 4096 || !value.is_ascii() {
        return Err(Error::UnsafePath(value.into()));
    }
    for part in value.split('/') {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'));
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.len() > 255
            || part.ends_with(['.', ' '])
            || part.eq_ignore_ascii_case(".freshen")
            || part.bytes().any(|c| c < 32 || b"\\:<>\"|?*".contains(&c))
            || device
        {
            return Err(Error::UnsafePath(value.into()));
        }
    }
    Ok(())
}

pub(crate) fn inventory(artifact: &Artifact) -> BTreeMap<&str, &PackageFile> {
    artifact
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect()
}
