use crate::{
    Artifact, Error, PackageKind, PreparedUpdate, ReleaseManifest, Result, SignedManifest,
    TrustStore,
    archive::hash_file,
    fsutil::{atomic_write, checked_join, copy_tree, remove_tree, safe_join, sync_dir},
    manifest::check_path,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// Local application policy, never inferred from an untrusted download name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InstallLayout {
    Portable {
        owned_files: Vec<String>,
        executable: String,
        #[serde(default)]
        expandable_directories: Vec<String>,
    },
    /// `root` is the existing parent of this bundle. No implicit /Applications move.
    /// Version 1 supports bundles containing regular files, not framework symlinks.
    MacBundle {
        bundle_name: String,
        executable: String,
        bundle_id: String,
        team_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPolicy {
    pub root: PathBuf,
    pub layout: InstallLayout,
    pub exit_timeout_secs: u64,
    pub confirmation_timeout_secs: u64,
}

impl InstallPolicy {
    pub fn portable(root: PathBuf, executable: String, owned_files: Vec<String>) -> Self {
        Self::portable_expanding(root, executable, owned_files, vec![])
    }

    /// Allow additional signed files strictly beneath host-named directories.
    /// Every owned file must still be present, including the executable.
    pub fn portable_expanding(
        root: PathBuf,
        executable: String,
        owned_files: Vec<String>,
        expandable_directories: Vec<String>,
    ) -> Self {
        Self {
            root,
            layout: InstallLayout::Portable {
                owned_files,
                executable,
                expandable_directories,
            },
            exit_timeout_secs: 120,
            confirmation_timeout_secs: 120,
        }
    }

    pub(crate) fn validate(&self, artifact: &Artifact) -> Result<()> {
        if !(1..=3600).contains(&self.exit_timeout_secs)
            || !(1..=3600).contains(&self.confirmation_timeout_secs)
        {
            return Err(Error::Invalid(
                "helper timeouts must be 1..=3600 seconds".into(),
            ));
        }
        match &self.layout {
            InstallLayout::Portable {
                owned_files,
                executable,
                expandable_directories,
            } => {
                if artifact.kind != PackageKind::Files {
                    return Err(Error::Invalid("expected a files package".into()));
                }
                let mut owned = BTreeSet::new();
                for file in owned_files {
                    check_path(file)?;
                    owned.insert(file.as_str());
                }
                let received: BTreeSet<_> =
                    artifact.files.iter().map(|f| f.path.as_str()).collect();
                for directory in expandable_directories {
                    check_path(directory)?;
                }
                if owned.len() != owned_files.len()
                    || !owned.is_subset(&received)
                    || received.difference(&owned).any(|path| {
                        !expandable_directories.iter().any(|directory| {
                            path.strip_prefix(directory.as_str())
                                .is_some_and(|suffix| suffix.starts_with('/'))
                        })
                    })
                    || !owned.contains(executable.as_str())
                {
                    return Err(Error::Invalid("package inventory must contain all locally owned files and the owned executable; additional files require a locally expandable directory".into()));
                }
            }
            InstallLayout::MacBundle {
                bundle_name,
                executable,
                bundle_id,
                team_id,
            } => {
                if !cfg!(target_os = "macos") {
                    return Err(Error::Invalid(
                        "macOS bundle installation requires macOS".into(),
                    ));
                }
                check_path(bundle_name)?;
                check_path(executable)?;
                if bundle_name.contains('/')
                    || !bundle_name.ends_with(".app")
                    || artifact.kind != PackageKind::MacBundle
                    || !executable.starts_with(&format!("{bundle_name}/Contents/MacOS/"))
                    || !artifact
                        .files
                        .iter()
                        .all(|f| f.path.starts_with(&format!("{bundle_name}/")))
                    || !artifact.files.iter().any(|f| &f.path == executable)
                {
                    return Err(Error::Invalid("invalid application bundle layout".into()));
                }
                for value in [bundle_id, team_id] {
                    if value.is_empty()
                        || !value
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c))
                    {
                        return Err(Error::Invalid("invalid signing identity".into()));
                    }
                }
            }
        }
        Ok(())
    }

    // Only call with the artifact accepted by signature and policy verification.
    fn items(&self, artifact: &Artifact) -> Vec<String> {
        match &self.layout {
            InstallLayout::Portable { .. } => artifact
                .files
                .iter()
                .map(|file| file.path.clone())
                .collect(),
            InstallLayout::MacBundle { bundle_name, .. } => vec![bundle_name.clone()],
        }
    }

    pub(crate) fn executable(&self) -> &str {
        match &self.layout {
            InstallLayout::Portable { executable, .. }
            | InstallLayout::MacBundle { executable, .. } => executable,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum InstallState {
    Prepared,
    Applying,
    AwaitingConfirmation,
    Committed,
    RollingBack,
    RolledBack,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallStatus {
    pub state: InstallState,
    pub version: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Plan {
    pub policy: InstallPolicy,
    pub signed: SignedManifest,
    pub trust: TrustStore,
    pub target: String,
    pub parent_pid: u32,
    pub relaunch_args: Vec<String>,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Journal {
    status: InstallStatus,
    // Write intent before changing any target. Backups are durable beforehand.
    touched: Vec<String>,
    existing: BTreeSet<String>,
}

pub(crate) struct InstallationLock {
    _file: File,
}
pub(crate) fn lock(root: &Path) -> Result<InstallationLock> {
    let state = checked_join(root, ".freshen")?;
    fs::create_dir_all(&state)?;
    let path = checked_join(root, ".freshen/update.lock")?;
    let file = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.try_lock_exclusive().map_err(|e| {
        if e.raw_os_error() == fs2::lock_contended_error().raw_os_error()
            || e.kind() == std::io::ErrorKind::WouldBlock
        {
            Error::Busy
        } else {
            e.into()
        }
    })?;
    Ok(InstallationLock { _file: file })
}

pub(crate) fn transaction(root: &Path) -> Result<PathBuf> {
    checked_join(root, ".freshen/transaction")
}

pub(crate) fn load_plan(path: &Path) -> Result<Plan> {
    if fs::metadata(path)?.len() > 4 * 1024 * 1024 {
        return Err(Error::SizeLimit);
    }
    let plan: Plan = serde_json::from_slice(&fs::read(path)?)?;
    let root = fs::canonicalize(&plan.policy.root)?;
    if root != plan.policy.root || fs::canonicalize(path)? != transaction(&root)?.join("plan.json")
    {
        return Err(Error::UnsafePath(path.display().to_string()));
    }
    verify_plan(&plan)?;
    Ok(plan)
}

fn verify_plan(plan: &Plan) -> Result<(ReleaseManifest, Artifact)> {
    if plan.parent_pid == 0
        || plan.token.len() != 64
        || !plan.token.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(Error::Invalid("invalid helper identity".into()));
    }
    let release = plan.trust.verify(&plan.signed)?;
    let artifact = release
        .artifacts
        .iter()
        .find(|a| a.target == plan.target)
        .ok_or_else(|| Error::Invalid("missing target".into()))?
        .clone();
    plan.policy.validate(&artifact)?;
    Ok((release, artifact))
}

impl PreparedUpdate {
    /// Copy verified staging into the installation filesystem and start a helper.
    /// Returns only after the helper holds the installation lock and is waiting.
    /// The caller then saves work and exits, or explicitly cancels the handoff.
    pub fn arm(
        self,
        mut policy: InstallPolicy,
        helper_executable: &Path,
        relaunch_args: Vec<String>,
    ) -> Result<crate::Handoff> {
        policy.validate(&self.candidate.artifact)?;
        policy.root = fs::canonicalize(&policy.root)?;
        let guard = lock(&policy.root)?;
        let txn = transaction(&policy.root)?;
        if txn.exists() {
            return Err(Error::Recovery(
                "resolve or discard the previous transaction before preparing another".into(),
            ));
        }
        fs::create_dir(&txn)?;
        let result = (|| {
            copy_tree(&self.temporary.path().join("stage"), &txn.join("stage"))?;
            let mut token_bytes = [0u8; 32];
            getrandom::fill(&mut token_bytes).map_err(|e| Error::Helper(e.to_string()))?;
            let plan = Plan {
                policy,
                signed: self.candidate.signed.clone(),
                trust: self.trust.clone(),
                target: self.candidate.artifact.target.clone(),
                parent_pid: std::process::id(),
                relaunch_args,
                token: hex::encode(token_bytes),
            };
            verify_stage(&plan)?;
            let journal = Journal {
                status: InstallStatus {
                    state: InstallState::Prepared,
                    version: self.candidate.release.version.to_string(),
                    detail: String::new(),
                },
                touched: vec![],
                existing: BTreeSet::new(),
            };
            save_journal(&txn, &journal)?;
            atomic_write(&txn.join("plan.json"), &serde_json::to_vec(&plan)?)?;
            Ok(plan)
        })();
        let plan = match result {
            Ok(plan) => plan,
            Err(error) => {
                let _ = remove_tree(&txn);
                return Err(error);
            }
        };
        drop(guard);
        crate::helper::launch(&plan, helper_executable)
    }
}

pub(crate) fn verify_stage(plan: &Plan) -> Result<()> {
    let (_, artifact) = verify_plan(plan)?;
    let stage = transaction(&plan.policy.root)?.join("stage");
    for file in &artifact.files {
        let path = safe_join(&stage, &file.path)?;
        if fs::metadata(&path)?.len() != file.size
            || !hash_file(&path)?.eq_ignore_ascii_case(&file.sha256)
        {
            return Err(Error::HashMismatch(file.path.clone()));
        }
    }
    // Bundles are copied as a directory: ensure no extra unsigned files were
    // introduced between preparation and handoff.
    let mut actual = BTreeSet::new();
    list_files(&stage, &stage, &mut actual)?;
    let expected: BTreeSet<_> = artifact.files.iter().map(|f| f.path.clone()).collect();
    if actual != expected {
        return Err(Error::Invalid("staged file inventory changed".into()));
    }
    if let InstallLayout::MacBundle {
        bundle_name,
        bundle_id,
        team_id,
        ..
    } = &plan.policy.layout
    {
        let requirement = format!(
            "anchor apple generic and certificate leaf[subject.OU] = \"{team_id}\" and identifier \"{bundle_id}\""
        );
        let status = Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict", "--test-requirement"])
            .arg(format!("={requirement}"))
            .arg(stage.join(bundle_name))
            .status()?;
        if !status.success() {
            return Err(Error::Signature);
        }
        let arch = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x86_64"
        };
        let status = Command::new("/usr/bin/lipo")
            .args(["-verify_arch", arch])
            .arg(stage.join(plan.policy.executable()))
            .status()?;
        if !status.success() {
            return Err(Error::Invalid("bundle architecture mismatch".into()));
        }
    }
    Ok(())
}

fn list_files(root: &Path, folder: &Path, output: &mut BTreeSet<String>) -> Result<()> {
    for item in fs::read_dir(folder)? {
        let item = item?;
        let metadata = fs::symlink_metadata(item.path())?;
        if crate::fsutil::is_link(&metadata) {
            return Err(Error::UnsafePath(item.path().display().to_string()));
        }
        if metadata.is_dir() {
            list_files(root, &item.path(), output)?;
        } else if metadata.is_file() {
            output.insert(
                item.path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        } else {
            return Err(Error::UnsafePath(item.path().display().to_string()));
        }
    }
    Ok(())
}

fn save_journal(txn: &Path, journal: &Journal) -> Result<()> {
    atomic_write(
        &txn.join("journal.json"),
        &serde_json::to_vec_pretty(journal)?,
    )
}

fn load_journal(txn: &Path) -> Result<Journal> {
    if fs::metadata(txn.join("journal.json"))?.len() > 16 * 1024 * 1024 {
        return Err(Error::SizeLimit);
    }
    Ok(serde_json::from_slice(&fs::read(
        txn.join("journal.json"),
    )?)?)
}

/// Read-only status; can also be shown by the GUI on a later application launch.
pub fn installation_status(root: &Path) -> Result<Option<InstallStatus>> {
    let txn = transaction(root)?;
    if !txn.exists() {
        return Ok(None);
    }
    Ok(Some(load_journal(&txn)?.status))
}

pub(crate) fn apply(plan: &Plan) -> Result<()> {
    verify_stage(plan)?;
    let (_, artifact) = verify_plan(plan)?;
    let items = plan.policy.items(&artifact);
    let txn = transaction(&plan.policy.root)?;
    let mut journal = load_journal(&txn)?;
    if journal.status.state != InstallState::Prepared {
        return Err(Error::Recovery("transaction is not prepared".into()));
    }
    let backup = txn.join("backup");
    fs::create_dir_all(&backup)?;
    // Capture the entire rollback set before modifying any destination.
    for item in &items {
        let destination = safe_join(&plan.policy.root, item)?;
        if destination.exists() {
            copy_tree(&destination, &safe_join(&backup, item)?)?;
            journal.existing.insert(item.clone());
        }
    }
    journal.status.state = InstallState::Applying;
    save_journal(&txn, &journal)?;
    let result = (|| {
        for item in &items {
            let destination = safe_join(&plan.policy.root, item)?;
            journal.touched.push(item.clone());
            save_journal(&txn, &journal)?;
            if matches!(plan.policy.layout, InstallLayout::MacBundle { .. }) {
                // Preserve the original bundle by rename on the same filesystem.
                // A durable backup also exists for interrupted rollback recovery.
                if destination.exists() {
                    fs::rename(&destination, txn.join("previous-bundle"))?;
                }
                fs::rename(safe_join(&txn.join("stage"), item)?, &destination)?;
                sync_dir(&plan.policy.root)?;
            } else {
                let source = safe_join(&txn.join("stage"), item)?;
                let start = std::time::Instant::now();
                loop {
                    match copy_tree(&source, &destination) {
                        Ok(()) => break,
                        Err(Error::Io(error))
                            if start.elapsed() < Duration::from_secs(10)
                                && matches!(
                                    error.kind(),
                                    std::io::ErrorKind::PermissionDenied
                                        | std::io::ErrorKind::WouldBlock
                                ) =>
                        {
                            std::thread::sleep(Duration::from_millis(200))
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
        }
        journal.status.state = InstallState::AwaitingConfirmation;
        save_journal(&txn, &journal)
    })();
    if let Err(error) = result {
        journal.status.detail = error.to_string();
        rollback_inner(plan, &mut journal)?;
        return Err(error);
    }
    Ok(())
}

fn rollback_inner(plan: &Plan, journal: &mut Journal) -> Result<()> {
    let (_, artifact) = verify_plan(plan)?;
    let items = plan.policy.items(&artifact);
    let txn = transaction(&plan.policy.root)?;
    journal.status.state = InstallState::RollingBack;
    save_journal(&txn, journal)?;
    for item in journal.touched.iter().rev() {
        if !items.contains(item) {
            return Err(Error::UnsafePath(item.clone()));
        }
        let destination = safe_join(&plan.policy.root, item)?;
        if journal.existing.contains(item) {
            let source = safe_join(&txn.join("backup"), item)?;
            if !source.exists() {
                return Err(Error::Recovery(format!("missing backup for {item}")));
            }
            if source.is_dir() {
                remove_tree(&destination)?;
            }
            crate::fsutil::copy_tree_with_retry(&source, &destination)?;
        } else {
            remove_tree(&destination)?;
        }
    }
    journal.status.state = InstallState::RolledBack;
    save_journal(&txn, journal)
}

/// Run before normal startup when status is Applying/RollingBack/Failed. The
/// installation must be offline: this does not terminate any running application.
pub fn recover_installation(root: &Path) -> Result<InstallStatus> {
    let root = fs::canonicalize(root)?;
    let _guard = lock(&root)?;
    let txn = transaction(&root)?;
    let plan = load_plan(&txn.join("plan.json"))?;
    let mut journal = load_journal(&txn)?;
    if matches!(
        journal.status.state,
        InstallState::Committed | InstallState::RolledBack
    ) {
        return Ok(journal.status);
    }
    rollback_inner(&plan, &mut journal)?;
    Ok(journal.status)
}

/// Remove staging/backups only after a terminal state. Recovery material is kept
/// until the host explicitly calls this function; no automatic backup expiration.
pub fn discard_finished_installation(root: &Path) -> Result<()> {
    let root = fs::canonicalize(root)?;
    let _guard = lock(&root)?;
    let txn = transaction(&root)?;
    if !txn.exists() {
        return Ok(());
    }
    let state = load_journal(&txn)?.status.state;
    if !matches!(
        state,
        InstallState::Committed | InstallState::RolledBack | InstallState::Prepared
    ) {
        return Err(Error::Recovery(
            "cannot discard an unfinished installation".into(),
        ));
    }
    remove_tree(&txn)
}

pub(crate) fn finish(root: &Path, state: InstallState, detail: String) -> Result<()> {
    let txn = transaction(root)?;
    let mut journal = load_journal(&txn)?;
    journal.status.state = state;
    journal.status.detail = detail;
    save_journal(&txn, &journal)
}

pub(crate) fn rollback(plan: &Plan, detail: String) -> Result<()> {
    let txn = transaction(&plan.policy.root)?;
    let mut journal = load_journal(&txn)?;
    journal.status.detail = detail;
    rollback_inner(plan, &mut journal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{manifest, signed, trust};

    fn fixture() -> (tempfile::TempDir, Plan) {
        let root = tempfile::tempdir().unwrap();
        let canonical = fs::canonicalize(root.path()).unwrap();
        let release = manifest();
        let plan = Plan {
            policy: InstallPolicy::portable(canonical, "app.exe".into(), vec!["app.exe".into()]),
            signed: signed(&release),
            trust: trust(),
            target: "test-target".into(),
            parent_pid: std::process::id(),
            relaunch_args: vec![],
            token: "a".repeat(64),
        };
        let txn = transaction(&plan.policy.root).unwrap();
        fs::create_dir_all(txn.join("stage")).unwrap();
        fs::write(txn.join("stage/app.exe"), b"new").unwrap();
        fs::write(txn.join("plan.json"), serde_json::to_vec(&plan).unwrap()).unwrap();
        let journal = Journal {
            status: InstallStatus {
                state: InstallState::Prepared,
                version: "2.0.0".into(),
                detail: String::new(),
            },
            touched: vec![],
            existing: BTreeSet::new(),
        };
        save_journal(&txn, &journal).unwrap();
        (root, plan)
    }

    #[test]
    fn applies_owned_files_and_keeps_user_data() {
        let (_root, plan) = fixture();
        fs::write(plan.policy.root.join("app.exe"), b"old").unwrap();
        fs::write(plan.policy.root.join("settings.ini"), b"precious").unwrap();
        apply(&plan).unwrap();
        assert_eq!(fs::read(plan.policy.root.join("app.exe")).unwrap(), b"new");
        assert_eq!(
            fs::read(plan.policy.root.join("settings.ini")).unwrap(),
            b"precious"
        );
        assert_eq!(
            installation_status(&plan.policy.root)
                .unwrap()
                .unwrap()
                .state,
            InstallState::AwaitingConfirmation
        );
        assert!(discard_finished_installation(&plan.policy.root).is_err());
        rollback(&plan, "test failure".into()).unwrap();
        assert_eq!(fs::read(plan.policy.root.join("app.exe")).unwrap(), b"old");
        discard_finished_installation(&plan.policy.root).unwrap();
        assert!(installation_status(&plan.policy.root).unwrap().is_none());
    }

    #[test]
    fn rollback_removes_files_that_did_not_previously_exist() {
        let (_root, plan) = fixture();
        apply(&plan).unwrap();
        rollback(&plan, "test failure".into()).unwrap();
        assert!(!plan.policy.root.join("app.exe").exists());
    }

    #[test]
    fn recovers_a_crash_after_intent_before_replacement() {
        let (_root, plan) = fixture();
        let txn = transaction(&plan.policy.root).unwrap();
        fs::create_dir(txn.join("backup")).unwrap();
        fs::write(txn.join("backup/app.exe"), b"old").unwrap();
        fs::write(plan.policy.root.join("app.exe"), b"partial").unwrap();
        let mut journal = load_journal(&txn).unwrap();
        journal.status.state = InstallState::Applying;
        journal.existing.insert("app.exe".into());
        journal.touched.push("app.exe".into());
        save_journal(&txn, &journal).unwrap();
        let status = recover_installation(&plan.policy.root).unwrap();
        assert_eq!(status.state, InstallState::RolledBack);
        assert_eq!(fs::read(plan.policy.root.join("app.exe")).unwrap(), b"old");
        assert_eq!(
            recover_installation(&plan.policy.root).unwrap().state,
            InstallState::RolledBack
        );
    }

    #[test]
    fn missing_backup_does_not_report_successful_rollback() {
        let (_root, plan) = fixture();
        fs::write(plan.policy.root.join("app.exe"), b"old").unwrap();
        apply(&plan).unwrap();
        fs::remove_file(
            transaction(&plan.policy.root)
                .unwrap()
                .join("backup/app.exe"),
        )
        .unwrap();
        assert!(matches!(
            rollback(&plan, "failure".into()),
            Err(Error::Recovery(_))
        ));
        assert_eq!(
            installation_status(&plan.policy.root)
                .unwrap()
                .unwrap()
                .state,
            InstallState::RollingBack
        );
    }

    #[test]
    fn tampered_staging_fails_before_touching_installation() {
        let (_root, plan) = fixture();
        fs::write(plan.policy.root.join("app.exe"), b"old").unwrap();
        fs::write(
            transaction(&plan.policy.root)
                .unwrap()
                .join("stage/app.exe"),
            b"bad",
        )
        .unwrap();
        assert!(matches!(apply(&plan), Err(Error::HashMismatch(_))));
        assert_eq!(fs::read(plan.policy.root.join("app.exe")).unwrap(), b"old");
    }

    #[test]
    fn installation_lock_excludes_another_writer() {
        let (_root, plan) = fixture();
        let _guard = lock(&plan.policy.root).unwrap();
        assert!(matches!(lock(&plan.policy.root), Err(Error::Busy)));
    }

    #[test]
    fn locally_owned_inventory_cannot_expand_from_remote_metadata() {
        let (_root, plan) = fixture();
        let mut artifact = manifest().artifacts.remove(0);
        let mut extra = artifact.files[0].clone();
        extra.path = "settings.ini".into();
        artifact.files.push(extra);
        assert!(plan.policy.validate(&artifact).is_err());
    }

    fn expanding_fixture() -> (tempfile::TempDir, Plan) {
        let (root, mut plan) = fixture();
        plan.policy = InstallPolicy::portable_expanding(
            plan.policy.root.clone(),
            "app.exe".into(),
            vec!["app.exe".into()],
            vec!["sounds".into()],
        );
        let mut release = manifest();
        let mut extra = release.artifacts[0].files[0].clone();
        extra.path = "sounds/die4.opus".into();
        release.artifacts[0].files.push(extra);
        plan.signed = signed(&release);
        let txn = transaction(&plan.policy.root).unwrap();
        fs::create_dir_all(txn.join("stage/sounds")).unwrap();
        fs::write(txn.join("stage/sounds/die4.opus"), b"new").unwrap();
        fs::write(txn.join("plan.json"), serde_json::to_vec(&plan).unwrap()).unwrap();
        (root, plan)
    }

    #[test]
    fn expansion_installs_and_recovers_new_and_existing_files() {
        for existing in [false, true] {
            let (_root, plan) = expanding_fixture();
            let sound = plan.policy.root.join("sounds/die4.opus");
            fs::write(plan.policy.root.join("app.exe"), b"old").unwrap();
            if existing {
                fs::create_dir_all(sound.parent().unwrap()).unwrap();
                fs::write(&sound, b"previous sound").unwrap();
            }
            apply(&plan).unwrap();
            assert_eq!(fs::read(&sound).unwrap(), b"new");
            // Exercise durable recovery through the serialized plan and journal.
            let status = recover_installation(&plan.policy.root).unwrap();
            assert_eq!(status.state, InstallState::RolledBack);
            assert_eq!(fs::read(plan.policy.root.join("app.exe")).unwrap(), b"old");
            if existing {
                assert_eq!(fs::read(&sound).unwrap(), b"previous sound");
            } else {
                assert!(!sound.exists());
            }
        }
    }

    #[test]
    fn expansion_requires_owned_inventory_and_strict_directory_boundary() {
        let (_root, plan) = expanding_fixture();
        for path in ["sounds/die4.opus", "sounds/extra/die.opus"] {
            let mut artifact = manifest().artifacts.remove(0);
            let mut extra = artifact.files[0].clone();
            extra.path = path.into();
            artifact.files.push(extra);
            plan.policy.validate(&artifact).unwrap();
            artifact.files.remove(0);
            assert!(plan.policy.validate(&artifact).is_err());
        }
        for path in [
            "saves/game.json",
            "notes.txt",
            "sounds-extra/die.opus",
            "sounds",
            "Sounds/die.opus",
        ] {
            let mut artifact = manifest().artifacts.remove(0);
            let mut extra = artifact.files[0].clone();
            extra.path = path.into();
            artifact.files.push(extra);
            assert!(plan.policy.validate(&artifact).is_err(), "{path}");
        }
        let mut policy = plan.policy.clone();
        if let InstallLayout::Portable { executable, .. } = &mut policy.layout {
            *executable = "sounds/die4.opus".into();
        }
        let (_, artifact) = verify_plan(&plan).unwrap();
        assert!(policy.validate(&artifact).is_err());
    }

    #[test]
    fn expansion_rejects_unsafe_directories() {
        let (_root, plan) = fixture();
        for prefix in [
            "",
            ".",
            "..",
            "sounds/../saves",
            ".freshen",
            "sounds/.freshen",
        ] {
            let policy = InstallPolicy::portable_expanding(
                plan.policy.root.clone(),
                "app.exe".into(),
                vec!["app.exe".into()],
                vec![prefix.into()],
            );
            assert!(
                matches!(
                    policy.validate(&manifest().artifacts[0]),
                    Err(Error::UnsafePath(_))
                ),
                "{prefix}"
            );
        }
    }

    #[test]
    fn legacy_plan_defaults_to_exact_ownership_and_recovers() {
        let (_root, plan) = fixture();
        fs::write(plan.policy.root.join("app.exe"), b"old").unwrap();
        apply(&plan).unwrap();
        let mut json = serde_json::to_value(&plan).unwrap();
        json["policy"]["layout"]["Portable"]
            .as_object_mut()
            .unwrap()
            .remove("expandable_directories");
        let path = transaction(&plan.policy.root).unwrap().join("plan.json");
        fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        let loaded = load_plan(&path).unwrap();
        assert!(
            matches!(&loaded.policy.layout, InstallLayout::Portable { expandable_directories, .. } if expandable_directories.is_empty())
        );
        let mut artifact = manifest().artifacts.remove(0);
        let mut extra = artifact.files[0].clone();
        extra.path = "sounds/die4.opus".into();
        artifact.files.push(extra);
        assert!(loaded.policy.validate(&artifact).is_err());
        recover_installation(&plan.policy.root).unwrap();
        assert_eq!(fs::read(plan.policy.root.join("app.exe")).unwrap(), b"old");
    }

    #[test]
    fn expansion_staging_still_requires_signed_bytes_and_inventory() {
        let (_root, plan) = expanding_fixture();
        verify_stage(&plan).unwrap();
        let stage = transaction(&plan.policy.root).unwrap().join("stage");
        fs::write(stage.join("sounds/die4.opus"), b"bad").unwrap();
        assert!(matches!(verify_stage(&plan), Err(Error::HashMismatch(_))));
        assert!(matches!(apply(&plan), Err(Error::HashMismatch(_))));
        assert!(!plan.policy.root.join("app.exe").exists());
        fs::write(stage.join("sounds/die4.opus"), b"new").unwrap();
        fs::write(stage.join("sounds/unsigned.opus"), b"new").unwrap();
        assert!(matches!(verify_stage(&plan), Err(Error::Invalid(_))));
    }

    #[test]
    fn expansion_rollback_rejects_unsigned_touched_paths() {
        let (_root, plan) = expanding_fixture();
        apply(&plan).unwrap();
        let txn = transaction(&plan.policy.root).unwrap();
        let mut journal = load_journal(&txn).unwrap();
        journal.touched.push("sounds/unsigned.opus".into());
        fs::write(plan.policy.root.join("sounds/unsigned.opus"), b"keep").unwrap();
        assert!(matches!(
            rollback_inner(&plan, &mut journal),
            Err(Error::UnsafePath(_))
        ));
        assert_eq!(
            fs::read(plan.policy.root.join("sounds/unsigned.opus")).unwrap(),
            b"keep"
        );
    }

    #[cfg(unix)]
    #[test]
    fn destination_symlink_is_rejected() {
        let (_root, plan) = fixture();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), plan.policy.root.join("app.exe")).unwrap();
        assert!(matches!(apply(&plan), Err(Error::UnsafePath(_))));
    }
}
