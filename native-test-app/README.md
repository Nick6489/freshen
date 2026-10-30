# Native Windows UI update test

This is a disposable Rust/Win32 application for testing Freshen's Inno backend
before building the external reference application. It uses genuine
resource-defined `DIALOGEX` templates and `DialogBoxParamW`, not a custom window
imitating a dialog. Inno 7 builds native x64 per-user installers. Both application
versions, the x64 bridge DLL, helper and signed update payload are built locally.
Nothing is published and no network or certificate-store change is required.

## Build

Requirements: Windows, Rust 1.88+ with x64/i686 MSVC targets, Visual Studio C++
Build Tools/Windows SDK (including `rc.exe`), PowerShell and Inno Setup 7.
From the repository root:

```powershell
.\native-test-app\scripts\build-demo.ps1
```

Pass `-Iscc 'C:\path\to\Inno Setup 7\ISCC.exe'` if it is not installed under
`%LOCALAPPDATA%\Programs\Inno Setup 7`. The build script locates the Windows SDK
resource compiler; set `RC` to its absolute path if your SDK is elsewhere.
Only the test app requires resources; the Freshen library remains GUI-free.

Outputs under `target\native-ui-demo`:

| Path | Purpose |
| --- | --- |
| `installers\FreshenNativeTest-1.0.0-x64.exe` | Initial interactive installer |
| `installers\FreshenNativeTest-2.0.0-x64.exe` | New version, executed by the updater |
| `v1`, `v2` | Version-specific EXE and helper staging |
| `payload\setup.exe` | Single-file input to the existing release tool |
| `feed\manifest.json`, `manifest.json.sig`, `update.zip` | Authenticated version 2 update |
| `private\publisher.key`, `publisher.pub` | Generated test-only signing key pair |

The private key is generated once and reused on rebuild so installed version 1
continues to trust future demo payloads. The public key is compiled into both
EXEs using `FRESHEN_UI_PUBLIC_KEY_FILE`; there is no runtime trust-key override.
`FRESHEN_UI_VERSION=1.0.0|2.0.0` controls both the UI version and native VERSIONINFO
resource. All generated packages and keys stay in ignored `target/`. Do not
delete the key pair and expect an older installed build to trust a new pair.
These are disposable test credentials, never production signing material.

## Install, enable the update and run it

1. Run `FreshenNativeTest-1.0.0-x64.exe` normally and complete Inno's native wizard.
   It creates a Start menu shortcut and can open the app at the end. Verify
   `Installed version: 1.0.0` in the main dialog. This is a separate installation
   from the earlier console `Freshen Inno Example`.
2. From the checkout, activate the signed feed:

   ```powershell
   .\native-test-app\scripts\enable-update.ps1
   ```

3. Choose **Check for updates**. The host uses the real Freshen `check` and
   `prepare` operations on a worker thread and offers **Install update** only
   after authentication/extraction succeed. The test-text field remains usable.
4. Choose **Install update**. The app arms the real copied helper and starts a
   separate copy of its presentation process outside the application directory.
   The original dialog closes only after the helper and monitor are ready.
5. The native update dialog remains visible while Inno replaces the stopped app.
   It shows Installing and Finalizing, followed by the actual installer outcome.
   Close/Escape are unavailable while installation is in progress. Choose
   **Open updated app** after success and verify `Installed version: 2.0.0`.

The installed app is `%LOCALAPPDATA%\Programs\FreshenNativeTest` with AppId
`FreshenNativeDialogTest`. Both installers retain that identity, per-user scope,
directory and 64-bit install mode. The feed is
`%LOCALAPPDATA%\FreshenNativeDialogTestFeed`. The helper/status/log root is the
separate `%LOCALAPPDATA%\FreshenNativeDialogTestServicing` directory.

The host's deliberate `LocalFeed` implementation maps exactly three fixed
`https://freshen-ui-test.invalid/` URLs to the local feed files. No HTTP request
is made. This exercises Freshen's supported custom-transport API; it does not
relax the default HTTP transport's TLS policy, publisher signature verification,
download/extraction limits or helper stage validation. Publishing this dummy
feed is not a substitute for the later reference app's actual distribution
configuration.

The script adds test-only work delays of 1.8 seconds at installation and 2.2
seconds during finalization, only when invoked with Freshen telemetry parameters.
These make UI states observable; they do not invent progress percentages or
block the UI. Manual initial installation has no such delays. Installation
counters come from the actual Inno callback. Finalizing is indeterminate, and
only process exit 0 enables Open updated app. Exit 3010 reports reboot required
without initiating a reboot. Bridge failure leaves installation indeterminate.

## Automated tests

Normal standalone UI test (no installation or activated feed required):

```powershell
cargo test -p freshen-native-test-app --test dialogs --locked -- --nocapture
```

It launches only its own dialog with an isolated temporary LOCALAPPDATA and
checks `#32770`, native edit controls, read-only status, tab traversal past the
disabled Install button, editable text, Enter invoking the default action,
bounded responsiveness, a failed check leaving installation disabled and Escape
closing the dialog. No fixture is silently installed by this normal test.

After manually installing version 1 and enabling the signed version 2 feed:

```powershell
cargo test -p freshen-native-test-app --test dialogs --locked -- --ignored --nocapture
```

This one-shot test uses the existing installed main dialog or launches it if
closed. Close duplicate dummy-app windows first. It queues native `BN_CLICKED`
commands on that dialog's UI thread, verifies the authenticated update becomes
available, installs it and inspects the real separate monitor process. It checks
that the UI responds, Close/WM_CLOSE are blocked during replacement, finalization
uses a marquee, real telemetry was observed, exit code is 0, the journal is
Committed and no second uninstaller appears. It opens the updated app, asserts
version 2.0.0 and closes that window. It never kills a helper or installer, even
if an assertion fails.

The harness discovers top-level windows by cached captions, verifies the target
process image, and sends bounded messages only to this test app's controls.
It does not inject desktop-wide mouse/keyboard input or require your window to
stay foreground. The standalone Enter/Escape checks pass through the actual
dialog-manager message queue. Native control-command assertions are distinct
from physical keyboard/mouse or NVDA testing.

## Native accessibility and manual checks

The dialogs use Segoe UI dialog units, standard buttons/edit/progress controls,
resource tab order, adjacent static labels, mnemonics, native default buttons
and the standard `#32770` dialog role. Static/edit values change by phase, not
on every download callback. Visual installation counters update at most once
per second; there are no unsolicited focus changes during checks or progress.
The main dialog initially focuses Check for updates; the update dialog initially
focuses its status field. A failed check preserves the main dialog instead of
opening an unsolicited error modal.

Manual checks with NVDA: read the dialog name/version, navigate the labelled
test-text and status fields, use Alt+C to check, Tab/Shift+Tab between available
controls, verify Install becomes available only after preparation, and listen
to Installing/Finalizing/Completed without repeated progress announcements.
Verify Escape closes the idle main dialog and the completed monitor but cannot
cancel replacement. Verify the progress UI survives the original app's exit.
No automated native-control test alone establishes NVDA behavior.

## Logs, repeat runs and reset

Evidence remains under the servicing root:

- `.freshen\transaction\inno.log`: actual Inno diagnostics.
- `.freshen\transaction\inno-status.json`: current/final phase, telemetry
  evidence and process exit code.
- `.freshen\transaction\journal.json`: durable Freshen installation result.
- `.freshen\transaction\helper-error.txt`: helper failure, when present.

A prior transaction intentionally prevents another arm. After closing the app
and completed monitor, repeat the disposable test with:

```powershell
.\native-test-app\scripts\reset-demo.ps1 -ReinstallVersion1
```

The script refuses if an app/helper/monitor from the test directories remains
running. It archives the transaction under ignored
`target\native-ui-demo\log-archive`, calls the existing safe discard API via
`freshen-release discard-finished ROOT`, then silently reinstalls the disposable
version 1 fixture only when that switch is supplied. It never kills processes,
implements rollback, or recursively deletes the servicing root. Unknown or
active outcomes remain blocked by Freshen's recovery rules. The signed version
2 feed remains enabled for the next run. Close all app instances to avoid files
in use; the installer does not force other instances to exit.

For uninstall, use Windows Installed apps or `unins000.exe` in the test app
directory. User-local feed/servicing evidence is retained for diagnosis.

This is a local UI test harness, not the later distributable reference app.
Machine-wide UAC, alternate credentials, NVDA/manual UI behavior and crash
recovery remain separate checks. No production release or crates.io publication
is performed.

## Validation performed

On this Windows machine with Inno 7.1.1, both native x64 installers and the signed
version 2 package were built. The user completed the version 1 installation.
The standalone native-dialog test passed. The installed-app UI test then passed
in 7.37 seconds: it adopted the open version 1 dialog, prepared the signed update,
observed the separate responsive monitor with blocked close and indeterminate
finalization, verified real telemetry/exit 0/Committed/single uninstaller, and
opened and verified version 2.0.0. Version 2 is installed on this machine.

Computer-use accessibility inspection of the installed version 2 independently
reported a native dialog, `Installed version: 2.0.0`, labelled/settable test edit,
labelled read-only Status edit and correctly named/enabled/disabled buttons.
Screenshot capture failed with `FrameArrived timed out`; the accessibility-only
inspection succeeded. A later physical click was not injected because the tool
detected concurrent user input. These observations do not constitute a completed
NVDA or visual-layout audit. The automated native tests use bounded control
commands and dialog-manager messages as described above.
