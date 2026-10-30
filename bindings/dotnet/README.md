# Freshen for .NET

A .NET 8 binding over the [freshen C ABI](../../ffi/README.md). It adds no update
logic: signature verification, bounded extraction, the helper process and rollback
all run in the native library. This layer gives .NET hosts idiomatic types,
`async` methods, `CancellationToken` and `IProgress<T>` support, and safe handle
management.

It uses source-generated `LibraryImport` P/Invokes and `UnmanagedCallersOnly`
callbacks, so it is trim- and Native AOT-compatible.

## Flow

```csharp
using Freshen;

// Run on a worker thread (or use the *Async methods).
using var updater = new Updater(new UpdaterOptions
{
    Product = "my-application",
    Channel = "stable",
    CurrentVersion = "1.0.0",
    Target = "x86_64-pc-windows-msvc",
    TrustedKeys = new[] { publisherPublicKey }, // 32-byte Ed25519 keys, pinned in your app
});

var source = ReleaseSource.Index(new Uri("https://updates.example.com/index.json"));
using Candidate? candidate = await updater.CheckAsync(source, progress, token);
if (candidate is null) return;                       // up to date

foreach (ReleaseNote note in candidate.History)     // authenticated notes, oldest first
    Show(note.Version, note.Notes);

using PreparedUpdate prepared = await updater.PrepareAsync(candidate, tempDirectory, progress, token);

// Later, once the user agrees and you can shut down:
var policy = InstallPolicy.Portable(installDir, "MyApp.exe", new[] { "MyApp.exe", "Freshen.dll" });
using Handoff handoff = await prepared.ArmAsync(policy, helperExecutablePath, new[] { "--restarted" });
// Save state and exit. If shutdown is declined instead:  handoff.Cancel();
```

In the restarted application, once essential initialization has succeeded:

```csharp
FreshenHost.ConfirmStartup(installDir);   // a no-op on an ordinary launch
```

## Things to know

- **Errors.** Failures throw `FreshenException` with a `FreshenErrorCode`
  (`Signature`, `Busy`, `SizeLimit`, `Recovery`, ...). Cancellation throws
  `OperationCanceledException`. A failed fetch or a bad signature is always an
  error, never "up to date": `Check` returns `null` only when the application is
  current.
- **Threading.** Operations block. An `Updater` runs one operation at a time and
  throws `InvalidOperationException` on overlap. `IProgress<T>` reports arrive on
  the worker thread; `Progress<T>` marshals them to the UI context.
- **Ownership.** `Candidate`, `PreparedUpdate`, `Handoff` and `Updater` are
  `IDisposable`. Disposing a `PreparedUpdate` deletes its staged files. **Disposing
  a `Handoff` does not cancel a requested installation**; call `Cancel()`.
  `Arm` consumes the `PreparedUpdate` whether it succeeds or fails.
- **Offline or unreachable servers.** A check that cannot reach the server throws;
  treat that as "update server unreachable" and retry later, not as a fault.
- **No insecure switches.** There is no option to skip signature verification or
  allow plain HTTP.

## Custom transports

Implement `IUpdateTransport` for authenticated feeds, offline storage or tests and
set `UpdaterOptions.Transport`. Every byte goes through the `DownloadSink`, which
enforces the size limit, cancellation and progress inside the native library, so a
transport cannot skip them. Non-HTTPS URLs and URLs with embedded credentials never
reach it.

## The NuGet package

The `Freshen` package (`0.1.0-preview.1`, not published anywhere) carries the assembly,
its XML documentation, and, for every platform it was built for, the native library
**and** the helper under `runtimes/{rid}/native/`:

```text
lib/net8.0/Freshen.dll  Freshen.xml
runtimes/win-x64/native/    freshen_ffi.dll      freshen-helper.exe
runtimes/linux-x64/native/  libfreshen_ffi.so    freshen-helper
runtimes/osx-arm64/native/  libfreshen_ffi.dylib freshen-helper      (and linux-arm64, osx-x64)
```

Reference it and nothing else is needed: the runtime finds the native library on its
own (no `FRESHEN_FFI_LIBRARY`), and `FreshenHost.FindBundledHelper()` returns the
packaged helper, whether the build kept the `runtimes/` layout or a RID-specific
publish flattened it beside the application.

```csharp
string helper = FreshenHost.FindBundledHelper()
    ?? throw new InvalidOperationException("The Freshen helper is not deployed.");
using Handoff handoff = prepared.Arm(policy, helper, new[] { "--restarted" });
```

NuGet does not preserve the execute permission, so a packaged helper arrives
non-executable on Linux and macOS. `FindBundledHelper` restores it, or, if the install
location is read-only, returns an executable copy from a private temporary directory.

**Building the package.** The native pieces come from Rust, so packing is two steps
(`Tools/pack.sh` does both):

```text
Tools/stage-native.sh   # cargo build --release, then copy this machine's library + helper
                        # into bindings/dotnet/runtimes/<rid>/native/
dotnet pack src/Freshen -c Release -o artifacts
```

A complete package needs every platform, and one machine can only build its own, so
`.github/workflows/nuget.yml` (run by hand) builds each platform, gathers them, packs
with `-p:FreshenRequiredRuntimes="linux-x64;linux-arm64;win-x64;osx-arm64;osx-x64"`, and
verifies the result. Packing with **no** natives, or with a required platform missing,
fails rather than producing a package that cannot load. `Tools/verify-package.sh` then
inspects the archive and consumes it from a scratch application that references it from
a local-only feed, checking that the native library loads and the helper is found.
Build the Linux libraries on the oldest OS you support, since a library built on a very
recent distribution needs a newer glibc.

Only the Linux x64 package has been built and verified so far. Nothing is published to
nuget.org; do that deliberately, and rename the package if `Freshen` is not the ID you
want.

## Deploying without the package

- **Native library.** Put `freshen_ffi.dll` / `libfreshen_ffi.so` /
  `libfreshen_ffi.dylib` where the runtime probes (beside the application, or
  `runtimes/{rid}/native`). Set `FRESHEN_FFI_LIBRARY` to load a specific file. The
  binding checks the library's C ABI version at load and refuses a mismatch.
- **Helper.** Freshen copies an executable into the transaction and runs it while
  your application is gone, so it must be self-contained. Ship the
  `freshen-helper` binary for each target beside your application and pass its
  path to `Arm` (the package does this for you). A .NET apphost is not a suitable
  helper on its own because it needs its managed assemblies.
  `FreshenHost.RunHelper(args)` exists for an
  application that is a single self-contained executable (single-file or Native
  AOT publish): call it first in `Main`, before any UI or single-instance lock.
- **Owned files.** The `InstallPolicy` must list every file your application owns.
  The update package must contain exactly those files (plus anything under an
  expandable directory you name). Include the native library in that list.
- **macOS.** `InstallPolicy.MacBundle` replaces a signed bundle in place. The signed
  archive must include the bundle prefix, and the bundle must be signed and
  notarized before packaging. This path is not covered by automated tests.

## Tests

`dotnet test bindings/dotnet/Freshen.sln` builds the native library, helper and
example programs with cargo, generates signed fixtures, and runs:

- unit tests over a fixture-serving transport: history and notes, up-to-date versus
  error, forged signatures, plain-HTTP and credential URLs refused, the sink
  enforcing the size limit against a host that writes past it, mid-download
  cancellation leaving nothing behind, progress events, single-operation
  enforcement, dispose semantics, and `Arm` consuming the prepared update;
- end-to-end tests that run a managed parent application, the native helper and a
  restarted native application as real processes: commit, cancel, failed startup
  rollback, unconfirmed timeout, and a busy installation lock.

A Native AOT publish of the end-to-end parent (`-r linux-x64 -p:PublishAot=true`)
was also run against a real update and installed it.

Requires the .NET 8 SDK and a Rust toolchain. Linux and Windows CI have passed the
suite and single-platform NuGet pack/consume checks. macOS .NET integration is not
covered by this CI matrix. The earlier Native AOT result above is a reported Linux
exercise, not an ongoing CI guarantee for every runtime identifier.

Before shipping, exercise your actual publish layout, native library/helper
deployment, UI dispatch, shutdown and startup confirmation on each target. The
manual multi-platform NuGet workflow is separate from the single-platform CI
checks. See [testing and integration](../../TESTING.md) for exact coverage and the
application acceptance checklist.
