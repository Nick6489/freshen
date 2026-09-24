//! Uses separate copies of this test executable as an application to exercise
//! the real parent/helper/restarted-process protocol without any GUI toolkit.
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use freshen::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Write},
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
struct Scenario {
    root: PathBuf,
    package: PathBuf,
    helper: PathBuf,
    mode: String,
    signed: SignedManifest,
}
fn scenario() -> Option<Scenario> {
    let path = std::env::var_os("FRESHEN_TEST_SCENARIO")?;
    Some(serde_json::from_slice(&fs::read(path).unwrap()).unwrap())
}

struct LocalTransport(Scenario);
impl Transport for LocalTransport {
    fn download(
        &self,
        url: &url::Url,
        out: &mut dyn Write,
        limit: u64,
        cancel: &Cancellation,
        _: &mut dyn FnMut(Event),
    ) -> Result<u64> {
        cancel.check()?;
        let bytes = match url.path() {
            "/manifest" => self.0.signed.document.clone(),
            "/signature" => self.0.signed.signature.as_bytes().to_vec(),
            "/package" => fs::read(&self.0.package)?,
            _ => return Err(Error::Invalid("unexpected test URL".into())),
        };
        if bytes.len() as u64 > limit {
            return Err(Error::SizeLimit);
        }
        out.write_all(&bytes)?;
        Ok(bytes.len() as u64)
    }
}

#[test]
fn update_parent() {
    let Some(scenario) = scenario() else {
        return;
    };
    let root = scenario.root.clone();
    let helper = scenario.helper.clone();
    let mode = scenario.mode.clone();
    let updater = Updater {
        product: "helper-test".into(),
        channel: "stable".into(),
        current_version: "1.0.0".parse().unwrap(),
        target: "test-target".into(),
        trust: TrustStore::new(vec![
            SigningKey::from_bytes(&[19; 32]).verifying_key().to_bytes(),
        ])
        .unwrap(),
        transport: LocalTransport(scenario),
        max_download: 512 * 1024 * 1024,
        max_unpacked: 512 * 1024 * 1024,
    };
    let cancel = Cancellation::default();
    let source = ReleaseSource::Manifest {
        document: "https://test.invalid/manifest".parse().unwrap(),
        signature: "https://test.invalid/signature".parse().unwrap(),
    };
    let candidate = updater
        .check(&source, &cancel, &mut |_| {})
        .unwrap()
        .unwrap();
    let prepared = updater
        .prepare(candidate, root.parent().unwrap(), &cancel, &mut |_| {})
        .unwrap();
    let mut policy =
        InstallPolicy::portable(root.clone(), app_name().into(), vec![app_name().into()]);
    policy.confirmation_timeout_secs = if mode == "timeout" { 1 } else { 30 };
    let handoff = prepared
        .arm(
            policy,
            &helper,
            vec![
                "--exact".into(),
                "updated_child".into(),
                "--nocapture".into(),
            ],
        )
        .unwrap();
    thread::sleep(Duration::from_millis(200));
    assert_eq!(fs::read(root.join(app_name())).unwrap(), b"old application");
    if mode == "cancel" {
        handoff.cancel().unwrap();
    }
    // Dropping the handoff must not cancel a requested installation.
}

#[test]
fn updated_child() {
    let Some(scenario) = scenario() else {
        return;
    };
    if scenario.mode == "fail" {
        panic!("simulated startup failure");
    }
    if scenario.mode == "timeout" {
        thread::sleep(Duration::from_secs(3));
        return;
    }
    assert!(confirm_startup(&scenario.root).unwrap());
}

fn app_name() -> &'static str {
    if cfg!(windows) {
        "application.exe"
    } else {
        "application"
    }
}

#[test]
fn real_helper_commit_cancel_failure_and_timeout() {
    if scenario().is_some() {
        return;
    }
    let current = fs::read(std::env::current_exe().unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            app_name(),
            zip::write::SimpleFileOptions::default().unix_permissions(0o755),
        )
        .unwrap();
    writer.write_all(&current).unwrap();
    let package = writer.finish().unwrap().into_inner();
    let document = ReleaseManifest {
        schema: 1,
        product: "helper-test".into(),
        channel: "stable".into(),
        version: "2.0.0".parse().unwrap(),
        notes: String::new(),
        artifacts: vec![Artifact {
            target: "test-target".into(),
            kind: PackageKind::Files,
            url: "https://test.invalid/package".parse().unwrap(),
            sha256: hex::encode(Sha256::digest(&package)),
            size: package.len() as u64,
            files: vec![PackageFile {
                path: app_name().into(),
                sha256: hex::encode(Sha256::digest(&current)),
                size: current.len() as u64,
                executable: true,
            }],
        }],
    };
    let document = serde_json::to_vec(&document).unwrap();
    let signature = STANDARD.encode(
        SigningKey::from_bytes(&[19; 32])
            .sign(&signing_message(&document))
            .to_bytes(),
    );
    for mode in ["success", "cancel", "fail", "timeout"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installation");
        fs::create_dir(&root).unwrap();
        fs::write(root.join(app_name()), b"old application").unwrap();
        fs::write(root.join("user-settings"), b"keep me").unwrap();
        let package_path = temp.path().join("package.zip");
        fs::write(&package_path, &package).unwrap();
        let scenario = Scenario {
            root: root.clone(),
            package: package_path,
            helper: PathBuf::from(env!("CARGO_BIN_EXE_freshen-helper")),
            mode: mode.into(),
            signed: SignedManifest {
                document: document.clone(),
                signature: signature.clone(),
            },
        };
        let scenario_path = temp.path().join("scenario.json");
        fs::write(&scenario_path, serde_json::to_vec(&scenario).unwrap()).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "update_parent", "--nocapture"])
            .env("FRESHEN_TEST_SCENARIO", &scenario_path);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let expected = match mode {
            "success" => InstallState::Committed,
            "cancel" => InstallState::Prepared,
            "fail" => InstallState::RolledBack,
            _ => InstallState::Failed,
        };
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let status = installation_status(&root).unwrap().unwrap();
            if status.state == expected {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{mode}: timed out with {status:?}; {:?}",
                fs::read_to_string(root.join(".freshen/transaction/helper-error.txt"))
            );
            thread::sleep(Duration::from_millis(100));
        }
        if mode == "timeout" {
            // The library intentionally leaves a running, unconfirmed process
            // alone. Wait for this fixture to exit before offline recovery.
            thread::sleep(Duration::from_secs(3));
            assert_eq!(
                recover_installation(&root).unwrap().state,
                InstallState::RolledBack
            );
        }
        assert_eq!(fs::read(root.join("user-settings")).unwrap(), b"keep me");
        if mode != "success" {
            assert_eq!(fs::read(root.join(app_name())).unwrap(), b"old application");
        }
        // A committed status may be observed just before the helper releases its lock.
        loop {
            match discard_finished_installation(&root) {
                Ok(()) => break,
                Err(Error::Busy) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(100))
                }
                result => panic!("cleanup failed: {result:?}"),
            }
        }
    }
}
