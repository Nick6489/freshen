//! The test host owns its native dialogs and scheduling; Freshen remains GUI-free.
#![allow(unsafe_op_in_unsafe_fn)] // All HWND/context access is confined to the dialog thread.
use base64::{Engine, engine::general_purpose::STANDARD};
use freshen::{
    inno::{self, InnoPolicy, Phase},
    *,
};
use std::{
    cell::RefCell,
    fs::{self, File},
    io::{Read, Write},
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{OpenProcess, WaitForSingleObject},
    },
    UI::{
        Controls::*,
        Input::KeyboardAndMouse::{EnableWindow, SetFocus},
        WindowsAndMessaging::*,
    },
};

const VERSION: &str = env!("FRESHEN_UI_VERSION");
const KEY: &str = env!("FRESHEN_UI_PUBLIC_KEY");
const APP_ID: &str = "FreshenNativeDialogTest";
const EXE: &str = "freshen-native-test.exe";
const VERSION_ID: i32 = 1001;
const STATUS: i32 = 1003;
const CHECK: i32 = 1004;
const INSTALL: i32 = 1005;
const PROGRESS: i32 = 1006;
const OPEN: i32 = 1007;
const TIMER: usize = 1;
// Win32's pointer-size-correct DWLP_USER = sizeof(LRESULT) + sizeof(DLGPROC).
const DIALOG_USER: i32 = (2 * std::mem::size_of::<isize>()) as i32;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn local(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA")).join(name)
}
pub fn state_directory() -> PathBuf {
    local("FreshenNativeDialogTestServicing")
}

/// Deliberate offline fixture transport. Only three fixed URLs map to local feed files.
/// Publisher signatures, size limits and installer/stage verification are unchanged.
struct LocalFeed(PathBuf);
impl Transport for LocalFeed {
    fn download(
        &self,
        url: &url::Url,
        output: &mut dyn Write,
        limit: u64,
        cancel: &Cancellation,
        events: &mut dyn FnMut(Event),
    ) -> Result<u64> {
        let name = match (url.scheme(), url.host_str(), url.path()) {
            ("https", Some("freshen-ui-test.invalid"), "/manifest.json") => "manifest.json",
            ("https", Some("freshen-ui-test.invalid"), "/manifest.json.sig") => "manifest.json.sig",
            ("https", Some("freshen-ui-test.invalid"), "/update.zip") => "update.zip",
            _ => {
                return Err(Error::Invalid(
                    "URL is not part of the local UI test feed".into(),
                ));
            }
        };
        cancel.check()?;
        let mut file = File::open(self.0.join(name))?;
        let total = file.metadata()?.len();
        if total > limit {
            return Err(Error::SizeLimit);
        }
        let mut received = 0u64;
        let mut buffer = [0; 65536];
        loop {
            cancel.check()?;
            let size = file.read(&mut buffer)?;
            if size == 0 {
                break;
            }
            received = received.checked_add(size as u64).ok_or(Error::SizeLimit)?;
            if received > limit {
                return Err(Error::SizeLimit);
            }
            output.write_all(&buffer[..size])?;
            events(Event::Downloading {
                received,
                total: Some(total),
            });
        }
        Ok(received)
    }
}

struct Work {
    status: String,
    busy: bool,
    arming: bool,
    armed: bool,
    prepared: Option<PreparedUpdate>,
    cancel: Cancellation,
}
impl Default for Work {
    fn default() -> Self {
        Self {
            status: "Ready. A local test update can be checked without using the network.".into(),
            busy: false,
            arming: false,
            armed: false,
            prepared: None,
            cancel: Cancellation::default(),
        }
    }
}
struct Context {
    work: Arc<Mutex<Work>>,
    monitor: bool,
    ready: Option<PathBuf>,
    installed: PathBuf,
    closing: bool,
    terminal: bool,
    succeeded: bool,
    text: String,
    progress: Option<u32>,
    last_progress: Instant,
    helper: Option<std::os::windows::io::OwnedHandle>,
}
fn status(work: &Arc<Mutex<Work>>, text: impl Into<String>) {
    work.lock().unwrap().status = text.into();
}
fn check(work: Arc<Mutex<Work>>) {
    let cancel = work.lock().unwrap().cancel.clone();
    let result = (|| -> Result<Option<PreparedUpdate>> {
        let key: [u8; 32] = STANDARD
            .decode(KEY)
            .map_err(|_| {
                Error::Invalid("test publisher key is not provisioned; run build-demo.ps1".into())
            })?
            .try_into()
            .map_err(|_| {
                Error::Invalid("test publisher key is not provisioned; run build-demo.ps1".into())
            })?;
        let updater = Updater {
            product: "freshen-native-dialog-test".into(),
            channel: "stable".into(),
            current_version: VERSION.parse().unwrap(),
            target: "x86_64-pc-windows-msvc".into(),
            trust: TrustStore::new(vec![key])?,
            transport: LocalFeed(local("FreshenNativeDialogTestFeed")),
            max_download: 128 * 1024 * 1024,
            max_unpacked: 256 * 1024 * 1024,
        };
        let source = ReleaseSource::Manifest {
            document: "https://freshen-ui-test.invalid/manifest.json"
                .parse()
                .unwrap(),
            signature: "https://freshen-ui-test.invalid/manifest.json.sig"
                .parse()
                .unwrap(),
        };
        let mut events = |event| match event {
            Event::Checking => status(&work, "Checking for updates..."),
            Event::Downloading { .. } => status(&work, "Downloading the test update..."),
            Event::Verifying => status(
                &work,
                "Verifying the publisher signature and update files...",
            ),
            Event::Extracting { .. } => status(&work, "Preparing the test update..."),
            _ => {}
        };
        let Some(candidate) = updater.check(&source, &cancel, &mut events)? else {
            return Ok(None);
        };
        let state = state_directory();
        fs::create_dir_all(&state)?;
        updater
            .prepare(candidate, &state, &cancel, &mut events)
            .map(Some)
    })();
    let mut state = work.lock().unwrap();
    state.busy = false;
    match result {
        Ok(Some(prepared)) => {
            state.status = format!(
                "Update {} is ready. Choose Install update to close this app and install it.",
                prepared.release().version
            );
            state.prepared = Some(prepared);
        }
        Ok(None) => state.status = "No updates available. This is the latest test version.".into(),
        Err(Error::Cancelled) => state.status = "Update check cancelled.".into(),
        Err(error) => state.status = format!("Could not check for updates: {error}"),
    }
}
fn install(work: Arc<Mutex<Work>>, installed: PathBuf, prepared: PreparedUpdate) {
    let result = (|| -> Result<()> {
        let state = state_directory();
        if installation_status(&state)?.is_some() {
            return Err(Error::Recovery("a previous test transaction is retained; inspect its logs and use the documented reset procedure".into()));
        }
        let policy = InstallPolicy {
            root: state.clone(),
            layout: InstallLayout::Inno(InnoPolicy {
                installer: "setup.exe".into(),
                app_id: APP_ID.into(),
                directory: installed.clone(),
                executable: EXE.into(),
                machine_wide: false,
                enhanced: true,
                timeout_secs: 120,
                relaunch: false,
            }),
            exit_timeout_secs: 60,
            confirmation_timeout_secs: 60,
        };
        let handoff = prepared.arm(policy, &installed.join("freshen-helper.exe"), vec![])?;
        // Presentation survives in a separate copy. Installation still belongs to
        // the existing Freshen helper, with its original parent-exit handshake.
        let ready = state.join(format!("monitor-ready-{}", handoff.helper_pid()));
        let monitor = state.join(format!("ui-monitor-{}.exe", handoff.helper_pid()));
        let launch = (|| -> Result<()> {
            fs::copy(std::env::current_exe()?, &monitor)?;
            let mut child = Command::new(&monitor)
                .arg("--monitor")
                .arg(&installed)
                .arg(&ready)
                .spawn()?;
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline {
                if ready.is_file() {
                    return Ok(());
                }
                if child.try_wait()?.is_some() {
                    return Err(Error::Helper(
                        "update dialog exited before readiness".into(),
                    ));
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(Error::Helper("update dialog did not become ready".into()))
        })();
        if let Err(error) = launch {
            handoff.cancel()?;
            return Err(error);
        }
        // Dropping Handoff intentionally leaves the armed helper alive.
        work.lock().unwrap().armed = true;
        Ok(())
    })();
    if let Err(error) = result {
        let mut state = work.lock().unwrap();
        state.busy = false;
        state.arming = false;
        state.status = format!("Could not start the update: {error}");
    }
}

unsafe fn text(hwnd: HWND, id: i32, value: &str) {
    SetDlgItemTextW(hwnd, id, wide(value).as_ptr());
}
unsafe fn show_status(hwnd: HWND, ctx: &mut Context, value: String) {
    if ctx.text != value {
        text(hwnd, STATUS, &value);
        ctx.text = value;
    }
}
unsafe fn monitor_tick(hwnd: HWND, ctx: &mut Context) {
    use std::os::windows::io::AsRawHandle;
    if let Some(path) = ctx.ready.take() {
        let _ = fs::write(path, b"native dialog ready");
    }
    match inno::status(&state_directory()) {
        Ok(Some(snapshot)) => {
            let was_terminal = ctx.terminal;
            ctx.terminal = matches!(
                snapshot.phase,
                Phase::Completed | Phase::Failed | Phase::RebootRequired
            );
            ctx.succeeded = snapshot.phase == Phase::Completed;
            let message=match snapshot.phase {
                Phase::Preparing=>"Preparing to install. The test app is closing.".into(),
                Phase::Installing if snapshot.overdue=>"Installation is taking longer than expected. It is still running; cancellation is unavailable.".into(),
                Phase::Installing=>"Installing the update. Cancellation is unavailable while files are being replaced.".into(),
                Phase::Finalizing=>"Finalizing the update. Waiting for the installer to finish.".into(),
                Phase::Completed=>"Update installed successfully. You can open version 2.0.0.".into(),
                Phase::RebootRequired=>"Update installed. Windows must be restarted before using the new version.".into(),
                Phase::Failed=>format!("Update failed: {}",snapshot.detail),
            };
            show_status(hwnd, ctx, message);
            EnableWindow(GetDlgItem(hwnd, IDCANCEL), i32::from(ctx.terminal));
            EnableWindow(GetDlgItem(hwnd, OPEN), i32::from(ctx.succeeded));
            let progress = if snapshot.phase == Phase::Installing {
                snapshot
                    .progress
                    .map(|(current, total)| ((current as u64 * 100) / (total as u64)) as u32)
            } else {
                None
            };
            let bar = GetDlgItem(hwnd, PROGRESS);
            if let Some(percent) = progress {
                if ctx.progress.is_none()
                    || (ctx.last_progress.elapsed() >= Duration::from_secs(1)
                        && ctx.progress != progress)
                {
                    SendMessageW(bar, PBM_SETMARQUEE, 0, 0);
                    let style = GetWindowLongPtrW(bar, GWL_STYLE) & !(PBS_MARQUEE as isize);
                    SetWindowLongPtrW(bar, GWL_STYLE, style);
                    SendMessageW(bar, PBM_SETPOS, percent as usize, 0);
                    ctx.progress = progress;
                    ctx.last_progress = Instant::now();
                }
            } else if ctx.progress.is_some() || (ctx.terminal && !was_terminal) {
                SendMessageW(bar, PBM_SETMARQUEE, 0, 0);
                let style = GetWindowLongPtrW(bar, GWL_STYLE);
                SetWindowLongPtrW(
                    bar,
                    GWL_STYLE,
                    if ctx.terminal {
                        style & !(PBS_MARQUEE as isize)
                    } else {
                        style | PBS_MARQUEE as isize
                    },
                );
                if ctx.terminal {
                    SendMessageW(bar, PBM_SETPOS, if ctx.succeeded { 100 } else { 0 }, 0);
                } else {
                    SendMessageW(bar, PBM_SETMARQUEE, 1, 40);
                }
                ctx.progress = None;
            }
        }
        Ok(None) => {
            // The helper's process handle below distinguishes waiting from loss.
        }
        Err(error) => show_status(
            hwnd,
            ctx,
            format!("Unable to read update status: {error}. Installer outcome is unknown."),
        ),
    }
    let helper_exited = ctx
        .helper
        .as_ref()
        .is_none_or(|h| WaitForSingleObject(h.as_raw_handle(), 0) == WAIT_OBJECT_0);
    if helper_exited && !ctx.terminal {
        ctx.terminal = true;
        ctx.succeeded = false;
        EnableWindow(GetDlgItem(hwnd, IDCANCEL), 1);
        let detail = fs::read_to_string(
            state_directory().join(".freshen/transaction/helper-error.txt"),
        )
        .unwrap_or_else(|_| {
            "The helper exited without a final result. Inspect the test logs before recovery."
                .into()
        });
        show_status(
            hwnd,
            ctx,
            format!("Update outcome requires inspection: {detail}"),
        );
    }
}

unsafe extern "system" fn dialog(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    if message == WM_INITDIALOG {
        SetWindowLongPtrW(hwnd, DIALOG_USER, lparam);
    }
    let pointer = GetWindowLongPtrW(hwnd, DIALOG_USER) as *const RefCell<Context>;
    if pointer.is_null() {
        return 0;
    }
    let Ok(mut ctx) = (&*pointer).try_borrow_mut() else {
        return 0;
    };
    match message {
        WM_INITDIALOG => {
            SetTimer(hwnd, TIMER, 250, None);
            if ctx.monitor {
                text(
                    hwnd,
                    STATUS,
                    "Preparing to install. The test app is closing.",
                );
                SendMessageW(GetDlgItem(hwnd, PROGRESS), PBM_SETRANGE32, 0, 100);
                SendMessageW(GetDlgItem(hwnd, PROGRESS), PBM_SETMARQUEE, 1, 40);
                SetFocus(GetDlgItem(hwnd, STATUS));
            } else {
                text(hwnd, VERSION_ID, &format!("Installed version: {VERSION}"));
                let initial = ctx.work.lock().unwrap().status.clone();
                show_status(hwnd, &mut ctx, initial);
                SetFocus(GetDlgItem(hwnd, CHECK));
            }
            0 // Initial focus was explicitly assigned; dialog manager preserves it.
        }
        WM_TIMER => {
            if ctx.monitor {
                monitor_tick(hwnd, &mut ctx);
            } else {
                let (value, busy, prepared, armed) = {
                    let state = ctx.work.lock().unwrap();
                    (
                        state.status.clone(),
                        state.busy,
                        state.prepared.is_some(),
                        state.armed,
                    )
                };
                show_status(hwnd, &mut ctx, value);
                EnableWindow(GetDlgItem(hwnd, CHECK), i32::from(!busy));
                EnableWindow(GetDlgItem(hwnd, INSTALL), i32::from(!busy && prepared));
                if armed || (ctx.closing && !busy) {
                    EndDialog(hwnd, 0);
                }
            }
            1
        }
        WM_COMMAND => {
            let id = (wparam & 0xffff) as i32;
            if !ctx.monitor && id == CHECK {
                let work = ctx.work.clone();
                let mut state = work.lock().unwrap();
                if state.busy {
                    return 1;
                }
                state.busy = true;
                state.prepared = None;
                state.cancel = Cancellation::default();
                state.status = "Checking for updates...".into();
                drop(state);
                EnableWindow(GetDlgItem(hwnd, CHECK), 0);
                EnableWindow(GetDlgItem(hwnd, INSTALL), 0);
                thread::spawn(move || check(work));
            } else if !ctx.monitor && id == INSTALL {
                let work = ctx.work.clone();
                let mut state = work.lock().unwrap();
                if state.busy {
                    return 1;
                }
                if let Some(prepared) = state.prepared.take() {
                    state.busy = true;
                    state.arming = true;
                    state.status="Preparing to install. The app will close after the update dialog is ready.".into();
                    drop(state);
                    let installed = ctx.installed.clone();
                    thread::spawn(move || install(work, installed, prepared));
                }
            } else if ctx.monitor && id == OPEN && ctx.succeeded {
                match Command::new(ctx.installed.join(EXE)).spawn() {
                    Ok(_) => {
                        EndDialog(hwnd, 0);
                    }
                    Err(error) => show_status(
                        hwnd,
                        &mut ctx,
                        format!("The update succeeded, but the app could not open: {error}"),
                    ),
                }
            } else if id == IDCANCEL {
                return close(hwnd, &mut ctx);
            }
            1
        }
        WM_CLOSE => close(hwnd, &mut ctx),
        WM_DESTROY => {
            KillTimer(hwnd, TIMER);
            1
        }
        _ => 0,
    }
}
unsafe fn close(hwnd: HWND, ctx: &mut Context) -> isize {
    if ctx.monitor {
        if ctx.terminal {
            EndDialog(hwnd, 0);
        }
    } else {
        let mut state = ctx.work.lock().unwrap();
        if state.arming {
            return 1;
        }
        if state.busy {
            state.cancel.cancel();
            state.status = "Cancelling the update check...".into();
            ctx.closing = true;
        } else {
            EndDialog(hwnd, 0);
        }
    }
    1
}

pub fn run() {
    use std::os::windows::io::FromRawHandle;
    if let Err(error) = freshen::run_helper(std::env::args_os().skip(1)) {
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                wide(&error.to_string()).as_ptr(),
                wide("Freshen Native Test").as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
        return;
    } else if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == "--freshen-apply")
    {
        return;
    }
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let monitor = args.first().is_some_and(|a| a == "--monitor");
    if monitor && args.len() != 3 {
        return;
    }
    let installed = if monitor {
        PathBuf::from(&args[1])
    } else {
        std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
    };
    let helper = if monitor {
        let pid = PathBuf::from(&args[2])
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("monitor-ready-"))
            .and_then(|s| s.parse::<u32>().ok());
        pid.and_then(|pid| {
            let handle = unsafe { OpenProcess(0x0010_0000, 0, pid) }; // SYNCHRONIZE only.
            (!handle.is_null())
                .then(|| unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle) })
        })
    } else {
        None
    };
    let ctx = RefCell::new(Context {
        work: Arc::new(Mutex::new(Work::default())),
        monitor,
        ready: monitor.then(|| PathBuf::from(&args[2])),
        installed,
        closing: false,
        terminal: false,
        succeeded: false,
        text: String::new(),
        progress: None,
        last_progress: Instant::now() - Duration::from_secs(1),
        helper,
    });
    unsafe {
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_PROGRESS_CLASS,
        };
        InitCommonControlsEx(&controls);
        let result = DialogBoxParamW(
            GetModuleHandleW(std::ptr::null()),
            if monitor { 102usize } else { 101usize } as *const u16,
            std::ptr::null_mut(),
            Some(dialog),
            &ctx as *const RefCell<Context> as isize,
        );
        if result == -1 {
            MessageBoxW(
                std::ptr::null_mut(),
                wide(&format!(
                    "Native dialog creation failed: {}",
                    std::io::Error::last_os_error()
                ))
                .as_ptr(),
                wide("Freshen Native Test").as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}
