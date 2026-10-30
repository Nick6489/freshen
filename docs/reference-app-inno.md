# Building the future reference application with Inno

The disposable GUI and console demonstration programs have been removed at the
user's request. The public audio-game reference application will be a separate
project. This guide documents its integration and includes a complete optional
backend scaffold that can be created outside the checkout; no reference-app
source or generated signing material is maintained inside the Freshen crate.
The demonstrated native UI pattern is preserved in [host integration](host-integration.md).

## Build Freshen and both DLL architectures

Requirements: Windows, Rust 1.88+ (stable recommended), Visual Studio 2022 C++
Build Tools/Windows SDK, PowerShell and Inno Setup 7.1.1. Inno 6.7.3 is also
tested for x86 installers. From the Freshen checkout:

```powershell
$freshen = (Get-Location).Path
cargo build --workspace --locked
cargo build --release --locked --bin freshen-release
rustup target add i686-pc-windows-msvc x86_64-pc-windows-msvc
.\inno\build-bridge.ps1
```

The bridge script sets `RUSTFLAGS=-C target-feature=+crt-static` per build and
restores it. Outputs are `target\i686-pc-windows-msvc\release\FreshenInno.dll`
and `target\x86_64-pc-windows-msvc\release\FreshenInno.dll`. Inno 6 needs i686
even for an x64 application. Inno 7 defaults to an x86 setup process; explicitly
set `SetupArchitecture=x64` and embed the x64 DLL for native x64 setup. Both
compiler versions can coexist. Install 7 if necessary:

```powershell
winget install --id JRSoftware.InnoSetup.7 --exact --source winget --silent --accept-package-agreements --accept-source-agreements
$iscc = "$env:LOCALAPPDATA\Programs\Inno Setup 7\ISCC.exe"
```

Locate `ISCC.exe` under Program Files instead if installed for all users.

## Host startup and update operations

For the real application, dispatch `run_helper` before the GUI, audio engine,
assets or single-instance lock. Establish this running copy's local install
policy, use `check_for_kind` and prepare on a worker thread. Save work and obtain
the user's installation decision, then `arm_current_exe` and quit normally only
after helper readiness. If its copied executable cannot start without adjacent
audio/runtime DLLs, use an explicitly deployed standalone helper instead.
The [distribution guide](distributions.md) explains portable/Inno side-by-side
selection; the [Inno guide](inno.md) documents the full policy and ABI.

For a reproducible backend-only scaffold, create a separate project:

```powershell
$work = Join-Path $env:TEMP ('FreshenReference-' + [guid]::NewGuid())
New-Item -ItemType Directory $work | Out-Null
cargo new --bin --name freshen-reference-app "$work\app"
& "$freshen\target\release\freshen-release.exe" keygen "$work\publisher.key" "$work\publisher.pub"
$env:FRESHEN_PUBLISHER_KEY = (Get-Content -Raw "$work\publisher.pub").Trim()
$dependencyPath = $freshen.Replace('\','/')
@"
[package]
name = "freshen-reference-app"
version = "1.0.0"
edition = "2024"
[dependencies]
freshen = { path = "$dependencyPath" }
base64 = "0.22"
"@ | Set-Content "$work\app\Cargo.toml"
```

Replace its `src\main.rs` with this complete console bootstrap. This scaffold
has no GUI/audio: the real reference app supplies those and its surviving
presentation process. The CLI takes a manifest URL and a private servicing root.

```rust
use base64::{engine::general_purpose::STANDARD, Engine};
use freshen::{inno::InnoPolicy, *};
use std::{path::PathBuf, time::Duration};

fn main() -> Result<()> {
    if run_helper(std::env::args_os().skip(1))? { return Ok(()); }
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--version") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.len() != 2 {
        println!("freshen-reference-app MANIFEST_URL SERVICING_ROOT");
        return Ok(());
    }
    let key: [u8; 32] = STANDARD.decode(env!("FRESHEN_PUBLISHER_KEY"))
        .map_err(|_| Error::Invalid("bad compiled public key".into()))?
        .try_into().map_err(|_| Error::Invalid("bad public key length".into()))?;
    let executable = std::env::current_exe()?;
    let policy = InstallPolicy {
        root: PathBuf::from(&args[1]),
        layout: InstallLayout::Inno(InnoPolicy {
            installer: "setup.exe".into(), app_id: "FreshenReference".into(),
            directory: executable.parent().unwrap().into(),
            executable: "freshen-reference-app.exe".into(),
            machine_wide: false, enhanced: true, timeout_secs: 1800, relaunch: false,
        }),
        exit_timeout_secs: 120, confirmation_timeout_secs: 120,
    };
    let updater = Updater {
        product: "freshen-reference".into(), channel: "stable".into(),
        current_version: env!("CARGO_PKG_VERSION").parse().unwrap(),
        target: "x86_64-pc-windows-msvc".into(), trust: TrustStore::new(vec![key])?,
        transport: HttpTransport::new(Duration::from_secs(300))?,
        max_download: 512 * 1024 * 1024, max_unpacked: 512 * 1024 * 1024,
    };
    let source = ReleaseSource::Manifest {
        document: args[0].parse().map_err(|_| Error::Invalid("manifest URL".into()))?,
        signature: format!("{}.sig", args[0]).parse()
            .map_err(|_| Error::Invalid("signature URL".into()))?,
    };
    let cancel = Cancellation::default();
    let mut events = |event| println!("{event:?}");
    let Some(candidate) = updater.check_for_kind(&source, policy.package_kind(), &cancel, &mut events)?
        else { println!("No applicable update"); return Ok(()); };
    std::fs::create_dir_all(&policy.root)?;
    let prepared = updater.prepare(candidate, &policy.root, &cancel, &mut events)?;
    let handoff = prepared.arm_current_exe(policy, vec![])?;
    println!("Helper {} ready; host exits normally", handoff.helper_pid());
    Ok(())
}
```

The scaffold first build creates its own Cargo.lock outside this repository:

```powershell
cargo build --release --manifest-path "$work\app\Cargo.toml"
New-Item -ItemType Directory "$work\v1", "$work\v2", "$work\installers" | Out-Null
Copy-Item "$work\app\target\release\freshen-reference-app.exe" "$work\v1\"
Copy-Item "$freshen\inno\FreshenInno.iss" "$work\FreshenInno.iss"
```

Keep the generated publisher private key outside source control. The same public
key must be pinned in both versions. The real application should supply its own
build/version pipeline and user-controlled UI instead of the console bootstrap.

## Complete Inno 7 script

Save the following as `$work\setup.iss`. Only the host EXE is installed; it is
also the helper. Inno embeds/extracts the bridge DLL through the copied include.

```iss
[Setup]
AppId=FreshenReference
AppName=Freshen Reference
AppVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\FreshenReference
PrivilegesRequired=lowest
SetupArchitecture=x64
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=FreshenReference-{#AppVersion}
UninstallDisplayIcon={app}\freshen-reference-app.exe
CloseApplications=no
RestartApplications=no
Compression=lzma2
SolidCompression=yes
[Files]
Source: "{#SourceDir}\freshen-reference-app.exe"; DestDir: "{app}"; Flags: ignoreversion
#include "FreshenInno.iss"
```

Build version 1 and install it interactively:

```powershell
$bridge = "$freshen\target\x86_64-pc-windows-msvc\release\FreshenInno.dll"
& $iscc '/DAppVersion=1.0.0' "/DSourceDir=$work\v1" "/DOutputDir=$work\installers" "/DFreshenInnoDll=$bridge" "$work\setup.iss"
& "$work\installers\FreshenReference-1.0.0.exe"
$installed = "$env:LOCALAPPDATA\Programs\FreshenReference"
& "$installed\freshen-reference-app.exe" --version
$identity = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\FreshenReference_is1'
$before = Get-ItemProperty $identity
$before | Select-Object DisplayVersion, InstallLocation, UninstallString
```

Build and compile version 2 with the same public key and install identity:

```powershell
(Get-Content -Raw "$work\app\Cargo.toml").Replace('version = "1.0.0"','version = "2.0.0"') | Set-Content "$work\app\Cargo.toml"
cargo build --release --locked --manifest-path "$work\app\Cargo.toml"
Copy-Item "$work\app\target\release\freshen-reference-app.exe" "$work\v2\"
& $iscc '/DAppVersion=2.0.0' "/DSourceDir=$work\v2" "/DOutputDir=$work\installers" "/DFreshenInnoDll=$bridge" "$work\setup.iss"
```

For an existing application, keep its established AppId, directory, scope and
**install mode**. A native x64 setup process defaults to 64-bit install mode;
upgrading an Inno 6 app that used 32-bit install mode needs an explicit empty
`ArchitecturesInstallIn64BitMode=` to retain it. An initial migration test caught
a second uninstall log when the mode was accidentally changed. Native x64 DLL
bitness remains required even when preserving 32-bit install mode.

## Publish the authenticated update and trigger it

Use the application's own HTTPS distribution service. Set `$baseUrl` to the
actual release directory you control (without a trailing slash):

```powershell
New-Item -ItemType Directory "$work\payload", "$work\publish" | Out-Null
Copy-Item "$work\installers\FreshenReference-2.0.0.exe" "$work\payload\setup.exe"
& "$freshen\target\release\freshen-release.exe" pack "$work\payload" "$work\publish\update.zip" "$baseUrl/update.zip" x86_64-pc-windows-msvc inno freshen-reference 2.0.0 stable "$work\publish\freshen-manifest.json"
& "$freshen\target\release\freshen-release.exe" sign "$work\publisher.key" "$work\publish\freshen-manifest.json" "$work\publish\freshen-manifest.json.sig"
```

Upload those three files unchanged to `$baseUrl`. Signing is over exact bytes;
add release notes before signing. A GitHub release uses those same manifest and
signature names and the referenced ZIP. The existing Index and Manifests sources
also work. The default transport requires verified HTTPS: no insecure HTTP or
certificate-verification bypass is provided. Upload commands depend on the
chosen hosting service and credentials, which are not part of this checkout.
For combined portable/Inno releases, merge the separately packed target/kind
manifests and sign once as described in [distributions](distributions.md).

Trigger from the installed version 1:

```powershell
$state = "$env:LOCALAPPDATA\FreshenReferenceServicing"
& "$installed\freshen-reference-app.exe" "$baseUrl/freshen-manifest.json" $state
Get-Content "$state\.freshen\transaction\inno-status.json"
Get-Content "$state\.freshen\transaction\journal.json"
Get-Content "$state\.freshen\transaction\inno.log"
```

Wait for the helper's actual terminal result before verifying the installed
executable with `--version` (expect 2.0.0). Expect Committed/exit 0 or the explicit
RebootRequired/3010 outcome; other installer exits are failures. A surviving GUI
polls the same status and presents Installing, Finalizing and completion. The
console scaffold does not pretend it can receive UI callbacks after exiting.

Verify `$identity` is unchanged, DisplayVersion is 2.0.0, InstallLocation and
UninstallString agree with `$before`, and there is no second independent
installation/uninstaller. Preserve diagnostics, close all app/monitor processes
and wait for helper exit, then use `freshen-release discard-finished $state`
to discard a known finished transaction before another update. Unknown outcomes
require inspection/repair with the same Inno identity; no automatic Inno
rollback is claimed.

## Validation and remaining reference-app work

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo test --workspace --doc --locked
cargo package --locked -p freshen
$env:FRESHEN_ISCC = $iscc
$env:FRESHEN_SETUP_ARCH = 'x64'
cargo test --locked --test inno_installer -- --ignored --nocapture
```

Set `FRESHEN_INITIAL_ISCC` to Inno 6's ISCC.exe to exercise cross-version x64
migration retaining the old 32-bit install mode. For Inno 7 x86 set
`FRESHEN_SETUP_ARCH=x86`; for Inno 6 use x86/the default and the i686 DLL.
The fixture automatically selects the matching DLL and reverses the mismatch
test. These backend fixtures are retained; the disposable GUI is not.

The public audio-game reference app still needs its real audio/GUI source,
copy-specific portable/Inno identity, production public-key provisioning,
actual distribution hosting and its own build pipeline. Give it the native
Petzold-style progress presentation in the host guide, test that audio/runtime
dependencies do not prevent helper startup, and keep the human rehearsal under
the user's control. Machine-wide UAC, alternate credentials, crash recovery and
NVDA speech on that real packaged app require separate tests. The earlier GUI
and installer validation record is retained in [validation](inno-validation.md).
