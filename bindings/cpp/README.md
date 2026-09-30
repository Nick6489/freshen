# Freshen for C++

`freshen.hpp` is a header-only C++17 wrapper over the [freshen C ABI](../../ffi/README.md),
for Qt, Win32 and any other C++ program. It adds no update logic: signature
verification, bounded extraction, the helper process and rollback all run in the native
library. This layer gives C++ code RAII ownership of every handle, typed errors,
`std::function` progress and a transport interface, without exposing the C API's raw
pointers.

## Using it

Put `ffi/include` (for `freshen.h`) and `bindings/cpp/include` on the include path, and link
the `freshen_ffi` library:

```cmake
target_include_directories(app PRIVATE ffi/include bindings/cpp/include)
target_link_directories(app PRIVATE path/to/library)
target_link_libraries(app PRIVATE freshen_ffi)
```

```cpp
#include "freshen/freshen.hpp"

freshen::UpdaterConfig config;
config.product = "my-application";
config.channel = "stable";
config.current_version = "1.0.0";
config.target = "x86_64-unknown-linux-gnu";
config.trusted_keys = {publisher_public_key};   // std::array<uint8_t, 32>, pinned in your app

freshen::Updater updater(config);               // throws freshen::Error; see try_create() below

// Run on a worker thread.
auto candidate = updater.check(freshen::Source::index("https://updates.example.com/index.json"),
                               &cancellation, [](const freshen::Progress& p) { /* worker thread */ });
if (!candidate) return;                         // up to date

for (const freshen::ReleaseNote& note : candidate->history())   // authenticated, oldest first
    show(note.version, note.notes);

freshen::PreparedUpdate prepared = updater.prepare(*candidate, temp_directory, &cancellation);

// Later, once the user agrees and you can shut down:
auto policy = freshen::InstallPolicy::portable(install_dir, "myapp", {"myapp", "libfreshen_ffi.so"});
freshen::Handoff handoff = prepared.arm(policy, helper_path, {"--restarted"});
// Save state and quit. If shutdown is declined instead:  handoff.cancel();
```

In the restarted application, once essential initialization has succeeded:

```cpp
freshen::host::confirm_startup(install_dir);    // a no-op on an ordinary launch
```

## Exceptions or not

Every operation comes in two flavors that share one implementation:

- `try_*()` functions never throw. They return `freshen::Result<T>`, which holds a value or a
  `freshen::Error` (`ok()`, `error()`, `value()`). Use these in code built with
  `-fno-exceptions`, where the throwing functions are compiled out.
- The plain functions (`check()`, `prepare()`, `arm()`, `Updater(config)`) throw
  `freshen::Error`, which derives from `std::runtime_error`.

`Error::code()` is an `ErrorCode` (`Signature`, `Busy`, `SizeLimit`, `Recovery`, `Cancelled`,
...). A failed fetch or a bad signature is always an error, never "up to date": a check
succeeds with an empty `std::optional` only when the application is current.

## Things to know

- **Threading.** Operations block, so run them on a worker thread. An `Updater` runs one
  operation at a time and fails a concurrent call with `ErrorCode::OperationInProgress`.
  Progress handlers and transports run on the worker thread. `Cancellation::cancel()` is safe
  from any thread; copies of a `Cancellation` share one flag.
- **Ownership.** Every type is move-only RAII (except `Cancellation`, which is a shared flag).
  Destroying a `PreparedUpdate` deletes its staged files. **Destroying a `Handoff` does not
  cancel a requested installation**; call `cancel()`. `arm()` consumes the prepared update
  whether it succeeds or fails; an argument that is invalid on its own (such as an
  out-of-range timeout) is rejected first and does not consume it.
- **No exception reaches native code.** A transport or progress handler may throw; it is caught
  at the boundary and reported (as an `IO` failure, with the message kept in `Error::cause()`).
- **Offline or unreachable servers.** A check that cannot reach the server returns an error;
  treat it as "update server unreachable" and retry later, not as a fault.
- **No insecure switches.** There is no option to skip signature verification or allow plain
  HTTP.

## Custom transports

Derive from `freshen::Transport` for authenticated feeds, offline storage or tests, and set
`UpdaterConfig::transport`. Every byte goes through the `DownloadSink`, which enforces the size
limit, cancellation and progress inside the native library, so a transport cannot skip them.
Non-HTTPS URLs and URLs with embedded credentials never reach it.

```cpp
class MyTransport : public freshen::Transport {
public:
    freshen::Status download(const std::string& url, freshen::DownloadSink& sink, std::uint64_t) override {
        // ... fetch, then:
        if (auto ok = sink.set_total(size); !ok) return ok;       // optional
        return sink.write(bytes.data(), bytes.size());            // returns the sink's error unchanged
    }
};
```

## Qt

`tests/qt/qt_test.cpp` is a working Qt 6 program using the wrapper the way a desktop client
would: a `QObject` owns the `Updater` and exposes the flow as signals; blocking calls run on
Qt's thread pool through `QtConcurrent::run`; progress and results are emitted from the worker
and, because the `QObject` lives on the GUI thread, Qt queues them so slots run there; and the
GUI thread cancels a download in flight. It is built with CMake against Qt 6.10 and is run by
the Rust test suite. Include Qt's headers before `freshen.hpp` if you like: they coexist with
Qt's `signals`/`slots`/`emit` macros.

## The helper

Freshen copies an executable into the transaction and runs it after your application quits,
so it must be self-contained. Either ship the `freshen-helper` binary with your application
and pass its path to `arm()`, or use your own executable as the helper by calling
`freshen::host::run_helper_if_requested(argc, argv)` first thing in `main()`, before any UI
or single-instance lock. A launch that named the helper but failed still returns `true`, so a
broken helper never starts the application.

## Tests

`ffi/tests/cpp_wrapper.rs` builds and runs, under `-Wall -Wextra -Werror -pedantic`:

- the header as C++17 and C++20;
- `tests/wrapper_test.cpp`: in-process checks (history and notes, up-to-date versus error,
  forged signatures, plain-HTTP and credential URLs refused, the sink enforcing the size
  limit, cancellation before and during a download, progress, one operation at a time,
  moved-from safety, throwing transports and handlers, arm consuming the update) plus the five
  real-process scenarios shared with the other bindings (commit, cancel, failed-startup
  rollback, unconfirmed timeout, busy lock), plain and under AddressSanitizer and
  UndefinedBehaviorSanitizer with leak detection;
- `tests/no_exceptions.cpp`, a program built with `-fno-exceptions` using only `try_*()`;
- the Qt integration test above.

The Qt test skips itself when cmake or the Qt 6 development files are missing, unless
`FRESHEN_REQUIRE_QT` is set.

## Status

Verified on Linux with g++ 15 and clang++ 21, and Qt 6.10. **Not yet built with MSVC or on
Windows or macOS**; the header uses only standard C++17, but treat those as untested. The
test program uses POSIX APIs (`mkdir`, `sleep`, `dirent`), so the C++ tests are Unix-only
until they are ported.
