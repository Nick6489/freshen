# Portable and installed copies of the same product

Freshen does not infer installation ownership from a downloaded package name,
the current directory, or the mere existence of a product's uninstall entry.
The host establishes the policy for **the copy that is running** and selects its
package kind before discovery. Installed and portable copies can coexist, use
the same publisher keys/channel/architecture and read the same signed release.

## Host policy

`InstallPolicy::package_kind()` maps explicit local layouts to:

| Layout | Signed artifact kind | Installation owner |
| --- | --- | --- |
| `InstallLayout::Portable` | `files` | Freshen, bounded by the locally owned inventory |
| `InstallLayout::Inno` | `inno` | Inno, using the existing AppId/directory/scope/install mode |
| `InstallLayout::MacBundle` | `mac_bundle` | Freshen, with the existing bundle identity checks |

Call `Updater::check_for_kind(source, policy.package_kind(), cancel, events)`.
Only releases containing that target/kind contribute candidates or release
history. A newer installer-only release does not replace a portable candidate
or appear as applicable portable history. No matching kind means no applicable
update; bad signatures, failed fetches and other verification failures remain
errors. There is no fallback from one kind to another.

For applications shipping the same executable in both forms, an installer-owned
distribution marker and a separately packaged portable marker are reasonable
host conventions. Establish the mode from locally deployed identity, then
validate its consistency with the running executable's canonical installation
root. An Inno mode must also match the expected AppId, registry scope and
registered directory; Freshen performs those registration checks at arming and
again in the helper. A portable copy elsewhere must not become an Inno copy
just because an installed copy of the same product exists.

A marker is not publisher authentication and must not independently authorize
elevation or ownership of arbitrary files. Missing/conflicting markers, moved
installed copies and contradictory registration should produce a host error
or explicit setup flow, not an automatic backend guess. In particular, do not
silently treat failed Inno registration validation as portable mode. Derive the
application root from the running executable, not its working directory. The
library supplies policy/verification primitives, not a universal marker-file
format or automatic installation detector.

## One signed release, multiple kinds

Manifest schema 1 is retained. Artifact uniqueness is now the pair `(target,
kind)`, instead of target alone. Publish one `files` ZIP and one `inno` ZIP for
the same Rust target triple; the latter contains exactly one declared setup EXE.
Every artifact retains its own URL, size, SHA-256 and complete file inventory.
Duplicate target/kind pairs are rejected. Kind is signed metadata, not a URL
suffix or unsigned discovery hint. Product/channel/version/target policy still
applies normally; installation mode is independent of stable/beta channels.

Create each artifact with the actual release tool, merge before signing:

```powershell
cargo build --release --locked --bin freshen-release
target\release\freshen-release.exe pack target\portable-dist target\portable.zip https://updates.example.com/portable.zip x86_64-pc-windows-msvc files my-app 2.0.0 stable target\portable.json
target\release\freshen-release.exe pack target\installer-dist target\installer.zip https://updates.example.com/installer.zip x86_64-pc-windows-msvc inno my-app 2.0.0 stable target\inno.json
target\release\freshen-release.exe merge target\freshen-manifest.json target\portable.json target\inno.json
target\release\freshen-release.exe sign C:\Signing\publisher.key target\freshen-manifest.json target\freshen-manifest.json.sig
```

Here `target\portable-dist` contains only the app-owned portable files, and
`target\installer-dist` contains only `setup.exe`. Replace the publisher's host,
product and private-key path with your deployment settings; no distribution
service or signing credential is created by these commands. Input notes must
agree when merging; set notes before signing. Tools refuse to overwrite files.

Preparation reauthenticates the exact chosen target/kind artifact, including its
URL and inventory. Arming enforces the local layout against that kind. The
durable helper plan already contains this local layout, so stage validation,
installation, rollback and startup confirmation resolve the artifact by target
**and** the layout's required kind. Manifest ordering cannot change the backend.
Existing helper plans do not require a new field or format migration.

Shared-folder convergence uses `shared::inspect_for_kind(..., (target, kind))`
for a mixed manifest. Legacy `inspect` refuses ambiguity. Only file/bundle
inventories can establish installed-file convergence: an Inno artifact lists
the setup payload, not the installed application's files, so neither shared
inspection API accepts it as proof that the application is ready to restart.

## Compatibility and bindings

`Updater` struct fields are unchanged. Existing `check` keeps working with
single-kind feeds. It rejects multiple kinds for the target, including kinds
encountered across applicable manifests in an index, rather than selecting the
first/highest release of an unintended distribution. Use `check_for_kind` for
mixed distributions. Older Freshen versions reject mixed-target-kind manifests
because they allowed only one target entry: keep their old feed available
until those clients have migrated. Do not serve a mixed feed to legacy clients
and claim transparent compatibility.

The C ABI adds `freshen_check_for_kind` and `FRESHEN_PACKAGE_AUTO` (0), FILES (1),
MAC_BUNDLE (2), INNO (3). AUTO delegates to legacy ambiguity checks. Existing
`freshen_check`, structs, handles and ABI version remain unchanged. New callers
must deploy a library that exports the additive function. The generated C
header documents its pointer/status contract. The current high-level C++, .NET
and Objective-C wrappers still call the legacy check; use separate feeds or
extend those wrappers to expose explicit kind selection. Inno arming is still
exposed through the Rust policy API, not their existing installation structs.

## Verification

Tests exercise both manifest orders, duplicate pairs, missing kinds, per-kind
history, ambiguity across releases, modified candidate metadata, mixed staging
and recovery, real helper startup confirmation/rollback, the C boundary and
publisher pack/merge/sign tools. The opt-in real Inno matrix includes a portable
artifact first in each signed manifest and selects the Inno kind explicitly.
