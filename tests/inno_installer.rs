//! Opt-in real Inno 6/7 fixtures. No tool downloads or system-wide installation.
#![cfg(windows)]
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use freshen::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
struct Scenario {
    state: PathBuf,
    directory: PathBuf,
    package: PathBuf,
    signed: SignedManifest,
    app_id: String,
    enhanced: bool,
}
struct Local(Scenario);
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        // Also clean a regression that accidentally created another uninstall log.
        let installed = self.0.join("installed");
        let mut uninstallers: Vec<_> = fs::read_dir(&installed)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("unins") && name.ends_with(".exe"))
            })
            .map(|entry| entry.path())
            .collect();
        uninstallers.sort();
        for uninstaller in uninstallers.into_iter().rev().filter(|path| path.exists()) {
            let _ = Command::new(uninstaller)
                .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
                .status();
        }
    }
}
impl Transport for Local {
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
            _ => return Err(Error::Invalid("fixture URL".into())),
        };
        if bytes.len() as u64 > limit {
            return Err(Error::SizeLimit);
        }
        out.write_all(&bytes)?;
        Ok(bytes.len() as u64)
    }
}
#[test]
fn servicing_parent() {
    let Some(path) = std::env::var_os("FRESHEN_INNO_SCENARIO") else {
        return;
    };
    let scenario: Scenario = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let updater = Updater {
        product: "inno-test".into(),
        channel: "stable".into(),
        current_version: "1.0.0".parse().unwrap(),
        target: "test-windows".into(),
        trust: TrustStore::new(vec![
            SigningKey::from_bytes(&[71; 32]).verifying_key().to_bytes(),
        ])
        .unwrap(),
        transport: Local(scenario),
        max_download: 100_000_000,
        max_unpacked: 100_000_000,
    };
    let scenario = &updater.transport.0;
    let cancel = Cancellation::default();
    let candidate = updater
        .check(
            &ReleaseSource::Manifest {
                document: "https://fixture.invalid/manifest".parse().unwrap(),
                signature: "https://fixture.invalid/signature".parse().unwrap(),
            },
            &cancel,
            &mut |_| {},
        )
        .unwrap()
        .unwrap();
    let prepared = updater
        .prepare(
            candidate,
            scenario.state.parent().unwrap(),
            &cancel,
            &mut |_| {},
        )
        .unwrap();
    let policy = InstallPolicy {
        root: scenario.state.clone(),
        layout: InstallLayout::Inno(inno::InnoPolicy {
            installer: "setup.exe".into(),
            app_id: scenario.app_id.clone(),
            directory: scenario.directory.clone(),
            executable: "version.txt".into(),
            machine_wide: false,
            enhanced: scenario.enhanced,
            timeout_secs: 1,
            relaunch: false,
        }),
        exit_timeout_secs: 30,
        confirmation_timeout_secs: 30,
    };
    let handoff = prepared
        .arm(
            policy,
            Path::new(env!("CARGO_BIN_EXE_freshen-helper")),
            vec![],
        )
        .unwrap();
    fs::write(
        scenario.state.parent().unwrap().join("helper-pid"),
        handoff.helper_pid().to_string(),
    )
    .unwrap();
}

fn compile(
    compiler: &Path,
    directory: &Path,
    app_id: &str,
    version: &str,
    mode: &str,
    dll: &Path,
    native_x64: bool,
) -> PathBuf {
    fs::write(
        directory.join("version.txt"),
        if mode == "locked" {
            "locked-new-content"
        } else {
            version
        },
    )
    .unwrap();
    // Enough incompressible payload for real progress callbacks to run for more than one poll.
    if !directory.join("payload.bin").exists() {
        let mut data = vec![0; 8 * 1024 * 1024];
        getrandom::fill(&mut data).unwrap();
        fs::write(directory.join("payload.bin"), data).unwrap();
    }
    let bridge = if mode == "basic" {
        String::new()
    } else {
        format!(
            "#define FreshenInnoDll \"{}\"\n#include \"{}\"\n",
            dll.display(),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("inno/FreshenInno.iss")
                .display()
        )
    };
    let behavior = match mode {
        "failure" => {
            "function PrepareToInstall(var NeedsRestart: Boolean): String; begin Result := 'Intentional fixture failure'; end;"
        }
        "reboot" => "function NeedRestart: Boolean; begin Result := True; end;",
        "disconnect" => {
            "<event('CurStepChanged')> procedure FixtureDisconnect(Step: TSetupStep); begin if Step = ssInstall then FreshenDisconnect; end;"
        }
        "overdue" => {
            "<event('InitializeSetup')> function FixtureDelay: Boolean; begin Sleep(1600); Result := True; end;"
        }
        _ => "",
    };
    let script = format!(
        r#"[Setup]
{architecture}
AppId={app_id}
AppName=Freshen integration fixture
AppVersion={version}
DefaultDirName={install}
PrivilegesRequired=lowest
OutputDir={output}
OutputBaseFilename=fixture-{version}-{mode}
Compression=none
CloseApplications=no
RestartApplications=no
[Files]
Source: "{output}\version.txt"; DestDir: "{{app}}"; Flags: ignoreversion
Source: "{output}\payload.bin"; DestDir: "{{app}}"; Flags: ignoreversion
{bridge}
[Code]
{behavior}
"#,
        install = directory.join("installed").display(),
        architecture = match (
            native_x64,
            std::env::var_os("FRESHEN_INITIAL_ISCC").is_some()
        ) {
            (false, _) => "",
            // Inno 6 fixture used 32-bit install mode. Preserve that identity even
            // though the new setup process and telemetry DLL are native x64.
            (true, true) => "SetupArchitecture=x64\nArchitecturesInstallIn64BitMode=",
            (true, false) => "SetupArchitecture=x64",
        },
        output = directory.display()
    );
    let path = directory.join("fixture.iss");
    fs::write(&path, script).unwrap();
    let output = Command::new(compiler).arg(&path).output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    directory.join(format!("fixture-{version}-{mode}.exe"))
}

fn run_update(directory: &Path, installer: &Path, app_id: &str, enhanced: bool) -> inno::Status {
    let state = directory.join("state");
    fs::create_dir_all(&state).unwrap();
    let package = directory.join("package.zip");
    let bytes = fs::read(installer).unwrap();
    let mut zip = zip::ZipWriter::new(fs::File::create(&package).unwrap());
    zip.start_file("setup.exe", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(&bytes).unwrap();
    zip.finish().unwrap();
    let artifact = Artifact {
        target: "test-windows".into(),
        kind: PackageKind::Inno,
        url: "https://fixture.invalid/package".parse().unwrap(),
        sha256: hex::encode(Sha256::digest(fs::read(&package).unwrap())),
        size: fs::metadata(&package).unwrap().len(),
        files: vec![PackageFile {
            path: "setup.exe".into(),
            sha256: hex::encode(Sha256::digest(&bytes)),
            size: bytes.len() as u64,
            executable: true,
        }],
    };
    let release = ReleaseManifest {
        schema: 1,
        product: "inno-test".into(),
        channel: "stable".into(),
        version: "2.0.0".parse().unwrap(),
        notes: String::new(),
        artifacts: vec![artifact],
    };
    let document = serde_json::to_vec(&release).unwrap();
    let signed = SignedManifest {
        signature: STANDARD.encode(
            SigningKey::from_bytes(&[71; 32])
                .sign(&signing_message(&document))
                .to_bytes(),
        ),
        document,
    };
    let scenario = Scenario {
        state: state.clone(),
        directory: directory.join("installed"),
        package,
        signed,
        app_id: app_id.into(),
        enhanced,
    };
    let path = directory.join("scenario.json");
    fs::write(&path, serde_json::to_vec(&scenario).unwrap()).unwrap();
    assert!(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "servicing_parent", "--nocapture"])
            .env("FRESHEN_INNO_SCENARIO", path)
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(status) = inno::status(&state).unwrap()
            && status.exit_code.is_some()
        {
            use windows_sys::Win32::{
                Foundation::{CloseHandle, WAIT_OBJECT_0},
                System::Threading::{OpenProcess, WaitForSingleObject},
            };
            let pid = fs::read_to_string(directory.join("helper-pid"))
                .unwrap()
                .parse()
                .unwrap();
            let handle = unsafe { OpenProcess(0x0010_0000, 0, pid) };
            if !handle.is_null() {
                let wait = unsafe { WaitForSingleObject(handle, 60000) };
                unsafe {
                    CloseHandle(handle);
                }
                assert_eq!(wait, WAIT_OBJECT_0);
            }
            // Status is written before the journal; retain the lock until both finish.
            match discard_finished_installation(&state) {
                Ok(()) => return status,
                Err(Error::Busy | Error::Recovery(_)) => {}
                // The helper releases its lock immediately before process teardown;
                // Windows may retain the copied helper.exe image mapping briefly.
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied => {}
                Err(error) => panic!("cannot discard completed fixture: {error}"),
            }
        }
        assert!(
            Instant::now() < deadline,
            "installer did not finish; retain fixture logs and stop processes before cleanup"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "requires FRESHEN_ISCC (Inno 6/7), optional FRESHEN_SETUP_ARCH=x64, and both release DLLs"]
fn real_install_update_basic_enhanced_failure_reboot_and_mismatched_dll() {
    let compiler = PathBuf::from(std::env::var_os("FRESHEN_ISCC").expect("set FRESHEN_ISCC"));
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let native_x64 = match std::env::var("FRESHEN_SETUP_ARCH").as_deref() {
        Err(_) | Ok("x86") => false,
        Ok("x64") => true,
        _ => panic!("FRESHEN_SETUP_ARCH must be x86 or x64"),
    };
    let x86 = root.join("target/i686-pc-windows-msvc/release/FreshenInno.dll");
    let x64 = root.join("target/x86_64-pc-windows-msvc/release/FreshenInno.dll");
    let (dll, wrong_dll) = if native_x64 { (x64, x86) } else { (x86, x64) };
    assert!(dll.exists() && wrong_dll.exists());
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path();
    let _cleanup = Cleanup(directory.to_path_buf());
    let app_id = format!("FreshenIntegrationFixture-{}", std::process::id());
    let initial_compiler = std::env::var_os("FRESHEN_INITIAL_ISCC").map(PathBuf::from);
    // Optional Inno-6/x86 initial install followed by Inno-7/x64 updates.
    let initial_dll = if initial_compiler.is_some() {
        root.join("target/i686-pc-windows-msvc/release/FreshenInno.dll")
    } else {
        dll.clone()
    };
    let initial = compile(
        initial_compiler.as_deref().unwrap_or(&compiler),
        directory,
        &app_id,
        "1.0.0",
        "enhanced",
        &initial_dll,
        native_x64 && initial_compiler.is_none(),
    );
    assert!(
        Command::new(initial)
            .args([
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/CURRENTUSER"
            ])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        fs::read_to_string(directory.join("installed/version.txt")).unwrap(),
        "1.0.0"
    );
    for (mode, dll) in [
        ("basic", &dll),
        ("enhanced", &dll),
        ("failure", &dll),
        ("reboot", &dll),
        ("mismatch", &wrong_dll),
        ("disconnect", &dll),
        ("overdue", &dll),
        ("locked", &dll),
    ] {
        let installer = compile(
            &compiler, directory, &app_id, "2.0.0", mode, dll, native_x64,
        );
        use std::os::windows::fs::OpenOptionsExt;
        let held_file = (mode == "locked").then(|| {
            fs::OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(directory.join("installed/version.txt"))
                .unwrap()
        });
        let status = run_update(directory, &installer, &app_id, mode != "basic");
        drop(held_file);
        if mode == "locked" {
            assert_ne!(
                status.exit_code,
                Some(0),
                "locked file must not report successful replacement: {status:?}"
            );
            continue;
        }
        assert_eq!(
            status.exit_code,
            Some(match mode {
                "failure" => 7,
                "reboot" => 3010,
                _ => 0,
            }),
            "{status:?}"
        );
        if mode == "enhanced" {
            assert!(
                status.telemetry_observed && status.progress_observed,
                "{status:?}"
            );
        }
        if mode == "basic" || mode == "mismatch" {
            assert!(!status.telemetry_observed);
        }
        if mode == "disconnect" {
            assert!(!status.telemetry_connected);
        }
        if mode == "overdue" {
            assert!(status.overdue);
        }
        if mode != "failure" {
            assert_eq!(
                fs::read_to_string(directory.join("installed/version.txt")).unwrap(),
                "2.0.0"
            );
        }
        // A single unchanged uninstall executable proves the same install is being serviced.
        assert!(directory.join("installed/unins000.exe").exists());
        assert!(!directory.join("installed/unins001.exe").exists());
    }
    assert!(
        Command::new(directory.join("installed/unins000.exe"))
            .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
            .status()
            .unwrap()
            .success()
    );
}
