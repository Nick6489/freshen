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

## Deploying the native library and helper

- **Native library.** Put `freshen_ffi.dll` / `libfreshen_ffi.so` /
  `libfreshen_ffi.dylib` where the runtime probes (beside the application, or
  `runtimes/{rid}/native`). Set `FRESHEN_FFI_LIBRARY` to load a specific file. The
  binding checks the library's C ABI version at load and refuses a mismatch.
- **Helper.** Freshen copies an executable into the transaction and runs it while
  your application is gone, so it must be self-contained. Ship the
  `freshen-helper` binary for each target beside your application and pass its
  path to `Arm`. A .NET apphost is not a suitable helper on its own because it
  needs its managed assemblies. `FreshenHost.RunHelper(args)` exists for an
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

Requires the .NET 8 SDK and a Rust toolchain. Tests currently run on Linux only;
Windows and macOS are untested.
