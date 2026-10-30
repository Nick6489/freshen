# Rebuild the reference application with Inno servicing

The external Freshen reference GUI application is not present in this checkout
and is intentionally not modified by this change. This guide supplies a complete
buildable console integration harness (`examples/inno_host.rs`) and installer
(`inno/example.iss`) using the actual Freshen manifest, publisher and helper APIs.
It also identifies exactly what to transfer to the reference application. No GUI
framework, reference-app-specific path or nonexistent build target is assumed.

## Tools and checkout

Use Windows with Visual Studio 2022 Build Tools (Desktop development with C++,
MSVC x86/x64 and Windows SDK), Rust 1.88 or later (stable recommended), PowerShell,
Inno Setup 6.7.3 or 7.1.1 (both tested), Python 3.10+ and OpenSSL 3.x for the local
HTTPS demonstration. Rust does not cross-link MSVC targets without the Windows
toolchain. The core APIs remain usable on the project's existing platforms;
servicing and the DLL require Windows. Native x64 Inno 7 and Inno 6-to-7
migration are covered by the optional fixture matrix below.

From a clean checkout in a VS developer PowerShell:

```powershell
git clone https://github.com/Nick6489/freshen.git
Set-Location freshen
rustup target add i686-pc-windows-msvc x86_64-pc-windows-msvc
cargo build --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo test --workspace --doc --locked
```

Install the compiler if needed:

```powershell
winget install --id JRSoftware.InnoSetup --exact --silent --accept-package-agreements --accept-source-agreements
$iscc = "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
if (!(Test-Path $iscc)) { $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" }
if (!(Test-Path $iscc)) { throw 'Locate ISCC.exe and set $iscc' }
$repo = (Get-Location).Path
```

## Build both bridge architectures

```powershell
.\inno\build-bridge.ps1
cargo test -p freshen-inno-bridge --locked
cargo test -p freshen-inno-bridge --target i686-pc-windows-msvc --locked
```

The script uses `RUSTFLAGS=-C target-feature=+crt-static` only for each build:

```powershell
cargo build --locked --release -p freshen-inno-bridge --target i686-pc-windows-msvc
cargo build --locked --release -p freshen-inno-bridge --target x86_64-pc-windows-msvc
```

The two `FreshenInno.dll` files appear beneath the corresponding
`target\<triple>\release` directory. Inno 6 always embeds the **i686 DLL**, even
when the packaged application is x64. A native x64 Inno 7 installer embeds the
x86_64 DLL. Static CRT builds need no separate Rust runtime. The EXE contains
the DLL through the include; do not separately install it into `{app}`.

## Build version 1 and its initial installer

For Inno 7 native x64, see the dedicated instructions below; the baseline
commands in this section retain Inno 6 compatibility.

```powershell
$env:FRESHEN_EXAMPLE_VERSION = '1.0.0'
cargo build --locked --release --example inno_host --bin freshen-helper --bin freshen-release
New-Item -ItemType Directory -Force target\inno-app-v1 | Out-Null
Copy-Item target\release\examples\inno_host.exe target\inno-app-v1\
Copy-Item target\release\freshen-helper.exe target\inno-app-v1\
$bridge = "$repo\target\i686-pc-windows-msvc\release\FreshenInno.dll"
& $iscc '/DAppVersion=1.0.0' "/DSourceDir=$repo\target\inno-app-v1" "/DFreshenInnoDll=$bridge" inno\example.iss
if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed' }
```

`inno/example.iss` is the complete working script. Its effective configuration is:

```iss
[Setup]
AppId=FreshenInnoExample
AppName=Freshen Inno Example
AppVersion={#AppVersion}
DefaultDirName={localappdata}\FreshenInnoExample
PrivilegesRequired=lowest
UninstallDisplayIcon={app}\inno_host.exe
OutputDir=..\target\inno-installers
OutputBaseFilename=FreshenExample-{#AppVersion}
Compression=lzma2
SolidCompression=yes
CloseApplications=no
RestartApplications=no
[Files]
Source: "{#SourceDir}\inno_host.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\freshen-helper.exe"; DestDir: "{app}"; Flags: ignoreversion
#include "FreshenInno.iss"
```

The actual file also defines/defaults `AppVersion` and validates `SourceDir`.
The include validates `FreshenInnoDll`. Pass all three with `/D` as above.
It contributes its own `[Files]` and `[Code]` sections. It uses event attributes
and delayed `files:` DLL declarations, allowing existing event handlers. Keep
AppId and PrivilegesRequired stable between versions. Your real script must
avoid custom dialogs and interactive `[Run]` actions in silent mode.

Run the initial installer interactively with no Freshen process running:

```powershell
& .\target\inno-installers\FreshenExample-1.0.0.exe
$installed = "$env:LOCALAPPDATA\FreshenInnoExample"
& "$installed\inno_host.exe" --version
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\FreshenInnoExample_is1'
$before = Get-ItemProperty $uninstallKey
$before | Select-Object DisplayName, DisplayVersion, InstallLocation, UninstallString
```

Complete the native Inno wizard normally. This confirms manual installation
does not depend on Freshen or a live pipe. Expect version `1.0.0`. Keep the
recorded uninstall path for comparison. To automate the initial install on a
test machine instead, pass `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CURRENTUSER`.

## Build version 2

```powershell
$env:FRESHEN_EXAMPLE_VERSION = '2.0.0'
cargo build --locked --release --example inno_host --bin freshen-helper --bin freshen-release
Remove-Item Env:\FRESHEN_EXAMPLE_VERSION
New-Item -ItemType Directory -Force target\inno-app-v2 | Out-Null
Copy-Item target\release\examples\inno_host.exe target\inno-app-v2\
Copy-Item target\release\freshen-helper.exe target\inno-app-v2\
& $iscc '/DAppVersion=2.0.0' "/DSourceDir=$repo\target\inno-app-v2" "/DFreshenInnoDll=$bridge" inno\example.iss
if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed' }
```

For the external reference app, this step builds its new executable/assets
instead of the harness. Do not change its established AppId or uninstall scope.

## Inno Setup 7 and native x64 installers

Inno Setup 7 can coexist with Inno 6. Install its recommended x64 compiler:

```powershell
winget install --id JRSoftware.InnoSetup.7 --exact --source winget --silent --accept-package-agreements --accept-source-agreements
$iscc7 = "$env:LOCALAPPDATA\Programs\Inno Setup 7\ISCC.exe"
```

If you installed for all users, locate `ISCC.exe` under `Program Files` instead.
Installing the x64 compiler does **not** select an x64 installer: Inno 7 still
defaults to x86. For the example's existing version-2 application staging folder:

```powershell
$bridge64 = "$repo\target\x86_64-pc-windows-msvc\release\FreshenInno.dll"
& $iscc7 '/DAppVersion=2.0.0' '/DFreshenSetupArchitecture=x64' "/DSourceDir=$repo\target\inno-app-v2" "/DFreshenInnoDll=$bridge64" inno\example.iss
if ($LASTEXITCODE -ne 0) { throw 'Native x64 installer compilation failed' }
```

Output: `target\inno-installers\FreshenExample-2.0.0-x64.exe`.
The example macro emits `[Setup] SetupArchitecture=x64`; it is not a built-in
Inno command-line switch. In your own installer script, set that directive
directly and embed the x64 DLL. `SetupArchitecture=x64` also changes Inno's
default allowed architecture and installation mode to x64-compatible. Review
any explicit registry-view or Program Files directives in your real script.
The example explicitly overrides `ArchitecturesInstallIn64BitMode=` (empty)
to retain its Inno 6 installation's 32-bit install mode and existing uninstall
log. The setup process and DLL still run as native x64. A previously 64-bit
install-mode application should instead retain its original 64-bit install-mode
directive. Changing mode is a separate migration: our test caught creation of
`unins001.exe` when the old 32-bit mode was accidentally changed to 64-bit.
Keep the original AppId, directory, privilege scope **and install mode** when
servicing an existing installation. Compiler architecture, installer process
architecture, install mode and application architecture are separate choices.

For an Inno 7 x86 installer, pass `/DFreshenSetupArchitecture=x86` and embed the
i686 DLL; its output filename ends in `-x86.exe`. Omitting the macro preserves
the old example filename and default x86 behavior. Do not pass the macro to
Inno 6, which does not support `SetupArchitecture`.

Run the complete real-installer matrix for native x64:

```powershell
$env:FRESHEN_ISCC = $iscc7
$env:FRESHEN_SETUP_ARCH = 'x64'
cargo test --locked --test inno_installer -- --ignored --nocapture
```

Then validate migration from an Inno 6/x86 initial installation:

```powershell
$env:FRESHEN_INITIAL_ISCC = "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
cargo test --locked --test inno_installer -- --ignored --nocapture
Remove-Item Env:\FRESHEN_INITIAL_ISCC
```

For Inno 7 x86, set `FRESHEN_SETUP_ARCH=x86` and rerun. The tests automatically
swap the correct/mismatched DLLs for the selected installer architecture. When
returning to Inno 6 tests, set `FRESHEN_ISCC` back to its compiler and remove
`FRESHEN_SETUP_ARCH` (or set it to `x86`). Neither test setting changes the
application target in the signed manifest or the public Freshen policy API.

Upstream reference: [64-bit Inno Setup](https://jrsoftware.org/ishelp/topic_64bit.htm).

## Create and sign the actual Freshen release

Store demo keys and certificates **outside the checkout**:

```powershell
$demo = Join-Path $env:TEMP ('FreshenInnoDemo-' + [guid]::NewGuid())
New-Item -ItemType Directory $demo | Out-Null
New-Item -ItemType Directory -Force target\inno-payload, target\inno-distribution | Out-Null
Copy-Item target\inno-installers\FreshenExample-2.0.0.exe target\inno-payload\setup.exe
& .\target\release\freshen-release.exe keygen "$demo\publisher.key" "$demo\publisher.pub"
& .\target\release\freshen-release.exe pack target\inno-payload target\inno-distribution\reference-2.0.0.zip https://localhost:8443/reference-2.0.0.zip x86_64-pc-windows-msvc inno freshen-inno-example 2.0.0 stable target\inno-distribution\freshen-manifest.json
& .\target\release\freshen-release.exe sign "$demo\publisher.key" target\inno-distribution\freshen-manifest.json target\inno-distribution\freshen-manifest.json.sig
```

These tools deliberately refuse to overwrite output files. For a repeat run,
archive/delete only the known previous output files or use a clean demo
directory. `inno-payload` must contain only `setup.exe`. Schema 1, product
`freshen-inno-example`, channel `stable`, target `x86_64-pc-windows-msvc` and
the two versions match the harness. The target identifies the **application**
distribution; it does not select the DLL bitness. Customize these local host
values and manifest fields together for your real app. Add release notes to
the manifest before signing; never edit it after signing.

For production, upload the ZIP, exact manifest and detached signature to your
existing HTTPS distribution service. `ReleaseSource::Manifest` consumes the
manifest/signature pair; the ordinary Index, Manifests and GitHub discovery
mechanisms work unchanged. GitHub discovery expects names
`freshen-manifest.json` and `freshen-manifest.json.sig` alongside the ZIP.
Keep production signing keys offline and pin their public keys in the host.

## Serve a reproducible local HTTPS release

The default transport deliberately rejects plain HTTP. Use a short-lived
loopback certificate trusted by this test user's Windows certificate store:

```powershell
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$demo\tls.key" -out "$demo\tls.pem" -days 2 -subj '/CN=localhost' -addext 'subjectAltName=DNS:localhost'
certutil -user -addstore Root "$demo\tls.pem"
py -3 inno\serve-updates.py --directory target\inno-distribution --cert "$demo\tls.pem" --key "$demo\tls.key"
```

Leave the server running. Use a second PowerShell at the same checkout and set
`$demo` to the printed/recorded temporary directory from the first shell.
This certificate is only a development HTTPS server certificate; the separate
Ed25519 publisher signature is still required. Do not disable TLS verification.

## Trigger and inspect the update

```powershell
$installed = "$env:LOCALAPPDATA\FreshenInnoExample"
$state = "$env:LOCALAPPDATA\FreshenInnoExampleServicing"
& "$installed\inno_host.exe" 1.0.0 https://localhost:8443/freshen-manifest.json "$demo\publisher.pub" $state $installed enhanced
$snapshot = Join-Path $state '.freshen\transaction\inno-status.json'
do {
    Start-Sleep -Milliseconds 250
    if (Test-Path $snapshot) {
        $s = Get-Content -Raw $snapshot | ConvertFrom-Json
        $s | Select-Object phase, progress, telemetry_observed, progress_observed, overdue, exit_code
    }
} while (!$s -or $null -eq $s.exit_code)
Get-Content -Raw (Join-Path $state '.freshen\transaction\journal.json')
```

The console reports Checking, Downloading, Verifying, Extracting, Ready, then
helper readiness. It exits normally so the helper can install. Snapshots report
Preparing, Installing (real counters if available), Finalizing, Completed; fast
installers may finish between polls. `telemetry_observed`/`progress_observed`
retain evidence after the DLL normally disconnects. `telemetry_connected`
describes current connection only. `exit_code=0` and journal `Committed` mean
installer success. RebootRequired/3010 means the host should offer reboot later;
Freshen never initiates it. Basic mode uses the same command with `basic` and
has indeterminate progress. An ordinary installer with no include works in that
mode, provided its AppId and scope match the local policy.

The harness disables automatic relaunch. Verify the result explicitly:

```powershell
& "$installed\inno_host.exe" --version
$after = Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\FreshenInnoExample_is1'
$after | Select-Object DisplayName, DisplayVersion, InstallLocation, UninstallString
Get-ChildItem "$installed\unins*.exe"
Get-Content (Join-Path $state '.freshen\transaction\inno.log')
```

Expect `2.0.0`, the same uninstall key and directory, the same uninstaller path
as version 1, and no second independent installation. Use Windows Installed
apps and the uninstaller to verify removal. The host can call
`discard_finished_installation` after preserving diagnostics; the demonstration
retains them so you can inspect them. Reusing the state root without resolving
or discarding its prior transaction intentionally fails.

When done, stop the server with Ctrl+C, uninstall the example through Installed
apps (or its uninstaller), and remove the development certificate:

```powershell
$fingerprint = (openssl x509 -in "$demo\tls.pem" -noout -fingerprint -sha1).Split('=')[1].Replace(':','')
certutil -user -delstore Root $fingerprint
```

Remove the recorded temporary demo directory and servicing data only after all
helpers/installers have exited and you no longer need logs. Never commit demo
keys, certificates, generated EXEs or downloaded packages.

## Automated real-installer test and remaining reference-app work

```powershell
$env:FRESHEN_ISCC = $iscc
cargo test --locked --test inno_installer -- --ignored --nocapture
```

This opt-in test builds real temporary Inno 6/7 installers and exercises initial
installation, version replacement, basic mode, enhanced mode, preparation
failure, reboot-required result and wrong-bitness DLL fallback. It uses the real
signed manifest/ZIP and copied helper. Native pipe/DLL tests run on both x86 and
x64. See [the architecture and ABI guide](inno.md) for exact coverage and the
manual deployment matrix. Tests do not claim validation of UAC,
reference GUI/NVDA behavior or all crash/file-lock cases.

Validation performed on Windows in this implementation:

- Formatting, strict workspace Clippy, workspace/all-target tests, documentation
  tests and core crate packaging verification passed. Packaging used
  `cargo package --locked --offline -p freshen --allow-dirty` before commit
  because the sandbox cannot reach the registry. Cargo noted the existing
  locked `yoke-derive 0.8.3` is yanked; dependencies were not changed to hide it.
- Both static-CRT DLL release builds succeeded; native pipe/ABI unit tests
  passed on x64 and i686.
- Inno 6.7.3 compiled the example installer. Both harness versions built in
  release mode. The real opt-in suite passed initial installation, basic and
  enhanced update with observed real counters, failure exit 7, reboot exit
  3010, wrong-bitness DLL fallback, deliberate IPC disconnection, overdue
  handling without killing Inno, and a locked destination file producing a
  non-success result. The unchanged AppId registration is checked at every
  update and the fixture asserts a single existing uninstaller.
- On retry, the example's native interactive wizard opened and the user
  completed it manually. The installed executable reported 1.0.0 and its HKCU
  uninstall registration had the expected AppId, directory and uninstaller.
  Automated control capture did not succeed, so this is a manual interactive
  smoke test, not an automated screen-reader/UI test. UAC,
  reference GUI/NVDA and OS-crash testing remain unexecuted.
- Inno 7.1.1 was subsequently installed side by side. The full real-installer
  matrix passed for x86, native x64 (default 64-bit install mode), and migration
  from an Inno 6/x86 initial installation to native x64 Inno 7 while preserving
  the original 32-bit install mode. Enhanced mode observed real counters with
  the matching DLL; the opposite-bitness DLL fell back safely. Both example
  installer architectures compiled. An initial migration run that allowed the
  install-mode default to change produced `unins001.exe` and failed the identity
  assertion; explicitly retaining the existing mode resolved it. There was no
  manual interactive Inno 7 wizard or machine-wide UAC test.

To build the next external reference application version:

1. Add Rust-host `InstallLayout::Inno` policy using its existing registered
   AppId, scope and directory, a separate private servicing root, and its own
   installed executable. The existing language-binding arming structs do not
   expose this policy yet; a non-Rust host needs a binding extension.
2. Keep its existing download/trust/discovery and helper readiness flow; offer
   saving work and coordinate every app instance before shutdown. Arrange a
   surviving presentation process if installation progress must remain visible.
3. Map `inno::status` snapshots to accessible UI through its normal dispatcher,
   disable cancellation once installation starts, handle 3010 and retain logs.
   Do not call `confirm_startup` expecting automatic Inno rollback.
4. Include `FreshenInno.iss` in its existing installer and embed the DLL matching
   the installer process, preserving AppId, scope, directories and uninstall
   behavior. Review custom dialogs and `[Run]` entries for silent operation.
5. Build the old/new app versions through that app's own build system, compile
   installers, pack the new setup.exe using `inno`, sign and publish using its
   pinned production publisher key and real product/channel/target settings.
6. Run interactive, update, files-in-use, reboot, interruption, scope/UAC,
   uninstall identity, and NVDA checks on the actual packaged reference app.

No crates.io publication or production release is part of this procedure.
