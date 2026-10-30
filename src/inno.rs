//! Inno servicing through the existing signed ZIP, staging and helper lifecycle.
//!
//! See `docs/inno.md`. Blocking operations belong on worker threads. Inno owns
//! installation and repair; Freshen never attempts file-level rollback for it.
use crate::{
    Artifact, Error, PackageKind, Result,
    fsutil::atomic_write,
    install::{self, Plan},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[cfg(windows)]
#[path = "inno_pipe.rs"]
mod pipe;
#[path = "inno_wire.rs"]
mod wire;

/// Explicit local ownership and execution policy. Never populate this from a manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InnoPolicy {
    /// Exact signed inventory path of the installer.
    pub installer: String,
    /// Existing Inno AppId, including braces if originally used.
    pub app_id: String,
    /// Existing installation directory, matched against uninstall registration.
    pub directory: PathBuf,
    /// Relative installed executable; used only for optional relaunch.
    pub executable: String,
    /// Require HKLM registration and launch with runas (a UAC prompt may appear).
    pub machine_wide: bool,
    /// Offer optional telemetry. Failure falls back to indeterminate progress.
    pub enhanced: bool,
    /// Report an overdue warning after this interval; never terminate an installer.
    pub timeout_secs: u64,
    /// Relaunch after exit code 0. Never on reboot required or failure.
    pub relaunch: bool,
}
impl InnoPolicy {
    pub(crate) fn validate(&self, artifact: &Artifact) -> Result<()> {
        if !cfg!(windows) {
            return Err(Error::Invalid("Inno servicing requires Windows".into()));
        }
        crate::manifest::check_path(&self.installer)?;
        crate::manifest::check_path(&self.executable)?;
        if artifact.kind != PackageKind::Inno
            || artifact.files.len() != 1
            || artifact.files[0].path != self.installer
            || self.app_id.is_empty()
            || self.app_id.len() > 200
            || self
                .app_id
                .chars()
                .any(|c| c.is_control() || matches!(c, '\\' | '/' | '"'))
            || !self.directory.is_absolute()
            || !(1..=86400).contains(&self.timeout_secs)
        {
            return Err(Error::Invalid(
                "invalid local Inno policy or installer inventory".into(),
            ));
        }
        Ok(())
    }
}

/// Backend-neutral phase names. Only installer process exit establishes completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Preparing,
    Installing,
    Finalizing,
    Completed,
    Failed,
    RebootRequired,
}

/// Latest snapshot, atomically replaced at most ten times a second. No unbounded event log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub phase: Phase,
    /// Actual Inno values within Installing only; None means indeterminate.
    pub progress: Option<(u32, u32)>,
    /// Cancellation is supported by Handoff before parent shutdown only.
    pub cancellable: bool,
    pub telemetry_connected: bool,
    /// Retained even after normal DLL teardown disconnects the pipe.
    pub telemetry_observed: bool,
    pub progress_observed: bool,
    pub overdue: bool,
    pub diagnostic: Option<u32>,
    pub detail: String,
    pub exit_code: Option<u32>,
}
impl Default for Status {
    fn default() -> Self {
        Self {
            phase: Phase::Preparing,
            progress: None,
            cancellable: false,
            telemetry_connected: false,
            telemetry_observed: false,
            progress_observed: false,
            overdue: false,
            diagnostic: None,
            detail: String::new(),
            exit_code: None,
        }
    }
}
impl Status {
    #[cfg(any(windows, test))]
    fn accept(&mut self, message: wire::Message, token: &[u8; 32]) -> bool {
        if &message.token != token {
            return false;
        }
        match message.kind {
            1 => {} // Preparing cannot regress a phase after process launch.
            2 if self.phase == Phase::Installing => {}
            3 if matches!(self.phase, Phase::Installing | Phase::Finalizing) => {
                self.phase = Phase::Finalizing;
                self.progress = None;
            }
            4 if self.phase == Phase::Installing
                && message.total > 0
                && message.current <= message.total =>
            {
                if self.progress.is_some_and(|(current, total)| {
                    total == message.total && current > message.current
                }) {
                    return false;
                }
                self.progress = Some((message.current, message.total));
                self.progress_observed = true;
            }
            5 => self.diagnostic = Some(message.current),
            _ => return false,
        }
        self.telemetry_connected = true;
        self.telemetry_observed = true;
        true
    }
    #[cfg(any(windows, test))]
    fn complete(&mut self, code: u32) {
        self.exit_code = Some(code);
        self.telemetry_connected = false;
        self.progress = None;
        self.phase = match code {
            0 => Phase::Completed,
            3010 => Phase::RebootRequired,
            _ => Phase::Failed,
        };
        self.detail = exit_description(code).into();
    }
}

/// Interpret documented Inno exit codes, retaining unknown codes as failures.
pub fn exit_description(code: u32) -> &'static str {
    match code {
        0 => "installer completed successfully",
        1 => "installer initialization failed",
        2 => "installation cancelled before installation",
        3 => "fatal error preparing installation",
        4 => "fatal error during installation",
        5 => "installation cancelled during installation",
        6 => "installer forcibly terminated by debugger",
        7 => "preparing installation failed; cannot proceed",
        8 => "preparing installation requires restart; cannot proceed",
        3010 => "installation completed; reboot required",
        _ => "unknown installer exit code; installation failed",
    }
}

/// Poll from a surviving host UI or inspect on next launch. Announce phase changes
/// and coarse progress milestones, not every snapshot, to assistive technology.
pub fn status(root: &Path) -> Result<Option<Status>> {
    let path = install::transaction(root)?.join("inno-status.json");
    match fs::metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
        Ok(metadata) if metadata.len() > 16384 => Err(Error::SizeLimit),
        Ok(_) => Ok(Some(serde_json::from_slice(&fs::read(path)?)?)),
    }
}
fn save(root: &Path, status: &Status) -> Result<()> {
    atomic_write(
        &install::transaction(root)?.join("inno-status.json"),
        &serde_json::to_vec(status)?,
    )
}

pub(crate) fn preflight(root: &Path, policy: &InnoPolicy) -> Result<()> {
    let directory = fs::canonicalize(&policy.directory)?;
    let state = fs::canonicalize(root)?;
    if state.starts_with(&directory) || directory.starts_with(&state) {
        return Err(Error::Invalid(
            "Inno servicing state must be separate from the application directory".into(),
        ));
    }
    #[cfg(windows)]
    windows::registered(policy)?;
    Ok(())
}

pub(crate) fn execute(plan: &Plan, policy: &InnoPolicy) -> Result<()> {
    if install::installation_status(&plan.policy.root)?
        .is_none_or(|s| s.state != crate::InstallState::Prepared)
    {
        return Err(Error::Recovery(
            "Inno transaction is not prepared; refusing to replay installer".into(),
        ));
    }
    #[cfg(windows)]
    let result = windows::execute(plan, policy);
    #[cfg(not(windows))]
    let result: Result<()> = {
        let _ = policy;
        Err(Error::Invalid("Inno requires Windows".into()))
    };
    if let Err(error) = &result {
        let mut snapshot = status(&plan.policy.root)?.unwrap_or_default();
        snapshot.phase = Phase::Failed;
        snapshot.progress = None;
        snapshot.detail = error.to_string();
        save(&plan.policy.root, &snapshot)?;
        install::finish(
            &plan.policy.root,
            crate::InstallState::Failed,
            error.to_string(),
        )?;
    }
    result
}

/// Fixed switches; no caller-supplied switches may override safety or install identity.
#[cfg(windows)]
fn arguments(policy: &InnoPolicy, log: &Path) -> Vec<String> {
    vec![
        "/SP-".into(),
        "/VERYSILENT".into(),
        "/SUPPRESSMSGBOXES".into(),
        "/NORESTART".into(),
        "/RESTARTEXITCODE=3010".into(),
        "/NORESTARTAPPLICATIONS".into(),
        "/NOCLOSEAPPLICATIONS".into(),
        format!("/DIR={}", policy.directory.display()),
        format!("/LOG={}", log.display()),
        if policy.machine_wide {
            "/ALLUSERS".into()
        } else {
            "/CURRENTUSER".into()
        },
    ]
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{
        process::{Child, Command, Stdio},
        ptr, thread,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::*,
        System::{Registry::*, Threading::*},
        UI::{Shell::*, WindowsAndMessaging::SW_HIDE},
    };

    pub(super) fn registered(policy: &InnoPolicy) -> Result<()> {
        let key = pipe::wide(&format!(
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{}_is1",
            policy.app_id
        ));
        let name = pipe::wide("InstallLocation");
        let mut matched = false;
        for view in [RRF_SUBKEY_WOW6464KEY, RRF_SUBKEY_WOW6432KEY] {
            let mut buffer = [0u16; 32768];
            let mut size = std::mem::size_of_val(&buffer) as u32;
            let code = unsafe {
                RegGetValueW(
                    if policy.machine_wide {
                        HKEY_LOCAL_MACHINE
                    } else {
                        HKEY_CURRENT_USER
                    },
                    key.as_ptr(),
                    name.as_ptr(),
                    RRF_RT_REG_SZ | view,
                    ptr::null_mut(),
                    buffer.as_mut_ptr().cast(),
                    &mut size,
                )
            };
            if code == 0 {
                let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
                let directory = PathBuf::from(
                    String::from_utf16(&buffer[..len])
                        .map_err(|_| Error::Invalid("invalid registered directory".into()))?,
                );
                if fs::canonicalize(directory)? != fs::canonicalize(&policy.directory)? {
                    return Err(Error::Invalid(
                        "Inno registration directory differs from local policy".into(),
                    ));
                }
                matched = true;
            }
        }
        if !matched {
            return Err(Error::Invalid(
                "existing Inno AppId registration not found in selected privilege scope".into(),
            ));
        }
        Ok(())
    }

    // Windows argv quoting, including paths ending in a backslash.
    fn quote(argument: &str) -> String {
        let mut result = String::from("\"");
        let mut slashes = 0;
        for c in argument.chars() {
            if c == '\\' {
                slashes += 1;
                continue;
            }
            if c == '"' {
                result.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
            } else {
                result.extend(std::iter::repeat_n('\\', slashes));
            }
            slashes = 0;
            result.push(c);
        }
        result.extend(std::iter::repeat_n('\\', slashes * 2));
        result.push('"');
        result
    }
    enum Process {
        User(Child),
        Elevated(HANDLE),
    }
    impl Drop for Process {
        fn drop(&mut self) {
            if let Self::Elevated(handle) = self {
                unsafe {
                    CloseHandle(*handle);
                }
            }
        }
    }
    impl Process {
        fn launch(path: &Path, args: &[String], elevated: bool) -> Result<Self> {
            if !elevated {
                use std::os::windows::process::CommandExt;
                return Ok(Self::User(
                    Command::new(path)
                        .args(args)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .creation_flags(CREATE_NO_WINDOW)
                        .spawn()?,
                ));
            }
            use std::os::windows::ffi::OsStrExt;
            let file: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let parameters =
                pipe::wide(&args.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" "));
            let verb = pipe::wide("runas");
            let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
            info.cbSize = std::mem::size_of_val(&info) as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
            info.lpVerb = verb.as_ptr();
            info.lpFile = file.as_ptr();
            info.lpParameters = parameters.as_ptr();
            info.nShow = SW_HIDE;
            if unsafe { ShellExecuteExW(&mut info) } == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            if info.hProcess.is_null() {
                return Err(Error::Helper("elevation returned no process handle".into()));
            }
            Ok(Self::Elevated(info.hProcess))
        }
        fn poll(&mut self) -> Result<Option<u32>> {
            match self {
                Self::User(child) => Ok(child.try_wait()?.map(|s| s.code().unwrap_or(-1) as u32)),
                Self::Elevated(handle) => match unsafe { WaitForSingleObject(*handle, 0) } {
                    WAIT_TIMEOUT => Ok(None),
                    WAIT_OBJECT_0 => {
                        let mut code = 0;
                        if unsafe { GetExitCodeProcess(*handle, &mut code) } == 0 {
                            return Err(std::io::Error::last_os_error().into());
                        }
                        Ok(Some(code))
                    }
                    _ => Err(std::io::Error::last_os_error().into()),
                },
            }
        }
    }
    pub(super) fn execute(plan: &Plan, policy: &InnoPolicy) -> Result<()> {
        registered(policy)?;
        install::verify_stage(plan)?;
        let root = &plan.policy.root;
        let txn = install::transaction(root)?;
        let mut snapshot = Status::default();
        save(root, &snapshot)?;
        let mut random = [0; 32];
        getrandom::fill(&mut random).map_err(|e| Error::Helper(e.to_string()))?;
        let endpoint = format!(r"\\.\pipe\freshen-inno-{}", hex::encode(random));
        let mut pipe = if policy.enhanced {
            pipe::Pipe::server(&endpoint).ok()
        } else {
            None
        };
        let mut args = arguments(policy, &txn.join("inno.log"));
        if pipe.is_some() {
            args.push(format!("/FRESHENPIPE={endpoint}"));
            args.push(format!("/FRESHENTOKEN={}", hex::encode(random)));
        }
        install::finish(
            root,
            crate::InstallState::Applying,
            "Inno installation; automatic rollback unavailable".into(),
        )?;
        let mut process = Process::launch(
            &txn.join("stage").join(&policy.installer),
            &args,
            policy.machine_wide,
        )?;
        snapshot.phase = Phase::Installing;
        let _ = save(root, &snapshot);
        let start = Instant::now();
        let mut connected = false;
        loop {
            let mut broken = false;
            if let Some(server) = &pipe {
                if !connected {
                    match server.connect() {
                        Ok(value) => connected = value,
                        Err(_) => broken = true,
                    }
                }
                if connected {
                    for _ in 0..16 {
                        match server.read() {
                            Ok(Some(bytes)) => {
                                if !wire::Message::decode(&bytes)
                                    .is_some_and(|m| snapshot.accept(m, &random))
                                {
                                    broken = true;
                                    break;
                                }
                            }
                            Ok(None) => break,
                            Err(_) => {
                                broken = true;
                                break;
                            }
                        }
                    }
                }
                if !connected && start.elapsed() > Duration::from_secs(10) {
                    broken = true;
                }
            }
            if broken {
                pipe = None;
                snapshot.telemetry_connected = false;
                snapshot.progress = None;
                snapshot.detail = "optional telemetry unavailable; installation continues".into();
            }
            if let Some(code) = process.poll()? {
                snapshot.complete(code);
                save(root, &snapshot)?;
                let state = match snapshot.phase {
                    Phase::Completed => crate::InstallState::Committed,
                    Phase::RebootRequired => crate::InstallState::RebootRequired,
                    _ => crate::InstallState::Failed,
                };
                install::finish(
                    root,
                    state,
                    format!("{} (exit {code}); see inno.log", snapshot.detail),
                )?;
                if code != 0 && code != 3010 {
                    return Err(Error::Helper(format!("{} (exit {code})", snapshot.detail)));
                }
                if code == 0 && policy.relaunch {
                    // A successful installer does not imply the relaunched app is healthy.
                    let relaunch = (|| -> Result<()> {
                        Command::new(crate::fsutil::safe_join(
                            &policy.directory,
                            &policy.executable,
                        )?)
                        .args(&plan.relaunch_args)
                        .current_dir(&policy.directory)
                        .spawn()?;
                        Ok(())
                    })();
                    if let Err(error) = relaunch {
                        snapshot.detail =
                            format!("installation succeeded; relaunch failed: {error}");
                        let _ = save(root, &snapshot);
                        let _ = install::finish(
                            root,
                            crate::InstallState::Committed,
                            snapshot.detail.clone(),
                        );
                    }
                }
                return Ok(());
            }
            snapshot.overdue = start.elapsed() > Duration::from_secs(policy.timeout_secs);
            // Persisting a snapshot is best effort after launch: disk failure must not
            // abandon supervision or release the installation lock while Inno runs.
            let _ = save(root, &snapshot);
            thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn windows_argument_quoting() {
        assert_eq!(
            quote("/DIR=C:\\Program Files\\App\\"),
            "\"/DIR=C:\\Program Files\\App\\\\\""
        );
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
    }

    #[test]
    fn process_polling_tracks_lifetime_and_spawn_errors() {
        let executable = std::env::current_exe().unwrap();
        let mut child = Process::launch(
            &executable,
            &["--exact".into(), "inno::windows::slow_child".into()],
            false,
        )
        .unwrap();
        assert!(child.poll().unwrap().is_none());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(code) = child.poll().unwrap() {
                assert_eq!(code, 0);
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            Process::launch(Path::new(r"Z:\freshen-nonexistent\setup.exe"), &[], false).is_err()
        );
        let mut failed = Process::launch(
            &executable,
            &["--invalid-freshen-test-option".into()],
            false,
        )
        .unwrap();
        loop {
            if let Some(code) = failed.poll().unwrap() {
                assert_ne!(code, 0);
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn slow_child() {
        thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_inno_inventory_is_exactly_one_executable() {
        let mut manifest = crate::tests::manifest();
        manifest.artifacts[0].kind = PackageKind::Inno;
        assert!(manifest.validate().is_ok());
        let file = manifest.artifacts[0].files[0].clone();
        manifest.artifacts[0].files.push(file);
        assert!(manifest.validate().is_err());
        manifest.artifacts[0].files.clear();
        assert!(manifest.validate().is_err());
    }
    #[test]
    fn authenticated_progress_never_commits_or_regresses() {
        let mut status = Status {
            phase: Phase::Installing,
            ..Status::default()
        };
        let mut message = wire::Message {
            token: [9; 32],
            kind: 4,
            current: 4,
            total: 10,
        };
        assert!(!status.accept(message, &[8; 32]));
        assert!(status.accept(message, &[9; 32]));
        message.current = 3;
        assert!(!status.accept(message, &[9; 32]));
        message.current = 10;
        assert!(status.accept(message, &[9; 32]));
        assert_eq!(status.phase, Phase::Installing);
        message.kind = 3;
        assert!(status.accept(message, &[9; 32]));
        assert!(status.progress.is_none());
        message.kind = 4;
        assert!(!status.accept(message, &[9; 32]));
        status.complete(4);
        assert_eq!(status.phase, Phase::Failed);
    }
    #[test]
    fn actual_exit_status_is_authoritative() {
        for code in [0, 1, 2, 3, 4, 5, 6, 7, 8, 3010, 9000, u32::MAX] {
            let mut status = Status::default();
            status.complete(code);
            assert_eq!(
                status.phase,
                match code {
                    0 => Phase::Completed,
                    3010 => Phase::RebootRequired,
                    _ => Phase::Failed,
                }
            );
            assert_eq!(status.exit_code, Some(code));
        }
    }
}
