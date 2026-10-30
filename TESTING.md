# Testing and application integration

Inno servicing has a separate [architecture, ABI and test guide](docs/inno.md)
and [reproducible reference-app rebuild procedure](docs/reference-app-inno.md).
Run `inno/build-bridge.ps1`, set `FRESHEN_ISCC` to Inno 6's `ISCC.exe`, then run
`cargo test --locked --test inno_installer -- --ignored --nocapture` for real
per-user installer fixtures. Normal workspace tests include protocol, native
pipe/DLL and status tests; the real installer suite is opt-in because it creates
temporary installations and uninstall registry entries. Inno success means
installer process success, with no Freshen startup-confirmation rollback.

Freshen has automated library, binding, and real-process tests. You do not need a
GUI application to test signatures, archive limits, ownership rules, or helper
rollback. You do need a disposable copy of your packaged application to establish
that its deployment and lifecycle work with Freshen.

## Automated coverage without your application

The following describes the workflows merged with [PR #1](https://github.com/Nick6489/freshen/pull/1).
All eight jobs passed in the [audited PR run](https://github.com/Nick6489/freshen/actions/runs/40338584148)
at commit `9408e2b`. That run is evidence for that revision, not a promise that
future runs or every shipping configuration pass.

| Component | Automated coverage | Limits |
| --- | --- | --- |
| Rust core | Windows, Linux and macOS: formatting, strict Clippy, unit/process tests, documentation tests and packaging of `freshen` | Fixture applications, not your packaged application; no successful Developer ID-signed bundle exercise |
| C ABI | Boundary/argument tests and generated-header drift checks on all three platforms; C harness with helper scenarios on Unix | C consumer harness is excluded on Windows; sanitizer runs depend on toolchain support |
| C++ | Unix header, wrapper, no-exceptions and helper scenarios; required Qt 6 test in a separate Linux job | No Windows/MSVC consumer test; Qt can skip outside the required job |
| .NET | Linux and Windows tests with a managed parent, native helper and native restarted fixture; single-platform NuGet pack/consume checks | No macOS .NET CI job; no continuous Native AOT or single-file publish matrix |
| Objective-C | Linux stub syntax check; macOS real Apple-toolchain build and debug portable-install tests | macOS job allows failure; no optimized ARC or signed-bundle acceptance test |

Tests use temporary directories and test-only signing keys. They cover forged
signatures, invalid URLs, bounded download/extraction, cancellation, signed history,
local file ownership and preservation of unrelated data. Process scenarios cover
successful installation and confirmation, declined shutdown/cancellation, startup
failure, confirmation timeout and a busy installation lock. Core tests also exercise
interrupted transaction recovery. Individual bindings do not each duplicate every
core test.

The C/C++ sanitizer harnesses instrument the consumer side, not the Rust library.
Leak detection is Linux-only. A passing sanitizer run does not establish safety for
every foreign-pointer misuse or for the Objective-C wrapper.

### Run the repository checks

From the repository root, with Rust installed:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo test --workspace --doc --locked
cargo package --locked -p freshen
```

Unix C/C++ tests need C and C++ compilers. To require the Qt integration test, also
install CMake and Qt 6 development files and set `FRESHEN_REQUIRE_QT=1` before
running `cargo test -p freshen-ffi --test cpp_header --test cpp_wrapper --locked`.
Windows Rust tests exclude these Unix harnesses.

With the .NET 8 SDK and Rust installed:

```text
dotnet test bindings/dotnet/Freshen.sln
```

On macOS with Xcode and Rust, run the Objective-C suite:

```sh
sh bindings/objc/Tools/run-tests.sh
```

That script builds debug artifacts. Optimized Objective-C validation requires a
separate release build of both the Rust library and the Objective-C consumer; the
existing script is not a release-mode test.

### Package checks without a product application

The .NET CI jobs stage the current platform's native library and helper, pack a
NuGet archive, inspect its contents, then run a scratch consumer in portable and
RID-specific layouts. This checks loading and helper discovery/execute permissions;
the scratch consumer does not install an update.

The manual [NuGet workflow](.github/workflows/nuget.yml) gathers multiple platforms
into one package. Its full matrix was not exercised in the audited PR run. Validate
the resulting native binaries and consume the package on each claimed runtime
identifier before distribution. The workflow produces artifacts and does not
publish to nuget.org. Core crate packaging does not validate NuGet or XCFramework
distribution, nor does it publish to crates.io.

## Validation with your application

Use an isolated VM, test account or disposable installation of the actual packaged
app, with a separate test feed and keys. Keep production installations and user data
out of destructive failure tests. Repeat for each supported OS, architecture,
compiler/runtime and publish layout.

1. **Deployment and loading.** Start the installed release build from its normal
   location. Verify native library loading, helper discovery, permissions, runtime
   dependencies, application-owned inventory and writable installation root. Test
   your selected .NET publish mode or C++ linkage/compiler, rather than inferring
   support from a fixture build.
2. **Host UI and scheduling.** Check, prepare and arm on worker threads. Marshal
   progress/results to the UI thread, serialize requests, cancel a download, and
   retry offline or failed checks. Verify prompts and errors with the host's
   keyboard and screen-reader workflows.
3. **Shutdown and readiness.** Prepare while the app remains usable. Arm, save
   work, and exit through the normal application path. Decline shutdown once and
   cancel the handoff. Verify single-instance locks and helper dispatch do not
   block readiness or start the normal UI in helper mode.
4. **Successful restart.** Install a signed test update, preserve unrelated user
   files, and check relaunch arguments and the intended executable. Confirm startup
   only after essential initialization succeeds. Verify the committed status and
   explicit backup cleanup.
5. **Failure and recovery.** In disposable copies, exercise failed startup, missing
   confirmation, blocked replacement, interrupted installation and restart during
   recovery. Display the retained status, stop application processes before
   maintenance recovery, and verify restoration of the old program and user data.
   An unconfirmed running process must not be killed or overwritten.
6. **Your feed and transport.** Exercise actual HTTPS hosting, proxy/authentication
   behavior, redirects and timeouts. Reject bad signatures and oversized packages.
   A custom transport must obey callback/sink lifetimes; fixture transport success
   does not validate its network implementation.

For macOS bundle installs, additionally test the actual Developer ID-signed,
notarized and stapled release on each shipping architecture. Verify configured
bundle/Team IDs, helper architecture, Gatekeeper behavior, bundle replacement,
startup confirmation and rollback. Generic portable-install tests and stub syntax
checks do not establish these properties. Current archive support rejects symlinks,
including framework symlinks; see [support boundaries](README.md#support-boundaries).

## Deferred review findings

PR #1 was merged as written; these findings remain open. Passing the automated
suite does not resolve them.

- **Objective-C marshaling lifetimes:** `FSHArena` locals in `FSHUpdater.m` and
  `FSHHost.m` own buffers passed as raw pointers into Rust without the existing
  `FSH_KEEP_ALIVE` annotation. Review their lifetimes under optimized ARC and
  exercise release builds before using this binding in production. This was a
  code-review finding, not a locally reproduced macOS failure. Objective-C
  integration review is deferred until there is an application use case.
- **C ABI status-output error path:** `report_status` in `ffi/src/install.rs`
  allocates version/detail strings before rejecting a null required `out_state`.
  With an existing transaction, that invalid call leaks the allocations. Supply a
  valid writable `out_state`; a regression test and implementation fix remain
  follow-up work. This concerns argument-error handling, not the normal status path.

Track future validation by recording the commit, OS/architecture, toolchain,
package/publish mode, scenarios and results. Avoid upgrading a successful fixture
run into a claim that every product application is verified.
