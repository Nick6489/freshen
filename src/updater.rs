use crate::{
    Artifact, Cancellation, Error, Event, ReleaseManifest, Result, SignedManifest, Transport,
    TrustStore,
    archive::{extract, hash_file},
    transport::fetch,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    path::Path,
};
use tempfile::TempDir;
use url::Url;

/// Location of a manifest and its detached signature, independent of hosting provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestLocation {
    pub document: Url,
    pub signature: Url,
}

/// Authenticated notes for one applicable release discovered during a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseNotes {
    pub version: Version,
    pub notes: String,
}

pub enum ReleaseSource {
    /// Locations supplied by the host's discovery mechanism (at most 100).
    Manifests(Vec<ManifestLocation>),
    /// URL of a JSON array of `ManifestLocation` objects (at most 100).
    /// Locations must be absolute URLs. The index only locates manifests;
    /// every manifest is authenticated separately. No recursive links.
    Index {
        document: Url,
    },
    Manifest {
        document: Url,
        signature: Url,
    },
    /// Searches the first 100 releases; release metadata itself is not trusted.
    GitHub {
        owner: String,
        repository: String,
    },
}

pub struct Updater<T: Transport> {
    pub product: String,
    pub channel: String,
    pub current_version: Version,
    pub target: String,
    pub trust: TrustStore,
    pub transport: T,
    pub max_download: u64,
    pub max_unpacked: u64,
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub(crate) signed: SignedManifest,
    pub(crate) release: ReleaseManifest,
    pub(crate) artifact: Artifact,
    pub(crate) history: Vec<ReleaseNotes>,
}
impl Candidate {
    pub fn signed_manifest(&self) -> &SignedManifest {
        &self.signed
    }
    pub fn release(&self) -> &ReleaseManifest {
        &self.release
    }
    /// Available authenticated history, oldest first, including the selected release.
    /// Entries are newer than the configured current version and match its product,
    /// channel and target policy. Missing manifests and discovery limits can leave
    /// gaps; completeness is not claimed. Empty notes are preserved for the host
    /// to provide a localized fallback.
    pub fn release_history(&self) -> &[ReleaseNotes] {
        &self.history
    }
    pub fn artifact(&self) -> &Artifact {
        &self.artifact
    }
}

/// Owns temporary files. Dropping a prepared update discards it without changing
/// the installed application. Installation is a separate explicit operation.
pub struct PreparedUpdate {
    pub(crate) temporary: TempDir,
    pub(crate) candidate: Candidate,
    pub(crate) trust: TrustStore,
}
impl PreparedUpdate {
    pub fn release(&self) -> &ReleaseManifest {
        self.candidate.release()
    }
    /// The available release history captured by the update check.
    pub fn release_history(&self) -> &[ReleaseNotes] {
        self.candidate.release_history()
    }
    pub fn staged_directory(&self) -> std::path::PathBuf {
        self.temporary.path().join("stage")
    }
}

impl<T: Transport> Updater<T> {
    pub fn check(
        &self,
        source: &ReleaseSource,
        cancel: &Cancellation,
        events: &mut dyn FnMut(Event),
    ) -> Result<Option<Candidate>> {
        events(Event::Checking);
        cancel.check()?;
        let pairs: Vec<ManifestLocation> = match source {
            ReleaseSource::Manifests(locations) => {
                if locations.len() > 100 {
                    return Err(Error::SizeLimit);
                }
                locations.clone()
            }
            ReleaseSource::Index { document } => {
                serde_json::from_slice(&fetch(&self.transport, document, 1024 * 1024, cancel)?)?
            }
            ReleaseSource::Manifest {
                document,
                signature,
            } => vec![ManifestLocation {
                document: document.clone(),
                signature: signature.clone(),
            }],
            ReleaseSource::GitHub { owner, repository } => {
                for value in [owner, repository] {
                    if value.is_empty()
                        || !value
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
                    {
                        return Err(Error::Invalid("GitHub owner or repository".into()));
                    }
                }
                let url = Url::parse(&format!(
                    "https://api.github.com/repos/{owner}/{repository}/releases?per_page=100"
                ))
                .unwrap();
                let releases: Vec<GitHubRelease> = serde_json::from_slice(&fetch(
                    &self.transport,
                    &url,
                    8 * 1024 * 1024,
                    cancel,
                )?)?;
                if releases.len() > 100 {
                    return Err(Error::SizeLimit);
                }
                releases
                    .into_iter()
                    .filter(|r| !r.draft && (!r.prerelease || self.channel != "stable"))
                    .filter_map(|r| {
                        let document = r
                            .assets
                            .iter()
                            .find(|a| a.name == "freshen-manifest.json")?;
                        let signature = r
                            .assets
                            .iter()
                            .find(|a| a.name == "freshen-manifest.json.sig")?;
                        Some(ManifestLocation {
                            document: document.browser_download_url.clone(),
                            signature: signature.browser_download_url.clone(),
                        })
                    })
                    .collect()
            }
        };
        if pairs.len() > 100 {
            return Err(Error::SizeLimit);
        }
        let mut best: Option<Candidate> = None;
        let mut history = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for ManifestLocation {
            document,
            signature,
        } in pairs
        {
            cancel.check()?;
            if !seen.insert((document.clone(), signature.clone())) {
                continue;
            }
            let signed = SignedManifest {
                document: fetch(&self.transport, &document, 1024 * 1024, cancel)?,
                signature: String::from_utf8(fetch(&self.transport, &signature, 1024, cancel)?)
                    .map_err(|_| Error::Signature)?,
            };
            let release = self.trust.verify(&signed)?;
            if release.product != self.product || release.channel != self.channel {
                continue;
            }
            if release.version <= self.current_version
                || (self.channel == "stable" && !release.version.pre.is_empty())
            {
                continue;
            }

            if let Some(artifact) = release
                .artifacts
                .iter()
                .find(|a| a.target == self.target)
                .cloned()
            {
                if let Some(previous) =
                    history.insert(release.version.clone(), release.notes.clone())
                    && previous != release.notes
                {
                    return Err(Error::Invalid(
                        "conflicting release notes for the same version".into(),
                    ));
                }
                if best
                    .as_ref()
                    .is_some_and(|c| c.release.version >= release.version)
                {
                    continue;
                }
                best = Some(Candidate {
                    history: Vec::new(),
                    signed,
                    release,
                    artifact,
                });
            }
        }
        if let Some(candidate) = &mut best {
            candidate.history = history
                .into_iter()
                .map(|(version, notes)| ReleaseNotes { version, notes })
                .collect();
        }
        Ok(best)
    }

    pub fn prepare(
        &self,
        candidate: Candidate,
        temporary_parent: &Path,
        cancel: &Cancellation,
        events: &mut dyn FnMut(Event),
    ) -> Result<PreparedUpdate> {
        // Revalidate with this updater's trust/policy in case a candidate was
        // obtained from a different Updater instance.
        let release = self.trust.verify(&candidate.signed)?;
        if release.product != self.product
            || release.channel != self.channel
            || release.version <= self.current_version
            || candidate.artifact.target != self.target
            || (self.channel == "stable" && !release.version.pre.is_empty())
        {
            return Err(Error::Invalid(
                "candidate does not match updater policy".into(),
            ));
        }
        if candidate.artifact.size > self.max_download {
            return Err(Error::SizeLimit);
        }
        fs::create_dir_all(temporary_parent)?;
        let temporary = tempfile::Builder::new()
            .prefix("freshen-")
            .tempdir_in(temporary_parent)?;
        let zip = temporary.path().join("package.zip");
        let mut output = File::create(&zip)?;
        self.transport.download(
            &candidate.artifact.url,
            &mut output,
            candidate.artifact.size,
            cancel,
            events,
        )?;
        output.sync_all()?;
        drop(output);
        cancel.check()?;
        events(Event::Verifying);
        if fs::metadata(&zip)?.len() != candidate.artifact.size
            || !hash_file(&zip)?.eq_ignore_ascii_case(&candidate.artifact.sha256)
        {
            return Err(Error::HashMismatch("package.zip".into()));
        }
        let stage = temporary.path().join("stage");
        fs::create_dir(&stage)?;
        extract(
            &zip,
            &stage,
            &candidate.artifact,
            self.max_unpacked,
            cancel,
            events,
        )?;
        events(Event::Ready);
        Ok(PreparedUpdate {
            temporary,
            candidate,
            trust: self.trust.clone(),
        })
    }
}

#[derive(Deserialize)]
struct GitHubRelease {
    draft: bool,
    prerelease: bool,
    assets: Vec<GitHubAsset>,
}
#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: Url,
}
