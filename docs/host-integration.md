# Application-as-helper and host-owned progress UI

The preferred integration uses the application's executable as its helper when
that copied executable can start independently. A separate `freshen-helper`
executable remains supported, including for portable applications. Both execute
the same library helper protocol; the choice is packaging, not update behavior.
The core and bridge DLL remain independent of every GUI toolkit.

## Startup and handoff

Before GUI initialization, audio devices, asset loading, application services or
single-instance locks:

```rust,no_run
fn main() -> freshen::Result<()> {
    if freshen::run_helper(std::env::args_os().skip(1))? {
        return Ok(());
    }
    // Normal application initialization follows here.
    Ok(())
}
```

Establish an explicit local `InstallPolicy`, select with
`check_for_kind(..., policy.package_kind(), ...)`, then check and prepare on a
worker thread. After the user agrees and work is saved, use
`PreparedUpdate::arm_current_exe(policy, relaunch_args)`. It resolves
`std::env::current_exe` and delegates to the existing `arm` path. Return to the
UI only after readiness, then exit normally through the host. If shutdown is
declined, cancel the returned Handoff. Dropping it does not cancel.

To use a separate helper, keep calling `arm(policy, helper_path, relaunch_args)`.
It can live beside the app, within its private support directory, or another
host-controlled deployment location. The library copies the chosen executable
into its private transaction directory for execution. It waits for the original
application to exit before touching program files. The user need not see a
separate helper EXE in the normal application directory when using self-helper.
There is still a private copy on disk; zero temporary executable files is not
promised. Recovery evidence remains until explicitly discarded.

The Windows loader resolves imported DLLs **before Rust `main` runs**. Early
`run_helper` dispatch therefore does not make an SDL/FFmpeg/runtime-dependent
executable independent of adjacent DLLs. Managed apphosts can also need their
runtime configuration/dependencies before application code starts. Use a small
standalone helper when necessary, or a host bootstrap/dependency deployment
design that securely makes copied startup self-contained. Do not relax DLL
search paths or verification to force readiness. The readiness timeout exposes
missing runtime/bootstrap dependencies before the parent exits.

For portable updates, `.freshen` normally belongs under the explicitly owned
portable root and portable recovery/startup confirmation applies. For Inno,
state/helper/installer material must be in a separate user-private servicing
root outside the installed application tree. Inno owns installation and repair;
portable rollback and startup-confirmation rollback do not apply to Inno.

## A progress dialog that survives shutdown

The disposable demonstration had three roles:

1. The normal app hosted Check, download/verification and the Install decision.
2. A separate copy of the host supplied a surviving presentation-only process.
3. The existing Freshen helper waited for the normal app to exit and supervised
   the actual Inno process. The presentation process did not install files.

To reproduce it in the external reference app, put the presentation process
outside Inno's replacement set, start it with private local context, and wait
for its UI readiness before closing the original host. If presentation startup
fails while the parent is still alive, cancel the existing Handoff. The monitor
polls `inno::status(state_root)` on its event loop timer, not a callback in the
terminated original process. Do not wait for the monitor to exit before starting
Inno; it exists to remain alive while installation runs. A copied presentation
EXE has the same independent-startup requirement as a copied self-helper.

The application decides the visual design and scheduling. Freshen supplies
structured phases/counters, telemetry evidence and the installer's actual exit.
Map Preparing, Installing, Finalizing and terminal states into the same host UI.
Installing is indeterminate without real counters. Finalizing clears percent
progress and remains indeterminate. Enable completion actions only after an
actual terminal result. Track helper lifetime as well: if it disappears without
a terminal result, show an unknown outcome and retain evidence for inspection.
Never turn a stale progress snapshot into success or silently rerun an installer.

## Native Windows dialog pattern

For the reference application's classic Windows UI, use resource-defined
`DIALOGEX` and `DialogBoxParamW`/`CreateDialogParamW`. These small resource
templates preserve the demonstrated structure without retaining a demo app:

```rc
#include <windows.h>
#include <commctrl.h>

101 DIALOGEX 0, 0, 330, 180
STYLE DS_SETFONT | DS_MODALFRAME | WS_POPUP | WS_CAPTION | WS_SYSMENU
CAPTION "Application updates"
FONT 9, "Segoe UI"
BEGIN
 LTEXT "Installed version", 1001, 12, 12, 306, 12
 LTEXT "&Status:", -1, 12, 36, 306, 12
 EDITTEXT 1003, 12, 50, 306, 70, ES_MULTILINE | ES_READONLY | WS_VSCROLL | WS_TABSTOP
 DEFPUSHBUTTON "&Check for updates", 1004, 12, 145, 100, 22, WS_TABSTOP
 PUSHBUTTON "&Install update", 1005, 119, 145, 95, 22, WS_TABSTOP | WS_DISABLED
 PUSHBUTTON "E&xit", IDCANCEL, 222, 145, 96, 22, WS_TABSTOP
END

102 DIALOGEX 0, 0, 330, 177
STYLE DS_SETFONT | DS_MODALFRAME | WS_POPUP | WS_CAPTION | WS_SYSMENU
CAPTION "Updating application"
FONT 9, "Segoe UI"
BEGIN
 LTEXT "&Status:", -1, 12, 12, 306, 12
 EDITTEXT 1003, 12, 26, 306, 64, ES_MULTILINE | ES_READONLY | WS_VSCROLL | WS_TABSTOP
 LTEXT "Installation &work:", -1, 12, 99, 306, 12
 CONTROL "Installation work", 1006, PROGRESS_CLASS, WS_CHILD | WS_VISIBLE | PBS_MARQUEE, 12, 114, 306, 13
 DEFPUSHBUTTON "&Open updated app", 1007, 100, 143, 116, 22, WS_TABSTOP | WS_DISABLED
 PUSHBUTTON "&Close", IDCANCEL, 222, 143, 96, 22, WS_TABSTOP | WS_DISABLED
END
```

Compile with the Windows SDK resource compiler and link the `.res` into the
host. Enable Common Controls v6 in its application manifest, initialize
`ICC_PROGRESS_CLASS`, then use the dialog manager. Store GUI state in
pointer-size-correct dialog user storage (`DWLP_USER`); dispatch worker results
back to the UI thread. Return FALSE from `WM_INITDIALOG` only when assigning
initial focus yourself. Do not synthesize a dialog using a custom top-level
window or manually disabled owner.

For determinate work counters stop the marquee and remove `PBS_MARQUEE` before
setting position. Restore marquee style/animation for Finalizing and missing
telemetry. Show 100% completion only after success; an Inno work counter reaching
its total is not proof that post-install operations have finished. Update status
text on phase changes, not every download callback, and throttle progress
announcements. Keep focus stable. Use native control names/states, label edits
with adjacent static labels and preserve resource tab order/default buttons.
Escape/Close must respect the cancellation capability: while Inno is modifying
files, closing the presentation process is not a safe cancellation mechanism.

## Manual rehearsal and validation

The user owns the demonstration: install the old version, make a signed newer
release available, and let the user choose Check and Install themselves. Run
automated installed-app control tests only as an explicit separate test; they
must not take over a hands-on rehearsal. The former demo's automated run passed
native dialog/control checks, Enter/Escape, responsiveness, disabled close,
indeterminate Finalizing, real telemetry, exit 0, retained uninstall identity and
opening version 2. It was then reset for the user's own successful rehearsal.

The demo sources and UI automation fixture were removed at the user's request;
this guide preserves the pattern. A future maintained reference app must test
its own resource layouts, navigation, all instances exiting, error handling,
UAC where applicable, monitor lifetime and NVDA speech. A native `#32770` role
and correct accessibility tree are useful evidence, not a substitute for NVDA
testing. The library's retained process/installer tests cover backend behavior.
