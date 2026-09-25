# Freshen

[![Rust checks](https://github.com/Nick6489/freshen/actions/workflows/ci.yml/badge.svg)](https://github.com/Nick6489/freshen/actions/workflows/ci.yml)

Freshen is a Rust library for authenticated desktop application updates **without a GUI of its own**. It works with any toolkit that can call Rust code on a worker thread. Applications control scheduling, prompts, unsaved work, shutdown, and error presentation.

This is an initial implementation, not yet published to crates.io. Review the [support boundaries](#support-boundaries) before integrating it into a shipped application.

## What it does

- Discovers signed releases from GitHub or manifest/signature URLs.
- Authenticates exact manifest bytes with pinned Ed25519 public keys. Signed metadata binds the product, channel, semantic version, target, package hash, and every file's hash and size.
- Downloads over HTTPS with limits, timeouts, cancellation, and structured progress events.
- Extracts only declared regular files, rejecting traversal, case collisions, reserved paths, symlinks, reparse points, and unexpected entries.
- Prepares the complete update while the application stays running.
- Hands installation to a copied helper and waits for readiness before the host exits.
- Replaces only locally declared program files on Windows/Linux, or a verified application bundle on macOS.
- Journals replacement intent, retains backups, restores failed updates, and requires the new executable to acknowledge successful startup.

Freshen never opens a dialog, browser, or console window for an update. It does not call `process::exit` inside the library, kill a host application, acquire administrator privileges, or choose when the user should install.

## Integration

Build and ship `freshen-helper` for your application's target, or use the application executable itself as the helper by calling `run_helper` before normal startup. A separate helper must be deployed with all runtime dependencies it needs; a self-contained Rust executable is simplest.

```rust,no_run
use freshen::{Cancellation, Event, HttpTransport, ReleaseSource, TrustStore, Updater};
use std::{path::Path, time::Duration};

fn prepare_update(
    publisher_key: [u8; 32],
    temporary_parent: &Path,
    events: &mut dyn FnMut(Event),
) -> freshen::Result<Option<freshen::PreparedUpdate>> {
    let updater = Updater {
        product: "my-application".into(),
        channel: "stable".into(),
        current_version: "1.0.0".parse().unwrap(),
        target: "x86_64-pc-windows-msvc".into(), // your distribution target
        trust: TrustStore::new(vec![publisher_key])?,
        transport: HttpTransport::new(Duration::from_secs(300))?,
        max_download: 256 * 1024 * 1024,
        max_unpacked: 1024 * 1024 * 1024,
    };
    let source = ReleaseSource::GitHub {
        owner: "your-account".into(),
        repository: "your-application".into(),
    };
    let cancellation = Cancellation::default();
    // Run on a worker thread. Send events through your GUI's dispatcher/channel.
    match updater.check(&source, &cancellation, events)? {
        Some(candidate) => updater.prepare(candidate, temporary_parent, &cancellation, events).map(Some),
        None => Ok(None),
    }
}
```

`Candidate::release()` exposes the version and release notes for your interface. Holding a `PreparedUpdate` lets the user defer installation; dropping it deletes its temporary files. Clone a `Cancellation` before dispatching work to cancel it from another thread. Cancellation during a blocked network read is bounded by the request timeout.

After the application decides it can shut down:

```rust,no_run
use freshen::{Handoff, InstallPolicy, PreparedUpdate};
use std::path::Path;

fn arm_update(prepared: PreparedUpdate, installation: &Path, helper: &Path)
    -> freshen::Result<Handoff>
{
    let policy = InstallPolicy::portable(
        installation.to_owned(),
        "my-application.exe".into(),
        vec!["my-application.exe".into(), "Manual.html".into()],
    );
    prepared.arm(policy, helper, vec![])
}
```

Once `arm` returns, the helper holds the update lock and waits for this process to exit. Save state and quit through your toolkit. If shutdown is declined, call `handoff.cancel()`. Dropping the handle does **not** cancel an armed update. Use a worker thread for the readiness wait too.

The restarted executable receives confirmation information in environment variables. Once its essential initialization succeeds, call `freshen::confirm_startup(installation_root)`. This is a no-op for an ordinary launch. Freshen checks the executable path and its signed hash before accepting confirmation.

For a self-helper application, dispatch this before GUI initialization and before any single-instance lock:

```rust,no_run
fn main() -> freshen::Result<()> {
    if freshen::run_helper(std::env::args_os().skip(1))? {
        return Ok(());
    }
    // Initialize the application, then confirm_startup(installation_root).
    // Enter the toolkit's event loop here.
    Ok(())
}
```

## Installation and recovery

The installation directory must already exist and be writable by the current user. Freshen stores its lock and transaction beneath `.freshen` in that directory. Do not include `.freshen` in your package or owned-file list.

The durable state sequence is Prepared, Applying, AwaitingConfirmation, then Committed. A replacement failure enters RollingBack and ends at RolledBack if restoration succeeds. Recovery failures remain visible rather than reporting success. Files absent before installation are removed during rollback; existing files are restored.

The new process must confirm startup within the deadline. If it exits without confirmation, the helper rolls back. If it stays running without confirmation, Freshen records Failed and keeps recovery material: it does not overwrite or kill that process. After rollback the old version is available for a manual restart; Freshen does not launch it automatically.

Use `installation_status` to display results. Once all application processes are stopped, `recover_installation` resumes a pending rollback; run it from a separate maintenance process when the target executable may be locked. `freshen-release recover ROOT` provides a command-line recovery entry point. On Committed or RolledBack, `discard_finished_installation` removes the transaction and its backups. It also accepts a cancelled Prepared transaction. An active helper returns `Error::Busy`; retry later. Backups remain until explicitly discarded.

This is journaled recovery, not an atomic switch of every file at once. Atomic file replacement and sync calls reduce partial-write risks, but power-loss durability ultimately depends on the filesystem. Recover before resuming a partially updated installation.

## macOS

Use `InstallLayout::MacBundle` with an explicit existing parent directory, bundle name, relative executable path, bundle identifier, and Team ID. Freshen does not silently relocate an application into `/Applications`.

The signed archive must include the bundle prefix, for example `MyApp.app/Contents/MacOS/MyApp`. Freshen checks the bundle using `codesign` against the configured Apple signing identity and uses `lipo` to check the helper's native architecture. Sign, notarize, and staple your app **before packaging it**. Notarization is a release-pipeline responsibility; Freshen does not submit apps to Apple or bypass Gatekeeper.

The old bundle is backed up and renamed aside on the same filesystem before the prepared bundle takes its place. Relaunch executes the bundle's declared executable directly so startup confirmation information reaches the new process.

## Publishing application updates

For an opt-in installation synchronized across machines, `shared::publish` writes the authenticated release metadata and `shared::inspect` reports Unchanged, WaitingForFiles, or ReadyToRestart. It checks every declared file hash, so a small metadata file arriving before the application does not trigger an early restart. Your application owns watching, polling, standing down, and relaunching. Freshen does not issue unauthenticated cross-machine shutdown requests or delete sync-conflict files.

Build the tools with `cargo build --release --bins`. The release tool provides:

```text
freshen-release keygen PRIVATE_KEY PUBLIC_KEY
freshen-release pack DIRECTORY ZIP URL TARGET files|mac_bundle PRODUCT VERSION CHANNEL MANIFEST
freshen-release merge OUTPUT_MANIFEST INPUT_MANIFEST...
freshen-release sign PRIVATE_KEY MANIFEST SIGNATURE
```

Keep the private key outside the repository and embed the decoded 32-byte public key in your application. On Unix, key generation creates the private file with mode 0600; on Windows use a private directory with suitable ACLs. The tool never prints private key material and refuses to overwrite existing outputs.

An example single-target release:

```text
freshen-release pack dist MyApp-windows.zip https://github.com/your-account/your-application/releases/download/v1.1.0/MyApp-windows.zip x86_64-pc-windows-msvc files my-application 1.1.0 stable freshen-manifest.json
freshen-release sign /private/location/publisher.key freshen-manifest.json freshen-manifest.json.sig
```

For multiple targets, generate separate manifests, merge them, then sign the combined manifest once. Add release notes before signing. Publish `freshen-manifest.json`, `freshen-manifest.json.sig`, and referenced ZIPs on the GitHub release. Signing uses Ed25519 over `freshen-manifest-v1\0` followed by the exact manifest bytes; changing whitespace after signing invalidates the signature.

GitHub discovery examines the first 100 releases. Drafts are ignored; stable-channel discovery also ignores prereleases. Selection uses the signed product, channel, semantic version, and target, not the GitHub tag. Releases lacking the manifest/signature pair are skipped. An invalid signature or failed fetch is an error, not an "up to date" result. Only versions newer than the configured current version qualify.

For other hosting, use `ReleaseSource::Manifest`. A custom `Transport` supports application-specific authentication or offline storage. TLS certificate verification uses the platform trust store without changing the process-wide rustls crypto provider.

## Support boundaries

- Intended for application-managed Windows/Linux portable directories and regular-file macOS bundles. Package-manager, MSIX, App Store, sandbox entitlement, service, and elevated installations need their own integration and are not implemented.
- Schema 1 accepts ASCII paths and regular files only. Bundles with framework symlinks and archives containing resource-fork sidecars are rejected. Package on the destination OS to preserve executable permissions. Additional archive entry types require a schema/backend extension.
- Portable inventories match the application's local owned-file list exactly by default. With `InstallPolicy::portable_expanding`, additional signed files are installed only under directories the running application explicitly passed in (including nested subdirectories). The manifest cannot introduce files or directories outside that local policy or remove an owned path; remote metadata cannot expand ownership by itself.
- This is not a full TUF implementation: there is no expiration, transparency log, or protection against withholding newer releases. Rotate keys by shipping old and new public keys together before switching signers.
- Installation directories, helper binaries, staging, and recovery metadata must be protected from other users. Freshen does not defend against a malicious process with the same filesystem privileges or elevate downloaded code.
- The GUI owns scheduling and must serialize update requests. Disk-space failures are reported and backups retained; Freshen does not delete unrelated files to make room.

## Development

```text
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
cargo test --doc --locked
cargo package --locked
```

CI runs these checks on Windows, Linux, and macOS. Tests cover authentication, archive rejection, preservation, rollback, interrupted-state recovery, and actual helper processes, including cancellation, startup failure, and confirmation timeout. A successful Developer ID-signed macOS bundle update additionally requires a real signed release and credentials unavailable in generic CI.

Every implementation increment is committed with an explanation of what changed and why. Publishing to crates.io is a separate release decision.

## Acknowledgments

The design draws on Andre Louis's [ElevenLabs Music Generator updater](https://github.com/OnjLouis/ElevenLabsMusicGenerator) and [Clipman updater](https://github.com/OnjLouis/Clipman). Freshen is an independent Rust implementation under the [MIT license](LICENSE).
