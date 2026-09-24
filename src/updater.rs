use crate::{
    Artifact, Cancellation, Error, Event, ReleaseManifest, Result, SignedManifest, Transport,
    TrustStore,
    archive::{extract, hash_file},
    transport::fetch,
};
use semver::Version;
use serde::Deserialize;
use std::{
    fs::{self, File},
    path::Path,
};
use tempfile::TempDir;
use url::Url;

pub enum ReleaseSource {
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
}
impl Candidate {
    pub fn release(&self) -> &ReleaseManifest {
        &self.release
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
    pub fn staged_directory(&self) -> &Path {
        self.temporary.path()
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
        let pairs = match source {
            ReleaseSource::Manifest {
                document,
                signature,
            } => vec![(document.clone(), signature.clone())],
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
                        Some((
                            document.browser_download_url.clone(),
                            signature.browser_download_url.clone(),
                        ))
                    })
                    .collect()
            }
        };
        let mut best: Option<Candidate> = None;
        for (document, signature) in pairs {
            cancel.check()?;
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
            if best
                .as_ref()
                .is_some_and(|c| c.release.version >= release.version)
            {
                continue;
            }
            if let Some(artifact) = release
                .artifacts
                .iter()
                .find(|a| a.target == self.target)
                .cloned()
            {
                best = Some(Candidate {
                    signed,
                    release,
                    artifact,
                });
            }
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
