use crate::{
    Error, InstallState, Result,
    archive::hash_file,
    fsutil::{atomic_write, copy_file, safe_join},
    install::{self, Plan, apply, finish, load_plan, lock, rollback, transaction},
};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// A helper is ready, but installation will not start until this process exits.
/// Dropping this value does not cancel; call `cancel` if shutdown is declined.
pub struct Handoff {
    root: PathBuf,
    child: Child,
}
impl Handoff {
    pub fn cancel(mut self) -> Result<()> {
        atomic_write(&transaction(&self.root)?.join("cancel"), b"cancel")?;
        let status = self.child.wait()?;
        if !status.success() {
            return Err(Error::Helper("helper did not cancel cleanly".into()));
        }
        Ok(())
    }
    pub fn helper_pid(&self) -> u32 {
        self.child.id()
    }
}

pub(crate) fn launch(plan: &Plan, executable: &Path) -> Result<Handoff> {
    let txn = transaction(&plan.policy.root)?;
    let helper = txn.join(if cfg!(windows) {
        "helper.exe"
    } else {
        "helper"
    });
    copy_file(executable, &helper)?;
    let mut command = Command::new(&helper);
    command
        .arg("--freshen-apply")
        .arg(txn.join("plan.json"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hidden(&mut command);
    let mut child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if fs::read_to_string(txn.join("ready")).is_ok_and(|text| text == plan.token) {
            return Ok(Handoff {
                root: plan.policy.root.clone(),
                child,
            });
        }
        if let Some(status) = child.try_wait()? {
            let detail = fs::read_to_string(txn.join("helper-error.txt"))
                .unwrap_or_else(|_| status.to_string());
            return Err(Error::Helper(detail));
        }
        thread::sleep(Duration::from_millis(50));
    }
    atomic_write(&txn.join("cancel"), b"cancel")?;
    // Parent is still alive, so the helper cannot be modifying program files.
    let _ = child.kill();
    let _ = child.wait();
    Err(Error::Helper("helper readiness timed out".into()))
}

fn hidden(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// Call before initializing the application or acquiring its single-instance
/// lock if using the application's executable as its own helper. Returns false
/// for ordinary application arguments. Never exits the process or creates UI.
pub fn run_helper(arguments: impl IntoIterator<Item = OsString>) -> Result<bool> {
    let args: Vec<_> = arguments.into_iter().collect();
    if args.first().is_none_or(|arg| arg != "--freshen-apply") {
        return Ok(false);
    }
    if args.len() != 2 {
        return Err(Error::Helper("expected --freshen-apply PLAN".into()));
    }
    let path = PathBuf::from(&args[1]);
    let plan = load_plan(&path)?;
    let _guard = lock(&plan.policy.root)?;
    let result = execute(&plan);
    if let Err(error) = &result {
        let txn = transaction(&plan.policy.root)?;
        let _ = atomic_write(&txn.join("helper-error.txt"), error.to_string().as_bytes());
    }
    result.map(|()| true)
}

fn execute(plan: &Plan) -> Result<()> {
    let txn = transaction(&plan.policy.root)?;
    install::verify_stage(plan)?;
    let parent = Parent::open(plan.parent_pid)?;
    atomic_write(&txn.join("ready"), plan.token.as_bytes())?;
    let deadline = Instant::now() + Duration::from_secs(plan.policy.exit_timeout_secs);
    loop {
        if txn.join("cancel").exists() {
            return Ok(());
        }
        if parent.exited()? {
            break;
        }
        if Instant::now() >= deadline {
            return Err(Error::Helper(
                "application did not exit before the deadline".into(),
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
    if txn.join("cancel").exists() {
        return Ok(());
    }
    if let Err(error) = apply(plan) {
        // apply attempts rollback itself; leave a persistent failure if that also failed.
        if install::installation_status(&plan.policy.root)?
            .is_some_and(|s| s.state == InstallState::RollingBack)
        {
            let _ = finish(&plan.policy.root, InstallState::Failed, error.to_string());
        }
        return Err(error);
    }
    let executable = safe_join(&plan.policy.root, plan.policy.executable())?;
    let mut command = Command::new(executable);
    command
        .args(&plan.relaunch_args)
        .current_dir(&plan.policy.root)
        .env("FRESHEN_CONFIRM_TOKEN", &plan.token)
        .env("FRESHEN_CONFIRM_ROOT", &plan.policy.root);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            rollback(plan, format!("relaunch failed: {error}"))?;
            return Err(error.into());
        }
    };
    let deadline = Instant::now() + Duration::from_secs(plan.policy.confirmation_timeout_secs);
    loop {
        if fs::read_to_string(txn.join("confirmed")).is_ok_and(|text| text == plan.token) {
            finish(
                &plan.policy.root,
                InstallState::Committed,
                "new application acknowledged startup".into(),
            )?;
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            rollback(
                plan,
                format!("new application exited without confirming startup: {status}"),
            )?;
            return Err(Error::Helper(
                "new application failed to confirm startup; rollback completed".into(),
            ));
        }
        if Instant::now() >= deadline {
            let detail = "startup confirmation timed out; new process remains running; stop it before recovery";
            finish(&plan.policy.root, InstallState::Failed, detail.into())?;
            return Err(Error::Recovery(detail.into()));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Call after successful initialization in the updated application. Returns false
/// for a normal launch. Acknowledgment is accepted only from the expected new
/// executable, whose hash is checked against the signed manifest.
pub fn confirm_startup(root: &Path) -> Result<bool> {
    let Some(token) = std::env::var_os("FRESHEN_CONFIRM_TOKEN") else {
        return Ok(false);
    };
    let Some(expected_root) = std::env::var_os("FRESHEN_CONFIRM_ROOT") else {
        return Ok(false);
    };
    let root = fs::canonicalize(root)?;
    if root != fs::canonicalize(PathBuf::from(expected_root))? {
        return Err(Error::Invalid("confirmation root mismatch".into()));
    }
    let txn = transaction(&root)?;
    let plan = load_plan(&txn.join("plan.json"))?;
    if token != plan.token.as_str() {
        return Err(Error::Invalid("confirmation token mismatch".into()));
    }
    let expected = fs::canonicalize(safe_join(&root, plan.policy.executable())?)?;
    if fs::canonicalize(std::env::current_exe()?)? != expected {
        return Err(Error::Invalid("unexpected confirming executable".into()));
    }
    let release = plan.trust.verify(&plan.signed)?;
    let artifact = release
        .artifacts
        .iter()
        .find(|a| a.target == plan.target)
        .ok_or_else(|| Error::Invalid("missing artifact".into()))?;
    let file = artifact
        .files
        .iter()
        .find(|f| f.path == plan.policy.executable())
        .ok_or_else(|| Error::Invalid("missing executable".into()))?;
    if !hash_file(&expected)?.eq_ignore_ascii_case(&file.sha256) {
        return Err(Error::HashMismatch(file.path.clone()));
    }
    if install::installation_status(&root)?
        .is_none_or(|s| s.state != InstallState::AwaitingConfirmation)
    {
        return Err(Error::Invalid(
            "transaction is not awaiting confirmation".into(),
        ));
    }
    atomic_write(&txn.join("confirmed"), plan.token.as_bytes())?;
    Ok(true)
}

#[cfg(windows)]
struct Parent(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl Parent {
    fn open(pid: u32) -> Result<Self> {
        // SYNCHRONIZE grants waiting only, not process termination or memory access.
        let handle =
            unsafe { windows_sys::Win32::System::Threading::OpenProcess(0x0010_0000, 0, pid) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self(handle))
    }
    fn exited(&self) -> Result<bool> {
        let result =
            unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(self.0, 0) };
        match result {
            0 => Ok(true),
            258 => Ok(false),
            _ => Err(std::io::Error::last_os_error().into()),
        }
    }
}
#[cfg(windows)]
impl Drop for Parent {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(unix)]
struct Parent(libc::pid_t);
#[cfg(unix)]
impl Parent {
    fn open(pid: u32) -> Result<Self> {
        let pid = libc::pid_t::try_from(pid)
            .map_err(|_| Error::Invalid("parent PID out of range".into()))?;
        if pid <= 0 {
            return Err(Error::Invalid("parent PID must be positive".into()));
        }
        Ok(Self(pid))
    }
    fn exited(&self) -> Result<bool> {
        if unsafe { libc::kill(self.0, 0) } == 0 {
            return Ok(false);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(true)
        } else if error.raw_os_error() == Some(libc::EPERM) {
            Ok(false)
        } else {
            Err(error.into())
        }
    }
}
