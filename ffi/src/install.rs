//! Arming an installation, the helper process, and recovery.

use crate::{
    error::{fail, guard, invalid, run},
    updater::FreshenPrepared,
    util::{cstr, cstr_list, handle_mut, store, to_c_string},
};
use freshen::{
    Handoff, InstallLayout, InstallPolicy, InstallState, InstallStatus,
    discard_finished_installation, installation_status, recover_installation,
};
use std::{
    ffi::{OsString, c_char},
    mem::size_of,
    path::{Path, PathBuf},
    ptr,
};

/// `FreshenInstallPolicy.layout`: a directory of application-owned files.
pub const FRESHEN_LAYOUT_PORTABLE: i32 = 1;
/// `FreshenInstallPolicy.layout`: a macOS application bundle.
pub const FRESHEN_LAYOUT_MAC_BUNDLE: i32 = 2;

/// No transaction exists in the installation.
pub const FRESHEN_STATE_NONE: i32 = 0;
pub const FRESHEN_STATE_PREPARED: i32 = 1;
pub const FRESHEN_STATE_APPLYING: i32 = 2;
pub const FRESHEN_STATE_AWAITING_CONFIRMATION: i32 = 3;
pub const FRESHEN_STATE_COMMITTED: i32 = 4;
pub const FRESHEN_STATE_ROLLING_BACK: i32 = 5;
pub const FRESHEN_STATE_ROLLED_BACK: i32 = 6;
pub const FRESHEN_STATE_FAILED: i32 = 7;
/// A state this ABI version does not know; read the detail text.
pub const FRESHEN_STATE_UNKNOWN: i32 = 100;

/// Local installation policy, decided by the application and never inferred
/// from a download. Set `struct_size` to `sizeof(FreshenInstallPolicy)`.
///
/// Portable layout uses `root`, `executable`, the owned-file list and the
/// optional expandable directories. Mac bundle layout uses `root` (the existing
/// parent directory), `bundle_name`, `executable`, `bundle_id` and `team_id`.
/// Unused pointer fields may be null.
#[repr(C)]
pub struct FreshenInstallPolicy {
    pub struct_size: usize,
    pub layout: i32,
    pub root: *const c_char,
    pub executable: *const c_char,
    pub owned_files: *const *const c_char,
    pub owned_file_count: usize,
    pub expandable_directories: *const *const c_char,
    pub expandable_directory_count: usize,
    pub bundle_name: *const c_char,
    pub bundle_id: *const c_char,
    pub team_id: *const c_char,
    /// Seconds to wait for this process to exit; 0 keeps the default (120).
    pub exit_timeout_secs: u64,
    /// Seconds to wait for the new version to confirm startup; 0 keeps the default (120).
    pub confirmation_timeout_secs: u64,
}

/// An armed update: a helper holds the installation lock and waits for this
/// process to exit. Freeing the handle does NOT cancel the installation.
pub struct FreshenHandoff(Option<Handoff>);

unsafe fn build_policy(policy: *const FreshenInstallPolicy) -> Result<InstallPolicy, i32> {
    let policy = unsafe { policy.as_ref() }.ok_or_else(|| invalid("policy must not be null"))?;
    if policy.struct_size < size_of::<FreshenInstallPolicy>() {
        return Err(invalid("policy.struct_size is too small"));
    }
    let root = PathBuf::from(unsafe { cstr(policy.root, "policy.root") }?);
    let executable = unsafe { cstr(policy.executable, "policy.executable") }?.to_owned();
    let mut built = match policy.layout {
        FRESHEN_LAYOUT_PORTABLE => InstallPolicy::portable_expanding(
            root,
            executable,
            unsafe {
                cstr_list(
                    policy.owned_files,
                    policy.owned_file_count,
                    "policy.owned_files",
                )
            }?,
            unsafe {
                cstr_list(
                    policy.expandable_directories,
                    policy.expandable_directory_count,
                    "policy.expandable_directories",
                )
            }?,
        ),
        FRESHEN_LAYOUT_MAC_BUNDLE => {
            let mut built = InstallPolicy::portable(root, String::new(), Vec::new());
            built.layout = InstallLayout::MacBundle {
                bundle_name: unsafe { cstr(policy.bundle_name, "policy.bundle_name") }?.to_owned(),
                executable,
                bundle_id: unsafe { cstr(policy.bundle_id, "policy.bundle_id") }?.to_owned(),
                team_id: unsafe { cstr(policy.team_id, "policy.team_id") }?.to_owned(),
            };
            built
        }
        other => return Err(invalid(&format!("unknown policy layout {other}"))),
    };
    if policy.exit_timeout_secs != 0 {
        built.exit_timeout_secs = policy.exit_timeout_secs;
    }
    if policy.confirmation_timeout_secs != 0 {
        built.confirmation_timeout_secs = policy.confirmation_timeout_secs;
    }
    Ok(built)
}

/// Copy the verified staging into the installation and start a helper that
/// waits for this process to exit. Returns after the helper holds the
/// installation lock. The caller then saves work and quits, or calls
/// `freshen_handoff_cancel` if shutdown is declined.
///
/// `prepared` is CONSUMED whether this succeeds or fails: its temporary files
/// are deleted and the handle can only be freed. Run this on a worker thread.
///
/// # Safety
/// Pointers must be valid as documented on their types.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_arm(
    prepared: *mut FreshenPrepared,
    policy: *const FreshenInstallPolicy,
    helper_executable: *const c_char,
    relaunch_args: *const *const c_char,
    relaunch_arg_count: usize,
    out_handoff: *mut *mut FreshenHandoff,
) -> i32 {
    run(|| {
        unsafe { store(out_handoff, ptr::null_mut(), "out_handoff") }?;
        let prepared = unsafe { handle_mut(prepared, "prepared") }?;
        let update = prepared
            .0
            .take()
            .ok_or_else(|| invalid("prepared update was already consumed"))?;
        let policy = unsafe { build_policy(policy) }?;
        let helper = unsafe { cstr(helper_executable, "helper_executable") }?;
        let args = unsafe { cstr_list(relaunch_args, relaunch_arg_count, "relaunch_args") }?;
        let handoff = update
            .arm(policy, Path::new(helper), args)
            .map_err(|error| fail(&error))?;
        unsafe {
            store(
                out_handoff,
                Box::into_raw(Box::new(FreshenHandoff(Some(handoff)))),
                "out_handoff",
            )
        }
    })
}

/// Process id of the waiting helper, or 0 if the handle is null or cancelled.
///
/// # Safety
/// `handoff` must be a live handoff handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_handoff_helper_pid(handoff: *const FreshenHandoff) -> u32 {
    guard(0, || {
        unsafe { handoff.as_ref() }
            .and_then(|h| h.0.as_ref())
            .map_or(0, Handoff::helper_pid)
    })
}

/// Cancel an armed update because shutdown was declined. Consumes the handoff.
///
/// # Safety
/// `handoff` must be a live handoff handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_handoff_cancel(handoff: *mut FreshenHandoff) -> i32 {
    run(|| {
        let handoff = unsafe { handle_mut(handoff, "handoff") }?;
        let armed = handoff
            .0
            .take()
            .ok_or_else(|| invalid("handoff was already cancelled"))?;
        armed.cancel().map_err(|error| fail(&error))
    })
}

/// Free a handoff handle. This does NOT cancel a requested installation.
///
/// # Safety
/// `handoff` must be null or a handle not already freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_handoff_free(handoff: *mut FreshenHandoff) {
    guard((), || {
        if !handoff.is_null() {
            drop(unsafe { Box::from_raw(handoff) });
        }
    });
}

fn state_code(state: &InstallState) -> i32 {
    match state {
        InstallState::Prepared => FRESHEN_STATE_PREPARED,
        InstallState::Applying => FRESHEN_STATE_APPLYING,
        InstallState::AwaitingConfirmation => FRESHEN_STATE_AWAITING_CONFIRMATION,
        InstallState::Committed => FRESHEN_STATE_COMMITTED,
        InstallState::RollingBack => FRESHEN_STATE_ROLLING_BACK,
        InstallState::RolledBack => FRESHEN_STATE_ROLLED_BACK,
        InstallState::Failed => FRESHEN_STATE_FAILED,
    }
}

unsafe fn report_status(
    status: Option<InstallStatus>,
    out_state: *mut i32,
    out_version: *mut *mut c_char,
    out_detail: *mut *mut c_char,
) -> Result<(), i32> {
    let (state, version, detail) = match &status {
        Some(status) => (
            state_code(&status.state),
            to_c_string(&status.version),
            to_c_string(&status.detail),
        ),
        None => (FRESHEN_STATE_NONE, ptr::null_mut(), ptr::null_mut()),
    };
    unsafe { store(out_state, state, "out_state") }?;
    if !out_version.is_null() {
        unsafe { out_version.write(version) };
    } else {
        unsafe { crate::freshen_string_free(version) };
    }
    if !out_detail.is_null() {
        unsafe { out_detail.write(detail) };
    } else {
        unsafe { crate::freshen_string_free(detail) };
    }
    Ok(())
}

/// Read-only status of the last installation transaction under `root`. Sets
/// `*out_state` to `FRESHEN_STATE_NONE` when there is none. `out_version` and
/// `out_detail` may be null; otherwise they receive strings (or null) to free
/// with `freshen_string_free`.
///
/// # Safety
/// `root` must be a NUL-terminated UTF-8 path; out-pointers must be writable
/// (or null where documented).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_installation_status(
    root: *const c_char,
    out_state: *mut i32,
    out_version: *mut *mut c_char,
    out_detail: *mut *mut c_char,
) -> i32 {
    run(|| {
        let root = unsafe { cstr(root, "root") }?;
        let status = installation_status(Path::new(root)).map_err(|error| fail(&error))?;
        unsafe { report_status(status, out_state, out_version, out_detail) }
    })
}

/// Resume a pending rollback once every application process is stopped. Run it
/// from a maintenance process when the executable may be locked. Outputs as
/// `freshen_installation_status`.
///
/// # Safety
/// As `freshen_installation_status`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_recover_installation(
    root: *const c_char,
    out_state: *mut i32,
    out_version: *mut *mut c_char,
    out_detail: *mut *mut c_char,
) -> i32 {
    run(|| {
        let root = unsafe { cstr(root, "root") }?;
        let status = recover_installation(Path::new(root)).map_err(|error| fail(&error))?;
        unsafe { report_status(Some(status), out_state, out_version, out_detail) }
    })
}

/// Remove a finished (committed or rolled back) or cancelled-prepared
/// transaction and its backups. `FRESHEN_ERR_BUSY` means a helper is still
/// active; retry later.
///
/// # Safety
/// `root` must be a NUL-terminated UTF-8 path.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_discard_finished_installation(root: *const c_char) -> i32 {
    run(|| {
        let root = unsafe { cstr(root, "root") }?;
        discard_finished_installation(Path::new(root)).map_err(|error| fail(&error))
    })
}

/// Self-helper dispatch. Call this first in `main`, before any UI or
/// single-instance lock, passing the process arguments EXCLUDING the program
/// name, as UTF-8. If `*out_handled` becomes true this process was launched as
/// the update helper, has finished, and must exit without starting normally.
///
/// # Safety
/// `argv` must hold `argc` NUL-terminated UTF-8 strings (it may be null when
/// `argc` is 0); `out_handled` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_run_helper(
    argc: usize,
    argv: *const *const c_char,
    out_handled: *mut bool,
) -> i32 {
    run(|| {
        unsafe { store(out_handled, false, "out_handled") }?;
        let arguments = unsafe { cstr_list(argv, argc, "argv") }?;
        let handled = freshen::run_helper(arguments.into_iter().map(OsString::from))
            .map_err(|error| fail(&error))?;
        unsafe { store(out_handled, handled, "out_handled") }
    })
}

/// Call from the restarted application once its essential initialization has
/// succeeded. `*out_confirmed` is true only when this launch was an update
/// awaiting confirmation; it is a harmless no-op for ordinary launches.
///
/// # Safety
/// `root` must be a NUL-terminated UTF-8 path; `out_confirmed` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn freshen_confirm_startup(
    root: *const c_char,
    out_confirmed: *mut bool,
) -> i32 {
    run(|| {
        unsafe { store(out_confirmed, false, "out_confirmed") }?;
        let root = unsafe { cstr(root, "root") }?;
        let confirmed = freshen::confirm_startup(Path::new(root)).map_err(|error| fail(&error))?;
        unsafe { store(out_confirmed, confirmed, "out_confirmed") }
    })
}
