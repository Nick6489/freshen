// freshen.hpp - a header-only C++17 wrapper over the freshen C ABI.
//
// It adds no update logic: signature verification, bounded extraction, the helper
// process and rollback all run in the native library. This layer gives C++ programs
// (Qt, Win32, anything) RAII ownership of every handle, typed errors, std::function
// progress, and a transport interface, without leaking the C API's raw pointers.
//
// Error handling comes in two flavors that share one implementation:
//
//   * try_*() functions never throw. They return freshen::Result<T>, which holds a
//     value or a freshen::Error. Use these in code built with -fno-exceptions.
//   * The plain functions (check(), prepare(), arm(), ...) throw freshen::Error. They
//     are compiled out when exceptions are unavailable or FRESHEN_NO_EXCEPTIONS is
//     defined.
//
// Threading: operations block, so run them on a worker thread. An Updater runs one
// operation at a time and fails a concurrent call with ErrorCode::OperationInProgress.
// A handle must not be used from two threads at once, except Cancellation::cancel(),
// which is safe from any thread. Progress and transport callbacks run on the thread
// performing the operation.
//
// No exception ever crosses into native code: transports and progress handlers may
// throw, and it is caught at the boundary.
//
// Link against the freshen_ffi library and put freshen.h (ffi/include) on the include
// path.
#ifndef FRESHEN_HPP
#define FRESHEN_HPP

#include "freshen.h"

#include <array>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <exception>
#include <functional>
#include <memory>
#include <mutex>
#include <optional>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#if !defined(FRESHEN_NO_EXCEPTIONS) && (defined(__cpp_exceptions) || defined(__EXCEPTIONS) || defined(_CPPUNWIND))
#define FRESHEN_HAS_EXCEPTIONS 1
#else
#define FRESHEN_HAS_EXCEPTIONS 0
#endif

static_assert(FRESHEN_ABI_VERSION == 1, "freshen.hpp is written for C ABI version 1");

namespace freshen {

// ---------------------------------------------------------------------------------
// Errors and results
// ---------------------------------------------------------------------------------

/// Classifies a failure. Values below 1000 come from the native library.
enum class ErrorCode : int {
    Cancelled = FRESHEN_ERR_CANCELLED,
    Signature = FRESHEN_ERR_SIGNATURE,
    Invalid = FRESHEN_ERR_INVALID,
    UnsafePath = FRESHEN_ERR_UNSAFE_PATH,
    SizeLimit = FRESHEN_ERR_SIZE_LIMIT,
    HashMismatch = FRESHEN_ERR_HASH_MISMATCH,
    /// Another update owns the installation lock. Retry later.
    Busy = FRESHEN_ERR_BUSY,
    Helper = FRESHEN_ERR_HELPER,
    Recovery = FRESHEN_ERR_RECOVERY,
    /// A filesystem or transport failure, including one reported by a custom Transport.
    IO = FRESHEN_ERR_IO,
    Json = FRESHEN_ERR_JSON,
    Http = FRESHEN_ERR_HTTP,
    Zip = FRESHEN_ERR_ZIP,
    Unknown = FRESHEN_ERR_UNKNOWN,
    InvalidArgument = FRESHEN_ERR_INVALID_ARGUMENT,
    Panic = FRESHEN_ERR_PANIC,
    /// This Updater is already running an operation.
    OperationInProgress = 1001,
    /// The prepared update was already armed, or the handoff was already cancelled.
    AlreadyConsumed = 1002,
};

/// An update operation failed. Derives from std::runtime_error; what() is the message.
class Error : public std::runtime_error {
public:
    Error(ErrorCode code, const std::string& message, std::string cause = {})
        : std::runtime_error(message), code_(code), cause_(std::move(cause)) {}

    ErrorCode code() const noexcept { return code_; }

    /// For ErrorCode::IO from a custom Transport: the message of the failure it reported
    /// or threw. Empty otherwise.
    const std::string& cause() const noexcept { return cause_; }

private:
    ErrorCode code_;
    std::string cause_;
};

namespace detail {

[[noreturn]] inline void raise(const Error& error) {
#if FRESHEN_HAS_EXCEPTIONS
    throw error;
#else
    (void)error;
    std::abort();
#endif
}

}  // namespace detail

/// A value or an Error. value() returns the value, or throws the Error (aborts when
/// exceptions are unavailable), so check ok() first in no-exceptions builds.
template <class T>
class Result {
public:
    Result(T value) : value_(std::move(value)) {}
    Result(Error error) : error_(std::move(error)) {}

    bool ok() const noexcept { return value_.has_value(); }
    explicit operator bool() const noexcept { return ok(); }

    /// The failure. Requires !ok().
    const Error& error() const { return *error_; }

    T& value() & {
        check();
        return *value_;
    }
    const T& value() const& {
        check();
        return *value_;
    }
    /// Moves the value out. Requires ok() (throws or aborts otherwise).
    T&& value() && {
        check();
        return std::move(*value_);
    }

private:
    void check() const {
        if (!value_.has_value()) {
            detail::raise(*error_);
        }
    }
    std::optional<T> value_;
    std::optional<Error> error_;
};

template <>
class Result<void> {
public:
    Result() = default;
    Result(Error error) : error_(std::move(error)) {}

    bool ok() const noexcept { return !error_.has_value(); }
    explicit operator bool() const noexcept { return ok(); }
    const Error& error() const { return *error_; }
    void value() const {
        if (error_.has_value()) {
            detail::raise(*error_);
        }
    }

private:
    std::optional<Error> error_;
};

using Status = Result<void>;

// ---------------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------------

namespace detail {

struct StringDeleter {
    void operator()(char* text) const noexcept { freshen_string_free(text); }
};
struct UpdaterDeleter {
    void operator()(FreshenUpdater* value) const noexcept { freshen_updater_free(value); }
};
struct CandidateDeleter {
    void operator()(FreshenCandidate* value) const noexcept { freshen_candidate_free(value); }
};
struct PreparedDeleter {
    void operator()(FreshenPrepared* value) const noexcept { freshen_prepared_free(value); }
};
struct HandoffDeleter {
    // Freeing does NOT cancel a requested installation.
    void operator()(FreshenHandoff* value) const noexcept { freshen_handoff_free(value); }
};
struct CancelDeleter {
    void operator()(FreshenCancel* value) const noexcept { freshen_cancel_free(value); }
};

/// Copies and frees a library-owned string. Null becomes an empty string.
inline std::string take(char* text) {
    std::unique_ptr<char, StringDeleter> owner(text);
    return text != nullptr ? std::string(text) : std::string();
}

/// The native message is thread-local: read it on the thread that failed, right away.
inline Error error_from_status(std::int32_t status, std::string cause = {}) {
    const char* message = freshen_last_error_message();
    std::string text = (message != nullptr) ? message : "";
    if (text.empty()) {
        text = "freshen operation failed (status " + std::to_string(status) + ")";
    }
    return Error(static_cast<ErrorCode>(status), text, std::move(cause));
}

inline Error invalid_argument(const std::string& message) {
    return Error(ErrorCode::InvalidArgument, message);
}

/// Rejects text the C API would silently truncate.
inline std::optional<Error> check_text(const char* name, const std::string& value) {
    if (value.find('\0') != std::string::npos) {
        return invalid_argument(std::string(name) + " must not contain a NUL character");
    }
    return std::nullopt;
}

inline std::int32_t status_for(const Error& error) {
    const int code = static_cast<int>(error.code());
    return (code > 0 && code < 1000) ? code : FRESHEN_ERR_IO;
}

}  // namespace detail

// ---------------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------------

/// A progress report.
struct Progress {
    enum class Kind { Unknown = 0, Checking, Downloading, Verifying, Extracting, Ready };

    Kind kind = Kind::Unknown;
    /// Bytes downloaded so far (Downloading only).
    std::uint64_t bytes_received = 0;
    /// Total size when known (Downloading only).
    std::optional<std::uint64_t> total_bytes;
    /// The file being extracted (Extracting only).
    std::string path;
};

/// Receives progress on the worker thread. Hop to your UI thread yourself.
using ProgressHandler = std::function<void(const Progress&)>;

namespace detail {

struct ProgressState {
    ProgressHandler handler;
};

}  // namespace detail

extern "C" {

// The native progress callback. It has C linkage because it is called from C, and it
// must never throw.
inline void freshen_cpp_progress(void* user, std::int32_t kind, std::uint64_t received, std::uint64_t total,
                                 const char* path) {
    auto* state = static_cast<freshen::detail::ProgressState*>(user);
    if (state == nullptr || !state->handler) {
        return;
    }
#if FRESHEN_HAS_EXCEPTIONS
    try {
#endif
        freshen::Progress progress;
        switch (kind) {
            case FRESHEN_EVENT_CHECKING:
                progress.kind = freshen::Progress::Kind::Checking;
                break;
            case FRESHEN_EVENT_DOWNLOADING:
                progress.kind = freshen::Progress::Kind::Downloading;
                break;
            case FRESHEN_EVENT_VERIFYING:
                progress.kind = freshen::Progress::Kind::Verifying;
                break;
            case FRESHEN_EVENT_EXTRACTING:
                progress.kind = freshen::Progress::Kind::Extracting;
                break;
            case FRESHEN_EVENT_READY:
                progress.kind = freshen::Progress::Kind::Ready;
                break;
            default:
                progress.kind = freshen::Progress::Kind::Unknown;
                break;
        }
        progress.bytes_received = received;
        if (total != FRESHEN_TOTAL_UNKNOWN) {
            progress.total_bytes = total;
        }
        if (path != nullptr) {
            progress.path = path;
        }
        state->handler(progress);
#if FRESHEN_HAS_EXCEPTIONS
    } catch (...) {
        // A misbehaving handler must not take down the update or unwind into native code.
    }
#endif
}

}  // extern "C"

// ---------------------------------------------------------------------------------
// Cancellation
// ---------------------------------------------------------------------------------

/// Cancels operations. Copies share one flag, and cancel() is safe from any thread.
class Cancellation {
public:
    Cancellation() : handle_(freshen_cancel_new(), detail::CancelDeleter{}) {}

    /// Requests cancellation. Idempotent. A read blocked on the network is bounded by the
    /// request timeout.
    void cancel() const noexcept { freshen_cancel_cancel(handle_.get()); }

    const FreshenCancel* native() const noexcept { return handle_.get(); }

private:
    std::shared_ptr<FreshenCancel> handle_;
};

// ---------------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------------

/// Receives the bytes of one download. Valid only during Transport::download.
///
/// The size limit, cancellation and progress reporting are enforced by the sink inside the
/// native library, so a transport cannot skip them.
class DownloadSink {
public:
    explicit DownloadSink(FreshenSink* sink) noexcept : sink_(sink) {}

    /// Whether the operation has been cancelled. Poll this while blocked on the network.
    bool is_cancelled() const noexcept { return sink_ == nullptr || freshen_sink_is_cancelled(sink_); }

    /// Reports the total size when known so progress can show a percentage. A total above the
    /// limit fails immediately.
    Status set_total(std::uint64_t total_bytes) {
        return finish(sink_ == nullptr ? FRESHEN_ERR_INVALID_ARGUMENT : freshen_sink_set_total(sink_, total_bytes));
    }

    /// Appends downloaded bytes. Fails with ErrorCode::Cancelled once the operation is
    /// cancelled and ErrorCode::SizeLimit past the limit; return that failure unchanged.
    Status write(const void* data, std::size_t size) {
        return finish(sink_ == nullptr ? FRESHEN_ERR_INVALID_ARGUMENT
                                       : freshen_sink_write(sink_, static_cast<const std::uint8_t*>(data), size));
    }
    Status write(std::string_view data) { return write(data.data(), data.size()); }

    void invalidate() noexcept { sink_ = nullptr; }

private:
    static Status finish(std::int32_t status) {
        if (status == FRESHEN_OK) {
            return Status();
        }
        return detail::error_from_status(status);
    }
    FreshenSink* sink_;
};

/// Supplies downloads: custom authentication, offline storage, or test fixtures.
///
/// Every byte must go through the sink. URLs that are not HTTPS, or that embed credentials,
/// are rejected before download() is called. Report failure by returning an Error (or by
/// throwing one; exceptions are caught at the boundary). Return a sink error unchanged so
/// its code (cancelled, size limit) is preserved; any other failure becomes ErrorCode::IO
/// with your message attached as cause().
class Transport {
public:
    virtual ~Transport() = default;
    virtual Status download(const std::string& url, DownloadSink& sink, std::uint64_t limit) = 0;
};

namespace detail {

/// State the native updater carries for its custom transport.
struct TransportState {
    std::shared_ptr<Transport> transport;
    std::optional<Error> last;

    std::string take_cause() {
        std::string cause = last.has_value() ? last->what() : std::string();
        last.reset();
        return cause;
    }
};

}  // namespace detail

extern "C" {

// The native download callback. C linkage; must never throw.
inline std::int32_t freshen_cpp_download(void* user, const char* url, FreshenSink* sink, std::uint64_t limit) {
    auto* state = static_cast<freshen::detail::TransportState*>(user);
    freshen::DownloadSink wrapper(sink);
    std::int32_t result = FRESHEN_OK;
#if FRESHEN_HAS_EXCEPTIONS
    try {
#endif
        freshen::Status status = state->transport->download(url, wrapper, limit);
        if (!status.ok()) {
            state->last = status.error();
            result = freshen::detail::status_for(status.error());
        }
#if FRESHEN_HAS_EXCEPTIONS
    } catch (const freshen::Error& error) {
        state->last = error;
        result = freshen::detail::status_for(error);
    } catch (const std::exception& error) {
        state->last = freshen::Error(freshen::ErrorCode::IO, error.what());
        result = FRESHEN_ERR_IO;
    } catch (...) {
        state->last = freshen::Error(freshen::ErrorCode::IO, "the transport threw an unknown exception");
        result = FRESHEN_ERR_IO;
    }
#endif
    wrapper.invalidate();
    return result;
}

}  // extern "C"

// ---------------------------------------------------------------------------------
// Release sources and policy
// ---------------------------------------------------------------------------------

/// Authenticated notes for one release.
struct ReleaseNote {
    std::string version;
    /// Possibly empty; the host owns formatting, localization and the empty-notes fallback.
    std::string notes;
};

/// Where to look for signed releases. Every source shares the same signature checks.
class Source {
public:
    /// A JSON array of manifest locations (at most 100, 1 MiB). The index only locates
    /// manifests; each one is authenticated separately.
    static Source index(std::string url) { return Source(FRESHEN_SOURCE_INDEX, std::move(url), {}, {}); }
    /// A single signed manifest and its detached signature.
    static Source manifest(std::string document_url, std::string signature_url) {
        return Source(FRESHEN_SOURCE_MANIFEST, std::move(document_url), std::move(signature_url), {});
    }
    /// The first 100 releases of a GitHub repository. Release metadata itself is not trusted.
    static Source github(std::string owner, std::string repository) {
        return Source(FRESHEN_SOURCE_GITHUB, std::move(owner), std::move(repository), {});
    }
    /// Manifest locations supplied by the host's own discovery (at most 100), as
    /// (document URL, signature URL) pairs.
    static Source manifests(std::vector<std::pair<std::string, std::string>> locations) {
        return Source(FRESHEN_SOURCE_MANIFESTS, {}, {}, std::move(locations));
    }

    /// Fills a native description. The pointers stay valid while this Source and
    /// `locations` are alive.
    std::optional<Error> to_native(FreshenSource& native, std::vector<FreshenLocation>& locations) const {
        for (const std::string* text : {&a_, &b_}) {
            if (auto failure = detail::check_text("source", *text)) {
                return failure;
            }
        }
        native = FreshenSource{};
        native.struct_size = sizeof native;
        native.kind = kind_;
        native.a = a_.c_str();
        native.b = b_.c_str();
        if (!locations_.empty()) {
            locations.clear();
            locations.reserve(locations_.size());
            for (const auto& pair : locations_) {
                for (const std::string* text : {&pair.first, &pair.second}) {
                    if (auto failure = detail::check_text("location", *text)) {
                        return failure;
                    }
                }
                FreshenLocation location{};
                location.document = pair.first.c_str();
                location.signature = pair.second.c_str();
                locations.push_back(location);
            }
            native.locations = locations.data();
            native.location_count = locations.size();
        }
        return std::nullopt;
    }

private:
    Source(int kind, std::string a, std::string b, std::vector<std::pair<std::string, std::string>> locations)
        : kind_(kind), a_(std::move(a)), b_(std::move(b)), locations_(std::move(locations)) {}

    int kind_;
    std::string a_;
    std::string b_;
    std::vector<std::pair<std::string, std::string>> locations_;
};

/// Where an installation transaction stands.
enum class InstallState : int {
    Unknown = FRESHEN_STATE_UNKNOWN,
    /// Staged and waiting; nothing has been replaced.
    Prepared = FRESHEN_STATE_PREPARED,
    Applying = FRESHEN_STATE_APPLYING,
    /// Installed; waiting for the new version to confirm startup.
    AwaitingConfirmation = FRESHEN_STATE_AWAITING_CONFIRMATION,
    Committed = FRESHEN_STATE_COMMITTED,
    RollingBack = FRESHEN_STATE_ROLLING_BACK,
    /// A failed update was restored; the old version is intact.
    RolledBack = FRESHEN_STATE_ROLLED_BACK,
    /// The update failed and recovery material was kept.
    Failed = FRESHEN_STATE_FAILED,
};

/// The last installation transaction under an installation root.
struct InstallationStatus {
    InstallState state = InstallState::Unknown;
    std::string version;
    /// Human-readable detail for diagnostics.
    std::string detail;
};

/// The application's local ownership policy for an install. It is decided by the application
/// and never inferred from a download.
class InstallPolicy {
public:
    /// A portable directory of application-owned files. Only `owned_files` are replaced (plus
    /// files strictly beneath `expandable_directories`); the manifest cannot introduce anything
    /// else.
    static InstallPolicy portable(std::string root, std::string executable, std::vector<std::string> owned_files,
                                  std::vector<std::string> expandable_directories = {}) {
        InstallPolicy policy;
        policy.layout_ = FRESHEN_LAYOUT_PORTABLE;
        policy.root_ = std::move(root);
        policy.executable_ = std::move(executable);
        policy.owned_files_ = std::move(owned_files);
        policy.expandable_directories_ = std::move(expandable_directories);
        return policy;
    }

    /// A macOS application bundle, replaced where it is (never moved to /Applications). The signed
    /// archive must include the bundle prefix, and the bundle must be signed and notarized first.
    static InstallPolicy mac_bundle(std::string parent_directory, std::string bundle_name, std::string executable,
                                    std::string bundle_id, std::string team_id) {
        InstallPolicy policy;
        policy.layout_ = FRESHEN_LAYOUT_MAC_BUNDLE;
        policy.root_ = std::move(parent_directory);
        policy.executable_ = std::move(executable);
        policy.bundle_name_ = std::move(bundle_name);
        policy.bundle_id_ = std::move(bundle_id);
        policy.team_id_ = std::move(team_id);
        return policy;
    }

    /// How long the helper waits for this process to exit. Zero keeps the default (120 s).
    std::chrono::seconds exit_timeout{0};
    /// How long the helper waits for the new version to confirm startup. Zero keeps the default.
    std::chrono::seconds confirmation_timeout{0};

    /// Storage the native description points into.
    struct Native {
        FreshenInstallPolicy policy{};
        std::vector<const char*> owned_files;
        std::vector<const char*> expandable_directories;
    };

    /// Validates and fills a native description. `out` must outlive its use.
    Status to_native(Native& out) const {
        for (const std::string* text : {&root_, &executable_, &bundle_name_, &bundle_id_, &team_id_}) {
            if (auto failure = detail::check_text("policy", *text)) {
                return *failure;
            }
        }
        for (const auto* list : {&owned_files_, &expandable_directories_}) {
            for (const std::string& text : *list) {
                if (auto failure = detail::check_text("policy", text)) {
                    return *failure;
                }
            }
        }
        for (const std::chrono::seconds value : {exit_timeout, confirmation_timeout}) {
            if (value.count() != 0 && (value.count() < 1 || value.count() > 3600)) {
                return detail::invalid_argument("timeouts must be zero (the default) or between one second and one hour");
            }
        }
        out = Native{};
        FreshenInstallPolicy& native = out.policy;
        native.struct_size = sizeof native;
        native.layout = layout_;
        native.root = root_.c_str();
        native.executable = executable_.c_str();
        native.exit_timeout_secs = static_cast<std::uint64_t>(exit_timeout.count());
        native.confirmation_timeout_secs = static_cast<std::uint64_t>(confirmation_timeout.count());
        if (layout_ == FRESHEN_LAYOUT_PORTABLE) {
            for (const std::string& text : owned_files_) {
                out.owned_files.push_back(text.c_str());
            }
            for (const std::string& text : expandable_directories_) {
                out.expandable_directories.push_back(text.c_str());
            }
            native.owned_files = out.owned_files.empty() ? nullptr : out.owned_files.data();
            native.owned_file_count = out.owned_files.size();
            native.expandable_directories = out.expandable_directories.empty() ? nullptr : out.expandable_directories.data();
            native.expandable_directory_count = out.expandable_directories.size();
        } else {
            native.bundle_name = bundle_name_.c_str();
            native.bundle_id = bundle_id_.c_str();
            native.team_id = team_id_.c_str();
        }
        return Status();
    }

private:
    InstallPolicy() = default;

    int layout_ = FRESHEN_LAYOUT_PORTABLE;
    std::string root_;
    std::string executable_;
    std::vector<std::string> owned_files_;
    std::vector<std::string> expandable_directories_;
    std::string bundle_name_;
    std::string bundle_id_;
    std::string team_id_;
};

// ---------------------------------------------------------------------------------
// Candidate, prepared update, handoff
// ---------------------------------------------------------------------------------

class Updater;
class PreparedUpdate;

/// An armed update: a helper holds the installation lock and waits for this process to exit.
/// **Destroying a Handoff does not cancel the installation.**
class Handoff {
public:
    Handoff(Handoff&&) noexcept = default;
    Handoff& operator=(Handoff&&) noexcept = default;

    /// Process id of the waiting helper.
    std::uint32_t helper_process_id() const noexcept { return pid_; }

    /// Cancels the requested installation because shutdown was declined. Blocking. A handoff can
    /// be cancelled once; a second call fails with ErrorCode::AlreadyConsumed.
    Status try_cancel() {
        if (!handle_ || cancelled_) {
            return Error(ErrorCode::AlreadyConsumed, "this handoff was already cancelled");
        }
        cancelled_ = true;  // the native call consumes the handoff whether or not it succeeds
        const std::int32_t status = freshen_handoff_cancel(handle_.get());
        if (status != FRESHEN_OK) {
            return detail::error_from_status(status);
        }
        return Status();
    }

#if FRESHEN_HAS_EXCEPTIONS
    void cancel() { try_cancel().value(); }
#endif

private:
    friend class PreparedUpdate;
    explicit Handoff(FreshenHandoff* raw) : handle_(raw), pid_(freshen_handoff_helper_pid(raw)) {}

    std::unique_ptr<FreshenHandoff, detail::HandoffDeleter> handle_;
    std::uint32_t pid_ = 0;
    bool cancelled_ = false;
};

/// A fully downloaded, verified and staged update. Holding one lets the user defer
/// installation. Destroying it without arming deletes its temporary files and leaves the
/// installation untouched.
class PreparedUpdate {
public:
    PreparedUpdate(PreparedUpdate&&) noexcept = default;
    PreparedUpdate& operator=(PreparedUpdate&&) noexcept = default;

    const std::string& version() const noexcept { return version_; }
    const std::string& notes() const noexcept { return notes_; }
    /// Authenticated release history retained by the update check, oldest first.
    const std::vector<ReleaseNote>& history() const noexcept { return history_; }

    /// Copies the verified staging into the installation and starts a helper that waits for this
    /// process to exit. Returns once the helper holds the installation lock. Save your work and
    /// quit, or cancel() the handoff if shutdown is declined. Blocking.
    ///
    /// This update is consumed whether the call succeeds or fails; a second call fails with
    /// ErrorCode::AlreadyConsumed. An argument that is invalid on its own (such as an
    /// out-of-range timeout) is rejected first and does not consume it.
    ///
    /// `helper_executable` must be self-contained: the freshen-helper binary shipped with the
    /// application, or the application executable itself if it calls host::run_helper_if_requested
    /// first thing in main().
    Result<Handoff> try_arm(const InstallPolicy& policy, const std::string& helper_executable,
                            const std::vector<std::string>& relaunch_arguments = {}) {
        if (!handle_) {
            return Error(ErrorCode::AlreadyConsumed, "this update was already armed, or moved from");
        }
        InstallPolicy::Native native;
        if (Status valid = policy.to_native(native); !valid.ok()) {
            return valid.error();
        }
        if (auto failure = detail::check_text("helper_executable", helper_executable)) {
            return *failure;
        }
        std::vector<const char*> arguments;
        for (const std::string& argument : relaunch_arguments) {
            if (auto failure = detail::check_text("relaunch_arguments", argument)) {
                return *failure;
            }
            arguments.push_back(argument.c_str());
        }
        if (consumed_) {
            return Error(ErrorCode::AlreadyConsumed, "this update was already armed");
        }
        consumed_ = true;

        FreshenHandoff* raw = nullptr;
        const std::int32_t status =
            freshen_arm(handle_.get(), &native.policy, helper_executable.c_str(), arguments.empty() ? nullptr : arguments.data(),
                        arguments.size(), &raw);
        if (status != FRESHEN_OK) {
            return detail::error_from_status(status);
        }
        return Handoff(raw);
    }

#if FRESHEN_HAS_EXCEPTIONS
    Handoff arm(const InstallPolicy& policy, const std::string& helper_executable,
                const std::vector<std::string>& relaunch_arguments = {}) {
        return try_arm(policy, helper_executable, relaunch_arguments).value();
    }
#endif

private:
    friend class Updater;
    explicit PreparedUpdate(FreshenPrepared* raw) : handle_(raw) {
        version_ = detail::take(freshen_prepared_version(raw));
        notes_ = detail::take(freshen_prepared_notes(raw));
        const std::size_t count = freshen_prepared_history_count(raw);
        for (std::size_t index = 0; index < count; ++index) {
            history_.push_back({detail::take(freshen_prepared_history_version(raw, index)),
                                detail::take(freshen_prepared_history_notes(raw, index))});
        }
    }

    std::unique_ptr<FreshenPrepared, detail::PreparedDeleter> handle_;
    std::string version_;
    std::string notes_;
    std::vector<ReleaseNote> history_;
    bool consumed_ = false;
};

/// A verified newer release that has not been downloaded yet.
class Candidate {
public:
    Candidate(Candidate&&) noexcept = default;
    Candidate& operator=(Candidate&&) noexcept = default;

    /// Version of the selected (newest) release.
    const std::string& version() const noexcept { return version_; }
    /// Notes of the selected release; possibly empty.
    const std::string& notes() const noexcept { return notes_; }
    /// Size in bytes of the package that prepare() will download.
    std::uint64_t download_size() const noexcept { return download_size_; }
    /// Authenticated releases newer than the installed version, oldest first, through the
    /// selected one. This is the available history, not a guarantee of completeness.
    const std::vector<ReleaseNote>& history() const noexcept { return history_; }

    const FreshenCandidate* native() const noexcept { return handle_.get(); }

private:
    friend class Updater;
    explicit Candidate(FreshenCandidate* raw) : handle_(raw) {
        version_ = detail::take(freshen_candidate_version(raw));
        notes_ = detail::take(freshen_candidate_notes(raw));
        download_size_ = freshen_candidate_download_size(raw);
        const std::size_t count = freshen_candidate_history_count(raw);
        for (std::size_t index = 0; index < count; ++index) {
            history_.push_back({detail::take(freshen_candidate_history_version(raw, index)),
                                detail::take(freshen_candidate_history_notes(raw, index))});
        }
    }

    std::unique_ptr<FreshenCandidate, detail::CandidateDeleter> handle_;
    std::string version_;
    std::string notes_;
    std::uint64_t download_size_ = 0;
    std::vector<ReleaseNote> history_;
};

// ---------------------------------------------------------------------------------
// Updater
// ---------------------------------------------------------------------------------

/// Settings for an Updater.
struct UpdaterConfig {
    /// Signed product name that releases must match.
    std::string product;
    /// Signed channel that releases must match, such as "stable".
    std::string channel;
    /// Semantic version of the running application.
    std::string current_version;
    /// Distribution target, for example "x86_64-unknown-linux-gnu".
    std::string target;
    /// Pinned Ed25519 public keys, 32 bytes each. Ship old and new keys together to rotate.
    std::vector<std::array<std::uint8_t, 32>> trusted_keys;
    /// Largest package download accepted. Defaults to 256 MiB.
    std::uint64_t max_download_bytes = 256ULL * 1024 * 1024;
    /// Largest total unpacked size accepted. Defaults to 1 GiB.
    std::uint64_t max_unpacked_bytes = 1024ULL * 1024 * 1024;
    /// Total timeout of the built-in HTTPS transport. Ignored when `transport` is set.
    std::chrono::seconds http_timeout{300};
    /// Optional custom transport for authentication, offline storage or tests. Null selects
    /// the built-in HTTPS transport. Shared, so it outlives the updater safely.
    std::shared_ptr<Transport> transport;
};

/// Checks for and prepares authenticated updates. Movable, not copyable.
class Updater {
public:
    /// Creates an updater. Never throws.
    static Result<Updater> try_create(const UpdaterConfig& config) {
        const std::pair<const char*, const std::string*> texts[] = {{"product", &config.product},
                                                                    {"channel", &config.channel},
                                                                    {"current_version", &config.current_version},
                                                                    {"target", &config.target}};
        for (const auto& field : texts) {
            if (auto failure = detail::check_text(field.first, *field.second)) {
                return *failure;
            }
        }
        if (config.trusted_keys.empty()) {
            return detail::invalid_argument("at least one trusted public key is required");
        }
        if (config.http_timeout.count() < 0) {
            return detail::invalid_argument("http_timeout must not be negative");
        }
        std::vector<std::uint8_t> keys;
        keys.reserve(config.trusted_keys.size() * 32);
        for (const auto& key : config.trusted_keys) {
            keys.insert(keys.end(), key.begin(), key.end());
        }

        auto impl = std::make_unique<Impl>();
        FreshenTransport transport{};
        FreshenUpdaterConfig native{};
        native.struct_size = sizeof native;
        native.product = config.product.c_str();
        native.channel = config.channel.c_str();
        native.current_version = config.current_version.c_str();
        native.target = config.target.c_str();
        native.trusted_keys = keys.data();
        native.trusted_key_count = config.trusted_keys.size();
        native.max_download = config.max_download_bytes;
        native.max_unpacked = config.max_unpacked_bytes;
        native.http_timeout_secs = static_cast<std::uint64_t>(config.http_timeout.count());
        if (config.transport) {
            impl->transport = std::make_unique<detail::TransportState>();
            impl->transport->transport = config.transport;
            transport.struct_size = sizeof transport;
            transport.user = impl->transport.get();
            transport.download = freshen_cpp_download;
            native.transport = &transport;
        }

        FreshenUpdater* raw = nullptr;
        const std::int32_t status = freshen_updater_new(&native, &raw);
        if (status != FRESHEN_OK) {
            return detail::error_from_status(status);
        }
        impl->handle.reset(raw);
        return Updater(std::move(impl));
    }

#if FRESHEN_HAS_EXCEPTIONS
    /// Creates an updater. Throws Error on failure.
    explicit Updater(const UpdaterConfig& config) : Updater(try_create(config).value()) {}
#endif

    Updater(Updater&&) noexcept = default;
    Updater& operator=(Updater&&) noexcept = default;

    /// Checks for a newer authenticated release. Blocking.
    ///
    /// A failed fetch or a bad signature is an error, never "up to date": success with an
    /// empty optional means the application is current.
    Result<std::optional<Candidate>> try_check(const Source& source, const Cancellation* cancellation = nullptr,
                                               ProgressHandler progress = {}) {
        auto guard = begin();
        if (!guard.ok()) {
            return guard.error();
        }
        FreshenSource native{};
        std::vector<FreshenLocation> locations;
        if (auto failure = source.to_native(native, locations)) {
            return *failure;
        }
        detail::ProgressState state{std::move(progress)};
        const bool reporting = static_cast<bool>(state.handler);
        FreshenCandidate* raw = nullptr;
        const std::int32_t status =
            freshen_check(impl_->handle.get(), &native, cancellation ? cancellation->native() : nullptr,
                          reporting ? freshen_cpp_progress : nullptr, reporting ? &state : nullptr, &raw);
        if (status != FRESHEN_OK) {
            return error_for(status);
        }
        if (raw == nullptr) {
            return std::optional<Candidate>();
        }
        return std::optional<Candidate>(Candidate(raw));
    }

    /// Downloads, verifies and extracts a candidate into a private directory beneath
    /// `temporary_directory`. The candidate stays valid. Blocking.
    Result<PreparedUpdate> try_prepare(const Candidate& candidate, const std::string& temporary_directory,
                                       const Cancellation* cancellation = nullptr, ProgressHandler progress = {}) {
        auto guard = begin();
        if (!guard.ok()) {
            return guard.error();
        }
        if (auto failure = detail::check_text("temporary_directory", temporary_directory)) {
            return *failure;
        }
        detail::ProgressState state{std::move(progress)};
        const bool reporting = static_cast<bool>(state.handler);
        FreshenPrepared* raw = nullptr;
        const std::int32_t status = freshen_prepare(impl_->handle.get(), candidate.native(), temporary_directory.c_str(),
                                                    cancellation ? cancellation->native() : nullptr,
                                                    reporting ? freshen_cpp_progress : nullptr, reporting ? &state : nullptr, &raw);
        if (status != FRESHEN_OK) {
            return error_for(status);
        }
        return PreparedUpdate(raw);
    }

#if FRESHEN_HAS_EXCEPTIONS
    /// Like try_check but throws Error on failure. Empty optional = up to date.
    std::optional<Candidate> check(const Source& source, const Cancellation* cancellation = nullptr,
                                   ProgressHandler progress = {}) {
        return try_check(source, cancellation, std::move(progress)).value();
    }

    /// Like try_prepare but throws Error on failure.
    PreparedUpdate prepare(const Candidate& candidate, const std::string& temporary_directory,
                           const Cancellation* cancellation = nullptr, ProgressHandler progress = {}) {
        return try_prepare(candidate, temporary_directory, cancellation, std::move(progress)).value();
    }
#endif

private:
    struct Impl {
        std::unique_ptr<FreshenUpdater, detail::UpdaterDeleter> handle;
        std::unique_ptr<detail::TransportState> transport;
        std::mutex busy;
    };

    explicit Updater(std::unique_ptr<Impl> impl) noexcept : impl_(std::move(impl)) {}

    /// Refuses overlap and moved-from use. Holds the busy lock for the operation.
    struct Guard {
        std::unique_lock<std::mutex> lock;
        std::optional<Error> failure;
        bool ok() const noexcept { return !failure.has_value(); }
        const Error& error() const { return *failure; }
    };

    Guard begin() {
        Guard guard;
        if (!impl_) {
            guard.failure = detail::invalid_argument("this Updater was moved from");
            return guard;
        }
        guard.lock = std::unique_lock<std::mutex>(impl_->busy, std::try_to_lock);
        if (!guard.lock.owns_lock()) {
            guard.failure = Error(ErrorCode::OperationInProgress, "an update operation is already running on this Updater");
        }
        return guard;
    }

    Error error_for(std::int32_t status) {
        return detail::error_from_status(status, impl_->transport ? impl_->transport->take_cause() : std::string());
    }

    std::unique_ptr<Impl> impl_;
};

// ---------------------------------------------------------------------------------
// Process-level entry points
// ---------------------------------------------------------------------------------

namespace host {

/// The C ABI version of the loaded native library.
inline std::uint32_t abi_version() noexcept { return freshen_abi_version(); }

/// The version of the loaded native library.
inline std::string library_version() {
    const char* version = freshen_library_version();
    return version != nullptr ? version : "";
}

/// Self-helper dispatch: call first in main(), before any UI or single-instance lock, with the
/// arguments EXCLUDING the program name. Returns true when this process was launched as the
/// update helper and has finished; return from main() without starting normally.
inline Result<bool> run_helper(const std::vector<std::string>& arguments) {
    std::vector<const char*> argv;
    for (const std::string& argument : arguments) {
        if (auto failure = detail::check_text("arguments", argument)) {
            return *failure;
        }
        argv.push_back(argument.c_str());
    }
    bool handled = false;
    const std::int32_t status = freshen_run_helper(argv.size(), argv.empty() ? nullptr : argv.data(), &handled);
    if (status != FRESHEN_OK) {
        return detail::error_from_status(status);
    }
    return handled;
}

/// Convenience for main(int argc, char** argv): true means "exit now, you were the helper".
/// A launch that named the helper (--freshen-apply) but failed still returns true so a broken
/// helper never starts the application; the failure is reported on stderr.
inline bool run_helper_if_requested(int argc, const char* const* argv) noexcept {
    if (argc < 2) {
        return false;
    }
    std::vector<std::string> arguments;
    for (int index = 1; index < argc; ++index) {
        arguments.emplace_back(argv[index]);
    }
    const bool helper_launch = arguments.front() == "--freshen-apply";
    Result<bool> result = run_helper(arguments);
    if (result.ok()) {
        return result.value();
    }
    if (helper_launch) {
        std::fprintf(stderr, "freshen helper failed: %s\n", result.error().what());
    }
    return helper_launch;
}

/// Call from the restarted application once its essential initialization has succeeded.
/// A harmless no-op for an ordinary launch. True only when this launch was an update
/// awaiting confirmation.
inline Result<bool> confirm_startup(const std::string& installation_root) {
    if (auto failure = detail::check_text("installation_root", installation_root)) {
        return *failure;
    }
    bool confirmed = false;
    const std::int32_t status = freshen_confirm_startup(installation_root.c_str(), &confirmed);
    if (status != FRESHEN_OK) {
        return detail::error_from_status(status);
    }
    return confirmed;
}

namespace detail_host {

inline InstallState state_from_native(std::int32_t state) {
    switch (state) {
        case FRESHEN_STATE_PREPARED:
            return InstallState::Prepared;
        case FRESHEN_STATE_APPLYING:
            return InstallState::Applying;
        case FRESHEN_STATE_AWAITING_CONFIRMATION:
            return InstallState::AwaitingConfirmation;
        case FRESHEN_STATE_COMMITTED:
            return InstallState::Committed;
        case FRESHEN_STATE_ROLLING_BACK:
            return InstallState::RollingBack;
        case FRESHEN_STATE_ROLLED_BACK:
            return InstallState::RolledBack;
        case FRESHEN_STATE_FAILED:
            return InstallState::Failed;
        default:
            return InstallState::Unknown;
    }
}

inline Result<std::optional<InstallationStatus>> read_status(const std::string& root, bool recover) {
    if (auto failure = detail::check_text("installation_root", root)) {
        return *failure;
    }
    std::int32_t state = -1;
    char* version = nullptr;
    char* detail_text = nullptr;
    const std::int32_t status = recover ? freshen_recover_installation(root.c_str(), &state, &version, &detail_text)
                                        : freshen_installation_status(root.c_str(), &state, &version, &detail_text);
    // Take ownership of both strings before deciding whether to fail, so nothing leaks.
    std::string version_text = detail::take(version);
    std::string detail_string = detail::take(detail_text);
    if (status != FRESHEN_OK) {
        return detail::error_from_status(status);
    }
    if (state == FRESHEN_STATE_NONE) {
        return std::optional<InstallationStatus>();
    }
    return std::optional<InstallationStatus>(InstallationStatus{state_from_native(state), std::move(version_text), std::move(detail_string)});
}

}  // namespace detail_host

/// Read-only status of the last installation transaction, or an empty optional if there is
/// none. Safe to show on a later launch.
inline Result<std::optional<InstallationStatus>> installation_status(const std::string& installation_root) {
    return detail_host::read_status(installation_root, false);
}

/// Resumes a pending rollback once every application process is stopped. Run it from a
/// maintenance process when the target executable may be locked.
inline Result<InstallationStatus> recover_installation(const std::string& installation_root) {
    Result<std::optional<InstallationStatus>> result = detail_host::read_status(installation_root, true);
    if (!result.ok()) {
        return result.error();
    }
    std::optional<InstallationStatus>& status = result.value();
    if (!status.has_value()) {
        return Error(ErrorCode::Unknown, "recovery reported no transaction");
    }
    return std::move(*status);
}

/// Removes a finished (committed or rolled back) or cancelled transaction and its backups,
/// which are otherwise kept. Fails with ErrorCode::Busy while a helper is still active.
inline Status discard_finished_installation(const std::string& installation_root) {
    if (auto failure = detail::check_text("installation_root", installation_root)) {
        return *failure;
    }
    const std::int32_t status = freshen_discard_finished_installation(installation_root.c_str());
    if (status != FRESHEN_OK) {
        return detail::error_from_status(status);
    }
    return Status();
}

}  // namespace host

}  // namespace freshen

#endif  // FRESHEN_HPP
