# Inno implementation inventory and validation

## Architectural result

Signed Inno ZIP payloads use the existing manifest verification, download,
bounded extraction, stage hashing, explicit local ownership and copied-helper
readiness protocol. The helper waits for application exit, verifies existing
AppId/directory/scope registration, runs Inno directly and records its actual
exit. No file-replacement rollback or startup-confirmation promise is applied
to Inno. Optional authenticated local named-pipe telemetry supplies actual
installation counters, with indeterminate fallback on bridge failure.

## Source inventory

New files:

- `src/inno.rs`: policy, registry preflight, snapshots, phase/progress validation,
  direct process/elevated process supervision, logs and reboot outcomes.
- `src/inno_pipe.rs`: shared secured, bounded, nonblocking native pipe transport.
- `src/inno_wire.rs`: shared versioned fixed-size protocol.
- `inno-bridge/Cargo.toml`, `inno-bridge/src/lib.rs`: repository-only
  `freshen-inno-bridge` cdylib producing `FreshenInno.dll`.
- `inno/FreshenInno.iss`: optional attributed Pascal handlers and embedded DLL.
- `inno/build-bridge.ps1`: x86/x64 static-CRT release builds.
- `tests/inno_installer.rs`: real compiled Inno installer/helper matrix.
- `docs/inno.md`: architecture, security, ABI, switches and troubleshooting.
- `docs/reference-app-inno.md`: clean-checkout rebuild/distribution procedure.
- `docs/inno-validation.md`: this inventory and validation record.
- `docs/distributions.md`: copy-specific portable/Inno selection and compatibility.
- `docs/host-integration.md`: self-helper startup, dependency requirements and
  native presentation resource patterns.

The disposable GUI app, its UI test crate and earlier console/example installer
sources were removed at the user's request. Documentation retains complete
integration/installer source and the native UI pattern. The original GUI test
result is historical evidence, not part of current automated coverage. Generated
local installers, test credentials and installed demo state are not committed.

Modified existing files:

- `Cargo.toml`, `Cargo.lock`: workspace DLL target and Windows API features.
- `src/lib.rs`: public Inno module.
- `src/manifest.rs`: signed `inno` kind and exact one-EXE inventory requirement.
- `src/install.rs`: local Inno layout, preflight, reboot state and separate
  recovery/discard rules.
- `src/helper.rs`: dispatch after the existing parent-exit wait.
- `src/updater.rs`: explicit target/kind discovery and exact selected-artifact
  reauthentication; legacy ambiguity fails closed.
- `ffi/src/updater.rs`, `ffi/include/freshen.h`: additive C selection API without
  changing struct/handle layouts or existing function signatures.
- `src/bin/release.rs`: `pack ... inno ...` support and help text.
- `ffi/src/install.rs`: preserve old ABI by mapping the new reboot state to
  UNKNOWN; existing binding policy structs do not expose Inno arming yet.
- `.github/workflows/ci.yml`: build both DLLs and run the x86 native ABI tests
  on the Windows job.
- `README.md`, `TESTING.md`: integration links, corrected support boundaries
  and opt-in real-installer testing instructions.

## New public configuration

`Updater::check_for_kind`, `InstallPolicy::package_kind()` and
`PreparedUpdate::arm_current_exe` make copy-specific selection and preferred
self-helper integration explicit. Signed artifacts are unique by `(target,kind)`.
Helper/recovery/startup confirmation resolve that same selected kind from local
policy. The C API adds `freshen_check_for_kind` and package-kind constants.
Separate-helper deployment remains fully supported. See the distribution and
host guides for marker/detection responsibilities and startup dependencies.

`PackageKind::Inno` serializes as `"inno"` within manifest schema 1.
`InstallLayout::Inno(InnoPolicy)` extends existing `PreparedUpdate::arm`.
`InnoPolicy` pins installer path, existing AppId, directory, installed executable,
privilege scope, optional telemetry, overdue interval and optional relaunch.
`InstallState::RebootRequired` records a successful installer requiring reboot.
`inno::status`, `Status`, `Phase` and `exit_description` expose durable outcomes,
actual work counters, telemetry evidence and numeric exit diagnostics.

DLL ABI: `FreshenInnoConnect`, `FreshenInnoPhase`, `FreshenInnoProgress`,
`FreshenInnoDiagnostic`, `FreshenInnoDisconnect`. The complete parameter,
calling-convention, lifetime and return-value contract is in `inno.md`.

## Commands actually executed successfully

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo test --workspace --doc --locked
cargo test -p freshen --lib inno --locked
.\inno\build-bridge.ps1
cargo test -p freshen-inno-bridge --locked
cargo test -p freshen-inno-bridge --target i686-pc-windows-msvc --locked
cargo package --locked --offline -p freshen --allow-dirty
```

Both x86 and x64 DLLs were built with `-C target-feature=+crt-static`. The core
crate tarball was verified by compiling its packaged source. Networked packaging
could not reach crates.io from the sandbox; offline packaging succeeded using
the existing cache. Cargo reported the existing locked `yoke-derive 0.8.3` as
yanked; no unrelated dependency version changes were made.

Inno Setup 6.7.3 was installed with the user's explicit winget authorization.
With `FRESHEN_ISCC` pointing to that installation, this passed:

```powershell
cargo test --locked --test inno_installer -- --ignored --nocapture
```

The real matrix covers initial installation and updating existing registration,
basic mode without a DLL, enhanced mode with observed actual counters, failure
exit 7, reboot-required exit 3010, x64-DLL-in-Inno-6 mismatch fallback, deliberate
IPC disconnection, a delayed installer exceeding its overdue interval without
being killed, and a destination file held open resulting in non-success.
It runs the real authenticated ZIP/staging/copied-helper lifecycle. The normal
workspace run skips this opt-in test, so it was invoked separately.

Protocol and native DLL tests exercised malformed/truncated/unsupported frames,
counter validation, token mismatch, phase ordering, real pipe connection,
bounded writes with an unread/saturated pipe, disconnects, exit authority,
argument quoting and subprocess lifetime/launch failures. Native DLL tests
ran in both 64-bit and 32-bit processes.

Release builds of the harness at 1.0.0 and 2.0.0 succeeded, and Inno compiled
both example installers. On retry after a computer-use approval timeout, the
user completed the version-1 native wizard manually. The installed executable
reported 1.0.0 and its expected HKCU uninstall identity/directory were verified.
Automated capture of the wizard controls failed; this is a manual interactive
test. The optional loopback HTTPS demonstration was documented but not run.

## Remaining deployment validation and reference-app work

### Inno Setup 7 follow-up

Inno Setup 7.1.1 (x64 compiler) was installed alongside version 6 with winget.
The real fixture matrix passed separately with:

| Compiler | Installer process | Initial install | Install mode |
| --- | --- | --- | --- |
| Inno 7.1.1 | x86 | Inno 7 x86 | Default 32-bit |
| Inno 7.1.1 | x64 | Inno 7 x64 | Default 64-bit |
| Inno 7.1.1 | x64 | Inno 6.7.3 x86 | Explicitly retain 32-bit |

Each run covered the complete basic/enhanced/failure/reboot/mismatch/disconnect/
overdue/locked-file matrix. The native-x64 runs used the x64 bridge and tested
the x86 bridge as the incompatible DLL. The enhanced case observed real
progress. Single-uninstaller identity assertions passed after every successful
update, including cross-version/compiler migration. Both x86 and x64 versions
of the actual example installer also compiled using Inno 7.

The first migration attempt accepted Inno 7's new default 64-bit install mode
and produced `unins001.exe`. This was a useful failing identity test. The example
now explicitly retains its original 32-bit install mode when selecting a native
x64 setup process, and the corrected migration matrix passed. DLL/process
bitness and install mode are independent. Applications already using 64-bit
install mode must retain that mode instead.

New example build macro: `FreshenSetupArchitecture=x86|x64` (Inno 7 only).
New opt-in test settings: `FRESHEN_SETUP_ARCH=x86|x64` and optional
`FRESHEN_INITIAL_ISCC` (the compiler for an Inno 6/x86 initial installation).
Exact commands are in `reference-app-inno.md`. No core servicing API or bridge
ABI changes were necessary. Formatting, strict Clippy, workspace tests,
documentation tests and offline package verification were rerun.

### Remaining checks

The final mixed-distribution regression tests passed selection in both orders,
per-kind history, absent kinds, duplicate pair rejection, ambiguity across
manifests, exact candidate/stage validation, recovery, real helper confirmation,
the new C boundary, and signed release-tool merging. The real Inno 7 native-x64
migration matrix also passed with a portable artifact first in each manifest;
it selected Inno explicitly and completed all eight installer cases. Historical
GUI evidence is preserved in `host-integration.md`; the demo source is removed.

The external reference GUI is not in this repository and was not modified.
Its next version needs to adopt the Rust policy/status API (or extend its
language binding), integrate the include with its own established AppId, build
the correct installer-process DLL, provide a surviving progress UI if needed,
and publish a signed `inno` artifact through its existing distribution service.
The complete steps are in `reference-app-inno.md`.

Manual interactive Inno 7, machine-wide UAC and alternate credentials,
reference GUI/NVDA, OS/helper crash recovery and broader files-in-use deployment
cases remain unexecuted. The listed real matrices cover per-user Inno 6 and 7.
No rollback, safe mid-install cancellation, automatic restart, startup-health
confirmation for Inno, crates.io publication or production release is claimed.
