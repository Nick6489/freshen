//! Direct, bounded Win32 integration assertions against only this test app's HWNDs.
//! No desktop-wide input injection or interaction with other applications.
#![cfg(windows)]
#![allow(unsafe_op_in_unsafe_fn)]
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    System::Threading::*,
    UI::{Controls::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
unsafe fn value(hwnd: HWND) -> String {
    let mut buffer = [0; 4096];
    // GetWindowText intentionally omits another process's edit-control contents.
    // WM_GETTEXT is a system message and its buffer is marshalled by Windows.
    let mut len = 0;
    assert_ne!(
        SendMessageTimeoutW(
            hwnd,
            WM_GETTEXT,
            buffer.len(),
            buffer.as_mut_ptr() as isize,
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            1500,
            &mut len
        ),
        0,
        "control text query timed out"
    );
    String::from_utf16_lossy(&buffer[..len])
}
unsafe fn class(hwnd: HWND) -> String {
    let mut buffer = [0; 128];
    let len = GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
    String::from_utf16_lossy(&buffer[..len as usize])
}
unsafe fn send(hwnd: HWND, message: u32, wparam: usize, lparam: isize) {
    let mut result = 0;
    assert_ne!(
        SendMessageTimeoutW(
            hwnd,
            message,
            wparam,
            lparam,
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            1500,
            &mut result
        ),
        0,
        "UI did not respond to message {message}"
    );
}
unsafe fn command(dialog: HWND, id: i32) {
    // Queue the normal BN_CLICKED notification on the dialog thread. BM_CLICK
    // can fail on inactive dialogs and SendMessage can interrupt a timer callback.
    // This does not activate the window or inject desktop-wide input.
    assert_ne!(
        PostMessageW(
            dialog,
            WM_COMMAND,
            id as usize,
            GetDlgItem(dialog, id) as isize
        ),
        0
    );
}
fn image(pid: u32) -> Option<PathBuf> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buffer = vec![0u16; 32768];
        let mut len = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        (ok != 0).then(|| PathBuf::from(String::from_utf16_lossy(&buffer[..len as usize])))
    }
}
struct Search<'a> {
    title: &'a str,
    pid: Option<u32>,
    found: Vec<(HWND, u32)>,
}
unsafe extern "system" fn enumerate(hwnd: HWND, param: isize) -> i32 {
    let search = &mut *(param as *mut Search<'_>);
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    // Only query cached top-level captions while enumerating. Never send a
    // synchronous message to arbitrary desktop windows or panic across this ABI.
    let mut caption = [0u16; 256];
    let len = GetWindowTextW(hwnd, caption.as_mut_ptr(), caption.len() as i32);
    if search.pid.is_none_or(|expected| expected == pid)
        && String::from_utf16_lossy(&caption[..len as usize]) == search.title
    {
        search.found.push((hwnd, pid));
    }
    1
}
fn windows(title: &str, pid: Option<u32>) -> Vec<(HWND, u32)> {
    let mut search = Search {
        title,
        pid,
        found: Vec::new(),
    };
    unsafe {
        EnumWindows(Some(enumerate), &mut search as *mut Search<'_> as isize);
    }
    search.found
}
fn wait_window(title: &str, pid: Option<u32>, timeout: Duration) -> (HWND, u32) {
    let end = Instant::now() + timeout;
    loop {
        let found = windows(title, pid);
        assert!(
            found.len() < 2,
            "close duplicate test-app windows before running the test"
        );
        if let Some(window) = found.first() {
            return *window;
        }
        assert!(Instant::now() < end, "timed out waiting for {title}");
        thread::sleep(Duration::from_millis(50));
    }
}
struct Launched(Child);
impl Drop for Launched {
    fn drop(&mut self) {
        for (hwnd, _) in windows("Freshen Native Test", Some(self.0.id())) {
            unsafe {
                let mut result = 0;
                let _ = SendMessageTimeoutW(
                    hwnd,
                    WM_CLOSE,
                    0,
                    0,
                    SMTO_ABORTIFHUNG | SMTO_BLOCK,
                    1500,
                    &mut result,
                );
            }
        }
        // Never kill a helper or installer, even when an assertion fails.
        let _ = self.0.try_wait();
    }
}

#[test]
fn resource_dialog_controls_keyboard_and_worker_error_remain_responsive() {
    let profile = tempfile::tempdir().unwrap();
    let mut app = Launched(
        Command::new(env!("CARGO_BIN_EXE_freshen-native-test"))
            .env("LOCALAPPDATA", profile.path())
            .spawn()
            .unwrap(),
    );
    let (hwnd, _) = wait_window(
        "Freshen Native Test",
        Some(app.0.id()),
        Duration::from_secs(10),
    );
    unsafe {
        assert_eq!(class(hwnd), "#32770");
        let check = GetDlgItem(hwnd, 1004);
        let install = GetDlgItem(hwnd, 1005);
        let exit = GetDlgItem(hwnd, IDCANCEL);
        assert!(value(GetDlgItem(hwnd, 1001)).starts_with("Installed version:"));
        assert_eq!(class(GetDlgItem(hwnd, 1002)), "Edit");
        assert_eq!(class(GetDlgItem(hwnd, 1003)), "Edit");
        assert_ne!(
            GetWindowLongPtrW(GetDlgItem(hwnd, 1003), GWL_STYLE) & ES_READONLY as isize,
            0
        );
        assert_eq!(IsWindowEnabled(install), 0);
        assert_eq!(
            GetNextDlgTabItem(hwnd, check, 0),
            exit,
            "disabled Install must be skipped by dialog navigation"
        );
        assert_ne!(
            GetWindowLongPtrW(check, GWL_STYLE) & BS_DEFPUSHBUTTON as isize,
            0
        );
        send(
            GetDlgItem(hwnd, 1002),
            WM_SETTEXT,
            0,
            wide("Text remains editable while checking").as_ptr() as isize,
        );
        assert_eq!(
            value(GetDlgItem(hwnd, 1002)),
            "Text remains editable while checking"
        );
        // Posted keyboard messages pass through DialogBox's real dialog manager.
        assert_ne!(PostMessageW(check, WM_KEYDOWN, VK_RETURN as usize, 0), 0);
        let end = Instant::now() + Duration::from_secs(15);
        loop {
            send(hwnd, WM_NULL, 0, 0);
            if value(GetDlgItem(hwnd, 1003)).starts_with("Could not check for updates:") {
                break;
            }
            assert!(
                Instant::now() < end,
                "Enter did not invoke the default check or error did not arrive"
            );
            thread::sleep(Duration::from_millis(50));
        }
        assert_ne!(IsWindowEnabled(check), 0);
        assert_eq!(
            IsWindowEnabled(install),
            0,
            "failed verification/check must not enable installation"
        );
        assert_ne!(PostMessageW(check, WM_KEYDOWN, VK_ESCAPE as usize, 0), 0);
    }
    let end = Instant::now() + Duration::from_secs(5);
    while app.0.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < end,
            "Escape did not close the native dialog"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "install the generated v1 Inno 7 package, enable the signed v2 feed, and run with --ignored --nocapture"]
fn installed_v1_to_v2_native_ui_update() {
    let profile = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap());
    let installed = profile.join("Programs/FreshenNativeTest");
    let executable = installed.join("freshen-native-test.exe");
    let state = profile.join("FreshenNativeDialogTestServicing");
    assert!(
        executable.is_file(),
        "install FreshenNativeTest-1.0.0-x64.exe first"
    );
    assert!(
        !state.join(".freshen/transaction").exists(),
        "archive/discard the previous transaction before repeating this one-shot test"
    );
    assert!(
        profile
            .join("FreshenNativeDialogTestFeed/manifest.json")
            .exists(),
        "run scripts/enable-update.ps1 first"
    );
    assert!(
        windows("Freshen Native Test - Update", None).is_empty(),
        "close a previous completed update dialog first"
    );
    let existing = windows("Freshen Native Test", None);
    assert!(existing.len() < 2, "close duplicate dummy-app windows");
    let mut launched = if existing.is_empty() {
        Some(Launched(Command::new(&executable).spawn().unwrap()))
    } else {
        None
    };
    let (main, pid) = wait_window("Freshen Native Test", None, Duration::from_secs(10));
    assert_eq!(
        fs::canonicalize(image(pid).unwrap()).unwrap(),
        fs::canonicalize(&executable).unwrap()
    );
    unsafe {
        assert_eq!(class(main), "#32770");
        assert_eq!(value(GetDlgItem(main, 1001)), "Installed version: 1.0.0");
        command(main, 1004);
        let end = Instant::now() + Duration::from_secs(30);
        loop {
            send(main, WM_NULL, 0, 0);
            if IsWindowEnabled(GetDlgItem(main, 1005)) != 0 {
                break;
            }
            let status = value(GetDlgItem(main, 1003));
            assert!(!status.starts_with("Could not"), "{status}");
            assert!(
                Instant::now() < end,
                "signed update did not become ready: {status}"
            );
            thread::sleep(Duration::from_millis(50));
        }
        assert!(value(GetDlgItem(main, 1003)).contains("Update 2.0.0 is ready"));
        command(main, 1005);
    }
    let (monitor, monitor_pid) = wait_window(
        "Freshen Native Test - Update",
        None,
        Duration::from_secs(30),
    );
    if let Some(mut app) = launched.take() {
        // Release the old process handle before Inno replaces its executable.
        let deadline = Instant::now() + Duration::from_secs(15);
        while app.0.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "original test process did not exit after monitor readiness"
            );
            thread::sleep(Duration::from_millis(20));
        }
        drop(app);
    }
    assert_ne!(
        pid, monitor_pid,
        "the progress UI must outlive the installed process"
    );
    assert!(image(monitor_pid).unwrap().starts_with(&state));
    let end = Instant::now() + Duration::from_secs(90);
    let mut saw_disabled = false;
    let mut saw_finalizing = false;
    unsafe {
        assert_eq!(class(monitor), "#32770");
        assert_eq!(class(GetDlgItem(monitor, 1006)), "msctls_progress32");
        loop {
            send(monitor, WM_NULL, 0, 0);
            let status = value(GetDlgItem(monitor, 1003));
            if status.starts_with("Installing") || status.starts_with("Finalizing") {
                assert_eq!(IsWindowEnabled(GetDlgItem(monitor, IDCANCEL)), 0);
                saw_disabled = true;
                send(monitor, WM_CLOSE, 0, 0);
                assert_ne!(
                    IsWindow(monitor),
                    0,
                    "unsafe cancellation must not close the monitor"
                );
            }
            if status.starts_with("Finalizing") {
                saw_finalizing = true;
                assert_ne!(
                    GetWindowLongPtrW(GetDlgItem(monitor, 1006), GWL_STYLE) & PBS_MARQUEE as isize,
                    0,
                    "finalization must be indeterminate"
                );
            }
            if status.starts_with("Update installed successfully") {
                break;
            }
            assert!(!status.starts_with("Update failed"), "{status}");
            assert!(Instant::now() < end, "update did not complete: {status}");
            thread::sleep(Duration::from_millis(100));
        }
        assert!(
            saw_disabled && saw_finalizing,
            "required installation/finalization UI states were not observed"
        );
        assert_eq!(
            IsWindow(main),
            0,
            "the original application dialog must close before installation"
        );
        let result = freshen::inno::status(&state).unwrap().unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert!(result.telemetry_observed && result.progress_observed);
        assert_eq!(
            freshen::installation_status(&state).unwrap().unwrap().state,
            freshen::InstallState::Committed
        );
        assert!(Path::new(&installed.join("unins000.exe")).exists());
        assert!(!installed.join("unins001.exe").exists());
        assert_ne!(IsWindowEnabled(GetDlgItem(monitor, 1007)), 0);
        command(monitor, 1007);
    }
    let (updated, _) = wait_window("Freshen Native Test", None, Duration::from_secs(10));
    unsafe {
        assert_eq!(value(GetDlgItem(updated, 1001)), "Installed version: 2.0.0");
        send(updated, WM_CLOSE, 0, 0);
    }
    println!(
        "PASS: native dialogs; signed v2; responsive UI; disabled cancellation; finalization; real telemetry; exit 0; unchanged uninstall identity; reopened version 2.0.0"
    );
}
