# Inno Setup servicing

Freshen owns discovery, publisher authentication, download, staging, application
shutdown coordination and status presentation. Inno owns install identity,
uninstall registration, file replacement, privilege checks and repair. The core
library and DLL contain no GUI toolkit or message loop. Run blocking Freshen
operations on a worker thread.

## Integration and public interfaces

The signed manifest retains schema 1. `Artifact.kind = "inno"` means a ZIP with
exactly one declared `.exe`, for example `setup.exe`. The existing release tool
supports `pack ... inno ...`. Both the ZIP hash and installer inventory hash are
verified using the existing pinned Ed25519 trust store, download limits and
bounded extraction. An authenticated ZIP is retained through helper handoff;
the helper checks the stage again before execution. Installer arguments are
local policy, never downloaded command lines.

Use `InstallLayout::Inno(InnoPolicy { ... })`. Prefer `arm_current_exe` when
the copied host can start independently; `arm` remains available for an
explicitly deployed standalone helper. Establish the policy before discovery
and pass its `package_kind()` to `Updater::check_for_kind`. The
[distribution guide](distributions.md) covers portable/Inno artifacts sharing a
target, and the [host guide](host-integration.md) covers helper startup and a
surviving native progress presentation process.

```rust,no_run
use freshen::{InstallLayout, InstallPolicy, PreparedUpdate};
use freshen::inno::InnoPolicy;
use std::path::Path;
fn arm(update: PreparedUpdate, state: &Path, installed: &Path)
    -> freshen::Result<freshen::Handoff>
{
    update.arm_current_exe(InstallPolicy {
        root: state.into(),
        layout: InstallLayout::Inno(InnoPolicy {
            installer: "setup.exe".into(),
            app_id: "MyExistingAppId".into(),
            directory: installed.into(),
            executable: "MyApp.exe".into(),
            machine_wide: false,
            enhanced: true,
            timeout_secs: 1800,
            relaunch: false,
        }),
        exit_timeout_secs: 120,
        confirmation_timeout_secs: 120,
    }, vec![])
}
```

`root` is an existing, user-writable servicing directory **outside the
application directory**, for example `%LOCALAPPDATA%\Vendor\MyAppServicing`.
It must not contain the application directory either. This keeps the helper,
installer, log and journal outside Inno's replacement set and supports servicing
a machine installation without elevating the Freshen UI or helper. Keep this
directory private to the user's account. Do not place it in a shared writable
directory. One state root must be used consistently for an application; do not
create different roots to bypass an unresolved transaction.

`app_id` and `directory` are explicit local ownership policy. Preflight requires
an existing `<AppId>_is1` uninstall key in HKCU for `machine_wide=false`, or HKLM
for `true`, examining both registry views. `InstallLocation` must canonicalize
to the configured directory. Missing registration, a scope mismatch or a
different directory fails before helper readiness. Thus this backend services
an existing Inno installation; ordinary initial installation still uses Inno.
Existing nonstandard installers without `InstallLocation` are rejected.

The update script must keep its original `AppId`, install scope, install mode and directory
semantics. Freshen cannot inspect an executable to prove its embedded AppId:
the pinned publisher remains responsible for shipping the correct script.
There is no AppId override switch. Do not convert a portable installation by
calling this API. Do not add uninstall/reinstall steps to the update script.

`installer` must match the single signed inventory entry. `executable` is a
validated relative path used only if `relaunch=true`. `timeout_secs` is 1..86400
and defines an overdue warning, **not a kill deadline**. `enhanced` offers
telemetry; it does not require it. `confirmation_timeout_secs` remains part of
the shared policy but is unused for Inno: startup-confirmation rollback belongs
only to the existing file/bundle backend. Inno success does not prove application
startup health. `relaunch_args` are passed directly to an optional normal-user
relaunch and never to the installer.

The existing C, C++, .NET and Objective-C installation policy ABIs do not yet
expose Inno arming. This addition is available to Rust hosts. Old C status
consumers map `RebootRequired` to their existing UNKNOWN state and must read the
detail text. The separate bridge ABI below is specifically for Pascal Script.

## Helper lifecycle and outcomes

The existing copied `freshen-helper`, installation lock, random launch token,
plan and ready file are reused. Helper staging verification includes ownership
preflight. `arm` returns only after readiness; the application saves work and
exits through its toolkit. `Handoff::cancel` remains safe before shutdown. The
parent exit deadline remains bounded by `exit_timeout_secs` (1..3600).

After parent exit, the helper rechecks staging/registration and records Applying.
It starts the exact installer path directly, with no command shell. Per-user
installers use `Command`. Machine installations use `ShellExecuteExW` with the
`runas` verb and `SEE_MASK_NOCLOSEPROCESS`, and the helper retains and waits on
the actual process handle. UAC cancellation/launch errors are failures. A
per-user policy never silently retries with elevation. Test alternate-account
UAC on your deployment; that account's per-user resources may differ.

Fixed arguments are:

| Argument | Purpose |
| --- | --- |
| `/SP-` | Suppress the initial setup prompt |
| `/VERYSILENT` | Hide Inno's wizard and installation progress window |
| `/SUPPRESSMSGBOXES` | Use Inno's documented defaults for suppressible message boxes |
| `/NORESTART` | Never initiate a reboot |
| `/RESTARTEXITCODE=3010` | Recognizable successful installation requiring reboot |
| `/NORESTARTAPPLICATIONS` | Keep application relaunch under host control |
| `/NOCLOSEAPPLICATIONS` | Do not let Restart Manager close unrelated application processes |
| `/DIR=<existing directory>` | Preserve the directory checked against local registration |
| `/LOG=<transaction>\inno.log` | Keep Inno diagnostics with the transaction |
| `/CURRENTUSER` or `/ALLUSERS` | Match the locally selected install scope |

An installer's own privilege directives still apply. These switches cannot
suppress UAC, custom Pascal Script dialogs, all early errors, or external
program UI. Your servicing script and `[Run]` entries must be reviewed for
silent operation. Use `skipifsilent` on interactive postinstall actions, and do
not launch the application in `[Run]` during Freshen servicing. The supplied
include does not rewrite `[Run]` or existing handlers. Freshen does not forcibly
close other instances: the host must coordinate all instances before shutdown.

Process exit is authoritative: 0 -> Committed/Completed; 3010 ->
RebootRequired; all other codes -> Failed, including unknown codes. Codes 1..8
have documented descriptions in `inno::exit_description`. Code 8 is failure to
proceed because preparation needs restart; it is not successful installation.
The snapshot retains the exact unsigned 32-bit exit code. A finalization message
and even progress equal to its total do not commit an update.

Once launched, cancellation is disabled (`Status.cancellable=false`). An overdue
installer sets `Status.overdue=true`; the helper keeps supervising with the lock
held until actual exit, without killing Inno or deleting staging. No hard
installer deadline is claimed. Report the warning and allow diagnostics; never
offer a destructive cancel button as though it were safe. Unexpected helper or
OS termination can leave Applying with an unknown outcome. Keep all evidence,
stop all application/installer processes, inspect registration/files and Inno
logs, and repair with the same AppId. Never automatically rerun an interrupted
installer or call the file-replacement rollback engine. `recover_installation`
refuses automatic Inno rollback. After an interrupted transaction has been
manually repaired and all processes stopped, an administrator/developer may
archive then remove the state root's `.freshen\transaction` directory. This is
an explicit maintenance operation, not an automatic recovery API.

`discard_finished_installation` accepts committed/reboot results and Inno
failures with a recorded process exit, retaining the normal lock check. Copy
logs first and wait for the helper process to fully exit before deleting its
copied executable on Windows. Unknown outcomes remain blocked. Before-launch cancellation can
discard the Prepared transaction through the existing API. Optional relaunch
failure is diagnostic and does not reverse a successful installation; no Inno
rollback is promised.

## Progress and accessible presentation

Download/verifying/extracting/ready events use the existing `Event` API.
`inno::status(state_root)` returns the latest durable `Status` from
`.freshen\transaction\inno-status.json`, or None before servicing starts.
The helper atomically replaces this bounded snapshot at at most 10 Hz, rather
than appending an unbounded queue. It includes Preparing, Installing,
Finalizing, Completed, Failed or RebootRequired; an optional actual `(current,
total)` pair; connection state, overdue flag, numeric diagnostic, detail and
exit code. It is a separate status API because a callback in a terminated host
cannot receive helper events. A surviving host UI process can poll; a
single-process app can inspect it on its next launch. Freshen creates no UI
surrogate. Your external reference application must choose its surviving UI
process arrangement if it needs to display installation while its main process
is stopped.

Basic mode and telemetry failure show indeterminate installation. Enhanced
values are Inno's installation work counters, not a promise about total update
time. Display them within Installing. Clear percentages during Finalizing;
keep completion contingent on process exit. Suppress regressing counters with
the same total, invalid totals and late installation messages after Finalizing.

Dispatch snapshots to the GUI thread. Keep focus stable; announce phase changes
and coarse progress milestones at an appropriate interval (for example five
seconds), not each 100 ms snapshot. Use native accessible progress controls with
indeterminate state where appropriate. The library does not open dialogs or
move focus. NVDA testing of the external reference GUI remains its host's job.

## Bridge implementation, security and protocol

There was no existing live process IPC transport: the existing helper exchanges
durable plans/readiness/results through files. Those remain appropriate for
handoff and recovery. For short-lived live telemetry, a local Windows named
pipe avoids a socket listener and can cross the normal-user/elevated boundary.

Implementation files:

| File | Responsibility |
| --- | --- |
| `src/inno.rs` | Local policy, preflight, status reducer, process supervision, registry/elevation |
| `inno-bridge/src/lib.rs` | Rust native DLL C ABI and coalescing |
| `src/inno_pipe.rs` | Shared native nonblocking pipe operations |
| `src/inno_wire.rs` | Shared fixed protocol encoding/validation |
| `inno/FreshenInno.iss` | Embedded DLL declarations and attributed event handlers |
| `inno/build-bridge.ps1` | Both MSVC architectures with static CRT |
| `tests/inno_installer.rs` | Opt-in compiled Inno 6/7 installation/update and migration fixtures |

The disposable demo apps/scripts were removed after validation. Complete host
bootstrap and installer source now live as documentation in
`docs/reference-app-inno.md`; the native presentation pattern lives in
`docs/host-integration.md`. The library/helper/installer regression fixtures stay.

The helper creates one first-instance pipe with `PIPE_REJECT_REMOTE_CLIENTS`,
message read mode and `PIPE_NOWAIT`. A protected DACL grants access only to the
owner, builtin administrators and SYSTEM (`D:P(A;;GA;;;OW)(A;;GA;;;BA)(A;;GA;;;SY)`).
Non-inheritable handles close through RAII. The name is
`\\.\pipe\freshen-inno-<64 random hex digits>`. The same 256-bit capability is
passed with `/FRESHENTOKEN=<64 hex digits>`; `/FRESHENPIPE=<name>` selects the
endpoint. These are private integration parameters, not standard Inno switches.
The include detects them through `{param:...|}`. The server checks the token on
every message. There are no control commands, paths, executable names, or
success messages in the protocol. Other users cannot connect; administrators
and the same account are within the trust boundary. Tokens in command lines or
Inno logs are **not secrets from another process running as the same user or an
administrator**. Do not claim this defends a compromised publisher, account or
administrator. Do not publish unredacted logs containing telemetry parameters.

Wire format is exactly 52 bytes, little endian:

| Offset | Length | Meaning |
| --- | --- | --- |
| 0 | 4 | ASCII `FINO` |
| 4 | 4 | Protocol version u32, currently 1 |
| 8 | 32 | Raw capability bytes decoded from hex |
| 40 | 4 | Kind u32: 1 preparing, 2 installing, 3 finalizing, 4 progress, 5 diagnostic |
| 44 | 4 | Current work counter, or diagnostic code |
| 48 | 4 | Total work counter (nonzero for progress) |

Oversize, short, unsupported version, unsupported kind, wrong token, invalid
counter and invalid phase messages disable the connection. The server processes
at most 16 frames per 100 ms poll. Kernel buffers are bounded to 4096 bytes. The
DLL uses nonblocking writes; full buffers or disconnection close its connection,
return an error and let installation continue. Progress callbacks coalesce to
at most 10 Hz; phases and diagnostics are immediate nonblocking writes.
Diagnostics preserve only the latest numeric code. Applications define codes
in their own documentation; they cannot execute commands.

The DLL attempts connection once with no wait or retry. The helper offers a
10-second connection window; basic installers, missing/wrong-architecture DLLs
and failed connections fall back to indeterminate progress. Pipe creation
failure does the same. There is no stalled-reader flush at close and no worker
queue to drain. Once a connected bridge fails, no reconnection is attempted.
Reporting failure never cancels a healthy installer. An installer log is still
available when telemetry is absent.

## DLL ABI and packaging

The crate is `freshen-inno-bridge`; output is `FreshenInno.dll`.
The DLL crate is built from this repository checkout and is not published as a
standalone Cargo package; its native transport/protocol source is shared with
the core crate under `src/` and included by path.
Exports use Rust
`extern "system"`: stdcall on 32-bit Windows and the Windows x64 ABI on x64.
Arguments are 32-bit integers or pointers to NUL-terminated UTF-16 strings,
matching Unicode Inno Pascal Script `String`. No Rust allocation crosses the ABI.
Call from Inno's serialized event callbacks. Return values: 0 accepted/coalesced,
1 invalid input, 2 disconnected/unavailable/full pipe, 3 concurrent/poisoned
state lock. No function waits for the helper. Do not call from DllMain.

| Export | Parameters | Result |
| --- | --- | --- |
| `FreshenInnoConnect` | `const uint16_t* endpoint`, `const uint16_t* token` | i32 status; endpoint at most 160 units, token exactly 64 ASCII hex digits |
| `FreshenInnoPhase` | u32 phase: 1 preparing, 2 installing, 3 finalizing | i32 status |
| `FreshenInnoProgress` | u32 current, u32 total | i32 status; total > 0 and current <= total |
| `FreshenInnoDiagnostic` | u32 application-defined code | i32 status |
| `FreshenInnoDisconnect` | none | void; idempotent resource close |

Connect pointers must reference valid readable memory through the terminator.
Null or invalid text is rejected; arbitrary invalid native pointers are outside
the C ABI contract. No progress function reports installation completion.

Build in a VS 2022 C++ development environment with Windows SDK and Rust 1.88+
(current stable recommended):

```powershell
rustup target add i686-pc-windows-msvc x86_64-pc-windows-msvc
.\inno\build-bridge.ps1
```

The script runs these commands separately with the environment setting below,
restoring the previous value afterward:

```powershell
$env:RUSTFLAGS = '-C target-feature=+crt-static'
cargo build --locked --release -p freshen-inno-bridge --target i686-pc-windows-msvc
cargo build --locked --release -p freshen-inno-bridge --target x86_64-pc-windows-msvc
```

Outputs are `target\i686-pc-windows-msvc\release\FreshenInno.dll` and
`target\x86_64-pc-windows-msvc\release\FreshenInno.dll`. The DLL depends on
Windows system libraries, with the CRT linked statically; no separate Rust
runtime is distributed. Build both after modifying shared wire/pipe code.

Inno 6's installer process is 32-bit even when it installs a 64-bit application:
embed the **i686** DLL. A native 64-bit Inno 7 installer needs the **x86_64** DLL.
A 32-bit installer needs i686 regardless of application architecture. Select
using the installer compiler/configuration, not the Rust application's target.
Inno 7.1.1 x86 and native-x64 servicing are exercised by the real fixture
matrix, including an Inno 6/x86 installation upgraded by native x64 Inno 7.
`SetupArchitecture=x64` selects a native x64 setup process but also defaults
to 64-bit install mode. Preserve your previous install mode explicitly when
migrating: our Inno 6 migration fixture pins `ArchitecturesInstallIn64BitMode=`
(empty) because its original script used 32-bit install mode. A native x64 setup process
can run in that mode with the x64 DLL. Changing install mode can create a
separate uninstall log even with the same AppId and directory. See
[Inno's application identity rules](https://jrsoftware.org/ishelp/topic_sameappnotes.htm).

Define `FreshenInnoDll` to the selected absolute DLL path, then include
`FreshenInno.iss` at top level of your script. It adds one `[Files]` entry with
`dontcopy noencryption` and `[Code]` handlers using event attributes, avoiding
handler-name collisions. `files:FreshenInno.dll` in delayed-load declarations
extracts the embedded DLL into Inno's temporary area, rather than installing
it into the application directory. Do not rename the embedded DLL. It does not
need to be shipped loose beside setup.exe. Existing scripts may place sections
after the include; they must explicitly reopen the appropriate section.

Without both private parameters, initialization does not call or load the DLL.
Manual interactive installations require no Freshen process. Load/connect or
report exceptions are logged and ignored. Event handlers always allow setup to
continue. You may declare `FreshenInnoDiagnostic` with the same external syntax
for application-specific warnings; handle its status as the include does.

## Validation and troubleshooting

Run the normal checks and the architecture-specific bridge tests:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo test --workspace --doc --locked
cargo test -p freshen-inno-bridge --target i686-pc-windows-msvc --locked
cargo package --locked -p freshen
$env:FRESHEN_ISCC = "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
cargo test --locked --test inno_installer -- --ignored --nocapture
```

Install Inno 6 with `winget install --id JRSoftware.InnoSetup --exact` if needed.
The real suite builds installers into temporary directories, uses a unique
per-user AppId, exercises the signed ZIP and parent/helper handoff, and
uninstalls on successful completion. A failed test can leave its test AppId
registration and installer processes behind; inspect and stop them before
cleanup. No suite exercises a production application or machine-wide install.

Bridge unit tests cover real named pipes, exported function behavior, actual
counters, invalid input, disconnected peers and a saturated pipe without
blocking. Core tests cover malformed frames, token mismatch, non-regressing
phases/progress, exit-code authority, argument quoting and subprocess lifetime.
The opt-in Inno suite covers initial install, basic/enhanced updates, preparation
failure, reboot code, deliberate disconnection, overdue supervision, files in
use and a mismatched DLL with successful telemetry fallback.
The manual checks below remain required for a release.

| Symptom | Check |
| --- | --- |
| Preflight says missing registration | Exact AppId including braces, HKCU/HKLM scope, registry view, InstallLocation |
| Installer fails to start | UAC cancellation, policy scope, executable permissions, helper-error.txt |
| No percentages | Basic mode is indeterminate; check DLL bitness and bridge exception in inno.log |
| Enhanced disconnected | Helper lifetime, pipe access, buffer pressure; installation may still succeed |
| Failed with unknown exit code | Keep numeric exit code and log; do not infer success |
| Files remain locked | Stop all application instances; inspect Inno log; host cannot cancel safely during replacement |
| Overdue stays true | Installer still running; inspect custom script and external process waits; do not kill blindly |
| Stale Applying after crash | Unknown outcome; manual Inno repair, never Freshen file rollback |

Before shipping, test ordinary interactive install with no Freshen running;
your actual application's Inno 6/7 migration; machine-wide UAC acceptance/cancellation and alternate
credentials; file locks and pending-reboot replacement; all application
instances exiting; helper/installer/OS crashes; registry identity before and
after update; and NVDA on the real surviving update UI. The automation and
actual results for this change are recorded in the rebuild guide.

Upstream references: [setup switches](https://jrsoftware.org/ishelp/topic_setupcmdline.htm),
[exit codes](https://jrsoftware.org/ishelp/topic_setupexitcodes.htm),
[script events](https://jrsoftware.org/ishelp/topic_scriptevents.htm),
[external DLLs](https://jrsoftware.org/ishelp/topic_scriptdll.htm).
