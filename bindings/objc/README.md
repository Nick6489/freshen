# Freshen for Objective-C

An Objective-C binding over the [freshen C ABI](../../ffi/README.md) for macOS
applications. It adds no update logic: signature verification, bounded extraction,
the helper process and rollback all run in the native library. This layer gives
Objective-C hosts ordinary classes, `NSError` reporting, blocks and safe handle
management.

> **Status: written and syntax-checked, not yet built or run on macOS.**
> The sources, tests and scripts were produced on a Linux machine with no Apple SDK.
> They pass a strict ARC/blocks/nullability syntax check against a stub Foundation
> (`Tools/check-syntax.sh`), and the C ABI underneath is exercised by the C, .NET and
> Rust tests. But nobody has compiled this against real Foundation or run its tests.
> Run `Tools/run-tests.sh` on a Mac before relying on it, and treat the first run as
> the real verification. See [What is and is not verified](#what-is-and-is-not-verified).

## Flow

```objc
#import "FreshenObjC.h"

// Run on a worker thread, or use the block-based variants below.
FSHUpdaterConfiguration *configuration =
    [[FSHUpdaterConfiguration alloc] initWithProduct:@"my-application"
                                             channel:@"stable"
                                      currentVersion:@"1.0.0"
                                              target:@"aarch64-apple-darwin"
                                         trustedKeys:@[publisherPublicKey]];  // 32-byte Ed25519 keys, pinned in your app
NSError *error = nil;
FSHUpdater *updater = [[FSHUpdater alloc] initWithConfiguration:configuration error:&error];

FSHReleaseSource *source = [FSHReleaseSource sourceWithIndexURL:indexURL];
FSHCandidate *candidate = nil;
if (![updater checkForUpdateFromSource:source cancellation:nil progress:nil candidate:&candidate error:&error]) {
    // Failed (including "server unreachable"); never means "up to date".
    return;
}
if (candidate == nil) { return; }                        // up to date

for (FSHReleaseNote *note in candidate.history) {         // authenticated notes, oldest first
    [self showNotes:note.notes forVersion:note.version];
}

FSHPreparedUpdate *prepared = [updater prepareCandidate:candidate
                                  temporaryDirectoryURL:tempDirectoryURL
                                           cancellation:cancellation
                                               progress:^(FSHProgress *p) { /* worker thread */ }
                                                  error:&error];

// Later, once the user agrees and you can shut down:
FSHInstallPolicy *policy = [FSHInstallPolicy macBundlePolicyWithParentDirectoryURL:appsURL
                                                                        bundleName:@"MyApp.app"
                                                                        executable:@"MyApp.app/Contents/MacOS/MyApp"
                                                                  bundleIdentifier:@"com.example.myapp"
                                                                    teamIdentifier:@"ABCDE12345"];
FSHHandoff *handoff = [prepared armWithPolicy:policy
                          helperExecutableURL:helperURL
                            relaunchArguments:@[@"--restarted"]
                                        error:&error];
// Save state and quit. If shutdown is declined instead:  [handoff cancelWithError:&error];
```

In the restarted application, once essential initialization has succeeded:

```objc
BOOL confirmed = NO;
[FSHHost confirmStartupForInstallationRootURL:installRootURL confirmed:&confirmed error:&error];  // no-op on an ordinary launch
```

Block-based variants (`checkForUpdateFromSource:…completionQueue:completion:`,
`prepareCandidate:…`, `armWithPolicy:…`) run on a background queue and call back on
`completionQueue`, which defaults to the main queue.

## Things to know

- **Errors.** Failures return `NO`/`nil` with an `NSError` in `FSHErrorDomain`
  (`FSHErrorCodeSignature`, `…Busy`, `…SizeLimit`, `…Recovery`, `…Cancelled`, …). A
  failed fetch or a bad signature is always an error, never "up to date":
  `checkForUpdate…` reports up to date as `YES` with a nil candidate.
- **Threading.** Synchronous methods block. An `FSHUpdater` runs one operation at a
  time and fails a concurrent call with `FSHErrorCodeOperationInProgress`. Progress
  handlers run on the worker thread; hop to the main queue yourself.
- **Ownership.** Objects free their native handles when released. Releasing an
  `FSHPreparedUpdate` deletes its staged files. **Releasing an `FSHHandoff` does not
  cancel a requested installation**; call `cancelWithError:`. `armWithPolicy:` consumes
  the prepared update whether it succeeds or fails; an invalid argument (such as an
  out-of-range timeout) is rejected first and does not consume it.
- **Offline or unreachable servers.** A check that cannot reach the server returns an
  error; treat it as "update server unreachable" and retry later, not as a fault.
- **No insecure switches.** There is no option to skip signature verification or allow
  plain HTTP.

## Custom transports

Implement `FSHTransport` for authenticated feeds, offline storage or tests and set
`FSHUpdaterConfiguration.transport`. Every byte goes through the `FSHDownloadSink`,
which enforces the size limit, cancellation and progress inside the native library, so
a transport cannot skip them. Non-HTTPS URLs and URLs with embedded credentials never
reach it. Objective-C exceptions raised by a transport are caught and reported as an
`FSHErrorCodeIO` failure.

## Integrating

**Static library (recommended).** `Tools/build-xcframework.sh` builds
`CFreshen.xcframework` containing a universal (arm64 + x86_64) static
`libfreshen_ffi.a` and `freshen.h`. It links into your executable, so there is no
extra dylib to embed, sign or list among the files an update owns. Link the
`Security` and `CoreFoundation` frameworks and `iconv` (confirm with
`cargo rustc -p freshen-ffi --release --crate-type staticlib --target aarch64-apple-darwin -- --print native-static-libs`).

**Swift Package.** `Package.swift` exposes the `FreshenObjC` library; run
`Tools/build-xcframework.sh` first, because the package depends on the generated
`CFreshen.xcframework`.

**XcodeGen / Xcode project.** Add `Sources/FreshenObjC` as a source group, add
`Sources/FreshenObjC/include` and the directory containing `freshen.h` to the header
search path, and link the static library and the frameworks above.

## The helper and the installation

- **Helper.** Freshen copies an executable into the transaction and runs it after your
  application quits, so it must be self-contained. Either ship the `freshen-helper`
  binary in your bundle and pass its path to `armWithPolicy:`, or use your own
  executable as the helper by calling `FSHRunHelperIfRequested(argc, argv)` first thing
  in `main`, before any UI or single-instance lock.
- **Bundle layout.** `macBundlePolicy…` replaces a signed bundle in place; it never moves
  it to `/Applications`. The signed archive must include the bundle prefix, and the
  bundle must be signed and notarized before packaging. The parent directory must
  exist and be writable by the current user. App Sandbox and Mac App Store
  installations are not supported.
- **Not covered by automated tests.** The bundle path needs a signed, notarized bundle
  and is checked by hand. The portable layout is what the tests exercise.

## Tests

`Tools/run-tests.sh` (macOS) builds a debug `CFreshen.xcframework`, the helper and
example programs, signed fixtures, and runs `swift test`:

- unit tests over a fixture-serving transport: history and notes, up-to-date versus
  error, forged signatures, plain-HTTP and credential URLs refused, the sink enforcing
  the size limit against a host that writes past it, mid-download cancellation
  leaving nothing behind, progress events, single-operation enforcement, option
  validation, and arming consuming the prepared update;
- end-to-end tests that run a parent application, the native helper and a restarted
  native application as real processes: commit, cancel, failed startup rollback,
  unconfirmed timeout, and a busy installation lock.

These mirror the Rust, C and .NET tests.

## What is and is not verified

Verified on Linux, where this was written:

- `Tools/check-syntax.sh` compiles every source and test file with `-fobjc-arc
  -fblocks -Wall -Wextra -Werror -Wconversion -Wnullable-to-nonnull-conversion` and
  designated-initializer checks against a stub Foundation and XCTest. It caught real
  issues while writing (nullable `NSURL` results passed to nonnull parameters, and
  `uint64_t` boxing that differs between platforms), and it rejects a deliberately
  broken file (an ARC bridging error).
- Every C ABI call this layer makes is covered by the Rust, C and .NET suites, including
  the size-limit, cancellation, consume-on-arm and helper scenarios.

Not verified, because it needs a Mac:

- compilation against real Foundation and libdispatch, and any use of Foundation
  that the stub cannot check (selectors, runtime behavior);
- the SwiftPM manifest, the XCFramework build and the system libraries the static
  library needs;
- running the tests, and everything about a real signed, notarized bundle update.
