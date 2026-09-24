//! Optional shared-folder convergence checks. The host supplies its watcher or
//! timer and decides whether to stand down or restart. There is no unsigned
//! cross-machine "close this application" command in this protocol.
use crate::{
    Error, Result, SignedManifest, TrustStore,
    archive::hash_file,
    fsutil::{atomic_write, safe_join},
};
use semver::Version;
use std::{fs, path::Path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedStatus {
    Unchanged,
    WaitingForFiles {
        version: Version,
        pending_files: Vec<String>,
    },
    ReadyToRestart {
        version: Version,
    },
}

/// Publish a signed release into a directory already synchronized by the host.
/// Call after a successful installation; do not put this file in the signed ZIP.
pub fn publish(path: &Path, signed: &SignedManifest, trust: &TrustStore) -> Result<()> {
    trust.verify(signed)?;
    atomic_write(path, &serde_json::to_vec(signed)?)
}

/// Check *all* declared file hashes, rather than only the executable, before
/// declaring a synchronized installation ready. A signature failure is an error.
pub fn inspect(
    path: &Path,
    root: &Path,
    trust: &TrustStore,
    product: &str,
    channel: &str,
    current_version: &Version,
    target: &str,
) -> Result<SharedStatus> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SharedStatus::Unchanged);
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.len() > 4 * 1024 * 1024 {
        return Err(Error::SizeLimit);
    }
    let signed: SignedManifest = serde_json::from_slice(&fs::read(path)?)?;
    let release = trust.verify(&signed)?;
    if release.product != product
        || release.channel != channel
        || release.version <= *current_version
        || (channel == "stable" && !release.version.pre.is_empty())
    {
        return Ok(SharedStatus::Unchanged);
    }
    let Some(artifact) = release.artifacts.iter().find(|a| a.target == target) else {
        return Ok(SharedStatus::Unchanged);
    };
    let mut pending_files = Vec::new();
    for file in &artifact.files {
        let candidate = safe_join(root, &file.path)?;
        let matches = match fs::metadata(&candidate) {
            Ok(metadata) if metadata.is_file() && metadata.len() == file.size => {
                match hash_file(&candidate) {
                    Ok(hash) => hash.eq_ignore_ascii_case(&file.sha256),
                    Err(Error::Io(_)) => false, // sync clients can briefly lock files
                    Err(error) => return Err(error),
                }
            }
            _ => false,
        };
        if !matches {
            pending_files.push(file.path.clone());
        }
    }
    if pending_files.is_empty() {
        Ok(SharedStatus::ReadyToRestart {
            version: release.version,
        })
    } else {
        Ok(SharedStatus::WaitingForFiles {
            version: release.version,
            pending_files,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{manifest, signed, trust};
    #[test]
    fn metadata_arriving_before_the_program_does_not_trigger_restart() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("shared.json");
        publish(&state, &signed(&manifest()), &trust()).unwrap();
        let inspect_now = || {
            inspect(
                &state,
                temp.path(),
                &trust(),
                "test-app",
                "stable",
                &"1.0.0".parse().unwrap(),
                "test-target",
            )
            .unwrap()
        };
        assert!(matches!(
            inspect_now(),
            SharedStatus::WaitingForFiles { .. }
        ));
        fs::write(temp.path().join("app.exe"), b"old").unwrap();
        assert!(matches!(
            inspect_now(),
            SharedStatus::WaitingForFiles { .. }
        ));
        fs::write(temp.path().join("app.exe"), b"new").unwrap();
        assert!(matches!(inspect_now(), SharedStatus::ReadyToRestart { .. }));
    }
}
