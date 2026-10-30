//! Optional shared-folder convergence checks. The host supplies its watcher or
//! timer and decides whether to stand down or restart. There is no unsigned
//! cross-machine "close this application" command in this protocol.
use crate::{
    Error, PackageKind, Result, SignedManifest, TrustStore,
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
    inspect_selected(
        path,
        root,
        trust,
        product,
        channel,
        current_version,
        (target, None),
    )
}

/// Inspect one explicitly selected `(target, kind)` from shared signed metadata.
/// Only file/bundle inventories describe installed files. Inno's installer
/// inventory cannot prove convergence of the installed application and is refused.
pub fn inspect_for_kind(
    path: &Path,
    root: &Path,
    trust: &TrustStore,
    product: &str,
    channel: &str,
    current_version: &Version,
    distribution: (&str, PackageKind),
) -> Result<SharedStatus> {
    inspect_selected(
        path,
        root,
        trust,
        product,
        channel,
        current_version,
        (distribution.0, Some(distribution.1)),
    )
}

fn inspect_selected(
    path: &Path,
    root: &Path,
    trust: &TrustStore,
    product: &str,
    channel: &str,
    current_version: &Version,
    distribution: (&str, Option<PackageKind>),
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
    let mut artifacts = release
        .artifacts
        .iter()
        .filter(|a| a.target == distribution.0 && distribution.1.is_none_or(|kind| a.kind == kind));
    let Some(artifact) = artifacts.next() else {
        return Ok(SharedStatus::Unchanged);
    };
    if artifacts.next().is_some() {
        return Err(Error::Invalid(
            "ambiguous shared target; select a file/bundle kind with inspect_for_kind".into(),
        ));
    }
    if artifact.kind == PackageKind::Inno {
        return Err(Error::Invalid(
            "an Inno payload inventory cannot prove installed-file convergence".into(),
        ));
    }
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
    fn mixed_shared_metadata_requires_kind_and_installer_bytes_never_prove_convergence() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("shared.json");
        let mut release = manifest();
        let mut installer = release.artifacts[0].clone();
        installer.kind = PackageKind::Inno;
        installer.files[0].path = "setup.exe".into();
        release.artifacts.insert(0, installer);
        publish(&path, &signed(&release), &trust()).unwrap();
        let current = "1.0.0".parse().unwrap();
        assert!(matches!(
            inspect(
                &path,
                root.path(),
                &trust(),
                "test-app",
                "stable",
                &current,
                "test-target"
            ),
            Err(Error::Invalid(_))
        ));
        fs::write(root.path().join("setup.exe"), b"new").unwrap();
        assert!(matches!(
            inspect_for_kind(
                &path,
                root.path(),
                &trust(),
                "test-app",
                "stable",
                &current,
                ("test-target", PackageKind::Inno)
            ),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            inspect_for_kind(
                &path,
                root.path(),
                &trust(),
                "test-app",
                "stable",
                &current,
                ("test-target", PackageKind::Files)
            )
            .unwrap(),
            SharedStatus::WaitingForFiles { .. }
        ));
        fs::write(root.path().join("app.exe"), b"new").unwrap();
        assert!(matches!(
            inspect_for_kind(
                &path,
                root.path(),
                &trust(),
                "test-app",
                "stable",
                &current,
                ("test-target", PackageKind::Files)
            )
            .unwrap(),
            SharedStatus::ReadyToRestart { .. }
        ));
    }
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
