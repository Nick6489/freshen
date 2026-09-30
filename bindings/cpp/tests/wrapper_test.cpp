// Tests for freshen.hpp, mirroring the C harness, the .NET tests and the Objective-C tests.
// One executable plays every role, like a host that uses itself as the update helper:
//
//   wrapper_test selftest FIXTURES                          in-process checks
//   wrapper_test parent FIXTURES ROOT MODE HELPER           check, prepare, arm
//   wrapper_test child FIXTURES ROOT MODE                   the restarted application
//   wrapper_test --freshen-apply ...                        the update helper
//
// Driven by ffi/tests/cpp_wrapper.rs, which builds the signed fixtures and runs it plain and
// under the sanitizers. Any failed check prints its location and exits nonzero.
#include "fixture_transport.hpp"

#include <sys/stat.h>
#include <unistd.h>

#include <cerrno>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <dirent.h>
#include <fstream>
#include <future>
#include <iterator>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#define CHECK(condition)                                                                              \
    do {                                                                                              \
        if (!(condition)) {                                                                           \
            std::fprintf(stderr, "FAIL %s:%d: %s (last error: %s)\n", __FILE__, __LINE__, #condition, \
                         freshen_last_error_message());                                               \
            std::exit(1);                                                                             \
        }                                                                                             \
    } while (false)

#define CHECK_OK(result)                                                                          \
    do {                                                                                          \
        if (!(result).ok()) {                                                                     \
            std::fprintf(stderr, "FAIL %s:%d: %s failed: %s\n", __FILE__, __LINE__, #result,      \
                         (result).error().what());                                                \
            std::exit(1);                                                                         \
        }                                                                                         \
    } while (false)

#define CHECK_CODE(result, expected)                                                                   \
    do {                                                                                               \
        if ((result).ok() || (result).error().code() != (expected)) {                                  \
            std::fprintf(stderr, "FAIL %s:%d: %s did not fail with %s (%s)\n", __FILE__, __LINE__,     \
                         #result, #expected, (result).ok() ? "it succeeded" : (result).error().what()); \
            std::exit(1);                                                                              \
        }                                                                                              \
    } while (false)

namespace {

using freshen::ErrorCode;

std::string read_text(const std::string& path) {
    std::ifstream stream(path, std::ios::binary);
    return std::string((std::istreambuf_iterator<char>(stream)), std::istreambuf_iterator<char>());
}

int count_entries(const std::string& directory) {
    DIR* handle = opendir(directory.c_str());
    CHECK(handle != nullptr);
    int count = 0;
    while (const dirent* entry = readdir(handle)) {
        const std::string name = entry->d_name;
        if (name != "." && name != "..") {
            ++count;
        }
    }
    closedir(handle);
    return count;
}

std::array<std::uint8_t, 32> read_key(const std::string& fixtures) {
    const std::string bytes = read_text(fixtures + "/pubkey");
    CHECK(bytes.size() == 32);
    std::array<std::uint8_t, 32> key{};
    for (std::size_t index = 0; index < 32; ++index) {
        key[index] = static_cast<std::uint8_t>(bytes[index]);
    }
    return key;
}

freshen::UpdaterConfig make_config(const std::string& fixtures, const std::shared_ptr<freshen::Transport>& transport,
                                   const std::string& current = "1.0.0",
                                   std::uint64_t max_download = 512ULL * 1024 * 1024) {
    freshen::UpdaterConfig config;
    config.product = "ffi-test";
    config.channel = "stable";
    config.current_version = current;
    config.target = "test-target";
    config.trusted_keys = {read_key(fixtures)};
    config.max_download_bytes = max_download;
    config.max_unpacked_bytes = 64ULL * 1024 * 1024;
    config.transport = transport;
    return config;
}

freshen::Updater make_updater(freshen::UpdaterConfig config) {
    auto created = freshen::Updater::try_create(config);
    CHECK_OK(created);
    return std::move(created).value();
}

const freshen::Source& index_source() {
    static const freshen::Source source = freshen::Source::index("https://test.invalid/index");
    return source;
}

/// Checks and returns the candidate, failing the test if there is none.
freshen::Candidate checked_candidate(freshen::Updater& updater) {
    auto checked = updater.try_check(index_source());
    CHECK_OK(checked);
    CHECK(checked.value().has_value());
    return std::move(*checked.value());
}

std::string make_directory(const std::string& base, const std::string& name) {
    const std::string path = base + "/" + name;
    if (mkdir(path.c_str(), 0700) != 0) {
        CHECK(errno == EEXIST);
    }
    return path;
}

// ---- in-process checks ----

void selftest(const std::string& fixtures) {
    CHECK(freshen::host::abi_version() == FRESHEN_ABI_VERSION);
    CHECK(!freshen::host::library_version().empty());

    auto transport = std::make_shared<FixtureTransport>(fixtures);

    // Configuration errors are reported, not crashed on.
    {
        freshen::UpdaterConfig config = make_config(fixtures, transport);
        config.trusted_keys.clear();
        auto created = freshen::Updater::try_create(config);
        CHECK_CODE(created, ErrorCode::InvalidArgument);

        config = make_config(fixtures, transport);
        config.product = std::string("has\0nul", 7);
        created = freshen::Updater::try_create(config);
        CHECK_CODE(created, ErrorCode::InvalidArgument);

        config = make_config(fixtures, transport);
        config.current_version = "not-semver";
        created = freshen::Updater::try_create(config);
        CHECK_CODE(created, ErrorCode::InvalidArgument);
    }

    freshen::Updater updater = make_updater(make_config(fixtures, transport));

    // A signed index yields the newest release plus authenticated history.
    {
        std::vector<freshen::Progress> events;
        auto checked = updater.try_check(index_source(), nullptr, [&events](const freshen::Progress& progress) {
            events.push_back(progress);
        });
        CHECK_OK(checked);
        CHECK(checked.value().has_value());
        const freshen::Candidate& candidate = *checked.value();
        CHECK(candidate.version() == "2.0.0");
        CHECK(candidate.notes() == "notes two");
        CHECK(candidate.download_size() > 0);
        CHECK(candidate.history().size() == 2);
        CHECK(candidate.history()[0].version == "1.5.0" && candidate.history()[0].notes == "notes one");
        CHECK(candidate.history()[1].version == "2.0.0" && candidate.history()[1].notes == "notes two");
        // A check reports only that it started; download progress belongs to prepare.
        CHECK(events.size() == 1 && events[0].kind == freshen::Progress::Kind::Checking);
    }

    // Already current: success with an empty optional, distinct from any error.
    {
        freshen::Updater current = make_updater(make_config(fixtures, transport, "2.0.0", 1024));
        auto checked = current.try_check(index_source());
        CHECK_OK(checked);
        CHECK(!checked.value().has_value());
    }

    // A signature that does not verify is an error, never "up to date".
    {
        auto forged = updater.try_check(freshen::Source::manifest("https://test.invalid/m/2.0.0", "https://test.invalid/bad-sig"));
        CHECK_CODE(forged, ErrorCode::Signature);
    }

    // Plain HTTP and embedded credentials are refused before the transport is called.
    {
        const int before = transport->calls();
        auto http = updater.try_check(freshen::Source::manifest("http://test.invalid/m/2.0.0", "https://test.invalid/s/2.0.0"));
        CHECK_CODE(http, ErrorCode::Invalid);
        auto credentials =
            updater.try_check(freshen::Source::manifest("https://user:pass@test.invalid/m/2.0.0", "https://test.invalid/s/2.0.0"));
        CHECK_CODE(credentials, ErrorCode::Invalid);
        CHECK(transport->calls() == before);
    }

    // The sink enforces the size limit even though this transport ignores it.
    {
        auto huge = updater.try_check(freshen::Source::index("https://test.invalid/huge"));
        CHECK_CODE(huge, ErrorCode::SizeLimit);
    }

    // A transport that returns an error: IO, with the transport's message attached.
    {
        auto missing = updater.try_check(freshen::Source::index("https://test.invalid/nothing-here"));
        CHECK_CODE(missing, ErrorCode::IO);
        CHECK(missing.error().cause().find("no fixture") != std::string::npos);
    }

    // Cancellation before the operation starts.
    {
        FixtureTransport& counted = *transport;
        const int before = counted.calls();
        freshen::Cancellation cancellation;
        freshen::Cancellation alias = cancellation;  // copies share the flag
        alias.cancel();
        auto cancelled = updater.try_check(index_source(), &cancellation);
        CHECK_CODE(cancelled, ErrorCode::Cancelled);
        CHECK(counted.calls() == before);
    }

    // Preparing enforces the configured download ceiling.
    const std::string work = make_directory(fixtures, "prepare");
    {
        freshen::Updater tiny = make_updater(make_config(fixtures, transport, "1.0.0", 10));
        freshen::Candidate small = checked_candidate(tiny);
        auto prepared = tiny.try_prepare(small, work);
        CHECK_CODE(prepared, ErrorCode::SizeLimit);
        CHECK(count_entries(work) == 0);
    }

    // A transport that writes more than the signed package size is stopped by the sink.
    {
        auto greedy = std::make_shared<FixtureTransport>(fixtures);
        freshen::Updater over = make_updater(make_config(fixtures, greedy));
        freshen::Candidate candidate = checked_candidate(over);
        greedy->overrun_package = true;
        auto prepared = over.try_prepare(candidate, work);
        CHECK_CODE(prepared, ErrorCode::SizeLimit);
        CHECK(count_entries(work) == 0);
    }

    // Cancelling in the middle of a download stops it and leaves nothing behind.
    {
        auto cancelling = std::make_shared<FixtureTransport>(fixtures);
        freshen::Updater mid = make_updater(make_config(fixtures, cancelling));
        freshen::Candidate candidate = checked_candidate(mid);
        freshen::Cancellation cancellation;
        cancelling->after_first_chunk = [&cancellation] { cancellation.cancel(); };
        auto prepared = mid.try_prepare(candidate, work, &cancellation);
        CHECK_CODE(prepared, ErrorCode::Cancelled);
        CHECK(count_entries(work) == 0);
    }

    // Full prepare with progress; destroying the prepared update removes the staged files.
    {
        freshen::Candidate candidate = checked_candidate(updater);
        std::vector<freshen::Progress> events;
        {
            auto prepared = updater.try_prepare(candidate, work, nullptr, [&events](const freshen::Progress& progress) {
                events.push_back(progress);
            });
            CHECK_OK(prepared);
            CHECK(prepared.value().version() == "2.0.0");
            CHECK(prepared.value().notes() == "notes two");
            CHECK(prepared.value().history().size() == 2);
            bool verifying = false;
            bool extracting = false;
            std::uint64_t most = 0;
            bool known_total = false;
            for (const freshen::Progress& event : events) {
                verifying = verifying || event.kind == freshen::Progress::Kind::Verifying;
                extracting = extracting || (event.kind == freshen::Progress::Kind::Extracting && event.path == "application");
                if (event.kind == freshen::Progress::Kind::Downloading) {
                    most = std::max(most, event.bytes_received);
                    known_total = known_total || event.total_bytes.has_value();
                }
            }
            CHECK(verifying && extracting && known_total);
            CHECK(most == candidate.download_size());
            CHECK(events.back().kind == freshen::Progress::Kind::Ready);
            CHECK(count_entries(work) == 1);
        }
        CHECK(count_entries(work) == 0);
    }

    // Arming: an invalid policy is rejected without consuming the update; a real failure
    // consumes it, so a further attempt reports AlreadyConsumed.
    {
        freshen::Candidate candidate = checked_candidate(updater);
        auto prepared = updater.try_prepare(candidate, work);
        CHECK_OK(prepared);
        freshen::PreparedUpdate& update = prepared.value();
        const std::string root = make_directory(fixtures, "arm-root");

        freshen::InstallPolicy too_long = freshen::InstallPolicy::portable(root, "application", {"application"});
        too_long.exit_timeout = std::chrono::hours(2);
        auto rejected = update.try_arm(too_long, "/nonexistent-helper");
        CHECK_CODE(rejected, ErrorCode::InvalidArgument);

        freshen::InstallPolicy missing_root = freshen::InstallPolicy::portable(root + "/does-not-exist", "application", {"application"});
        auto failed = update.try_arm(missing_root, "/nonexistent-helper");
        CHECK(!failed.ok());
        CHECK(failed.error().code() != ErrorCode::AlreadyConsumed);
        auto again = update.try_arm(missing_root, "/nonexistent-helper");
        CHECK_CODE(again, ErrorCode::AlreadyConsumed);
    }

    // A moved-from updater reports an error instead of crashing.
    {
        freshen::Updater first = make_updater(make_config(fixtures, transport));
        freshen::Updater second = std::move(first);
        auto moved = first.try_check(index_source());  // NOLINT(bugprone-use-after-move): deliberate
        CHECK_CODE(moved, ErrorCode::InvalidArgument);
        auto fine = second.try_check(index_source());
        CHECK_OK(fine);
    }

    // An updater runs one operation at a time.
    {
        auto blocking = std::make_shared<FixtureTransport>(fixtures);
        std::promise<void> entered;
        std::promise<void> release;
        std::shared_future<void> release_future = release.get_future().share();
        blocking->before_download = [&entered, release_future] {
            static std::once_flag once;
            std::call_once(once, [&entered] { entered.set_value(); });
            release_future.wait();
        };
        freshen::Updater busy = make_updater(make_config(fixtures, blocking));
        std::thread worker([&busy] { (void)busy.try_check(index_source()); });
        CHECK(entered.get_future().wait_for(std::chrono::seconds(30)) == std::future_status::ready);
        auto overlapping = busy.try_check(index_source());
        CHECK_CODE(overlapping, ErrorCode::OperationInProgress);
        release.set_value();
        worker.join();
    }

    // Host helpers behave on an ordinary launch.
    {
        auto helper = freshen::host::run_helper({"--not-a-helper"});
        CHECK_OK(helper);
        CHECK(!helper.value());
        auto confirmed = freshen::host::confirm_startup(work);
        CHECK_OK(confirmed);
        CHECK(!confirmed.value());
        auto status = freshen::host::installation_status(work);
        CHECK_OK(status);
        CHECK(!status.value().has_value());
        const char* arguments[] = {"program", "--not-a-helper"};
        CHECK(!freshen::host::run_helper_if_requested(2, arguments));
    }

#if FRESHEN_HAS_EXCEPTIONS
    // The throwing API: failures are freshen::Error, and Result::value() throws too.
    {
        bool threw = false;
        try {
            (void)updater.check(freshen::Source::manifest("https://test.invalid/m/2.0.0", "https://test.invalid/bad-sig"));
        } catch (const freshen::Error& error) {
            threw = error.code() == ErrorCode::Signature;
        }
        CHECK(threw);
        CHECK(updater.check(index_source()).has_value());

        // Exceptions never cross into native code: a throwing transport or progress handler
        // is caught at the boundary and reported.
        auto exploding = std::make_shared<FixtureTransport>(fixtures);
        exploding->failure = FixtureTransport::Failure::ThrowStdException;
        freshen::Updater bad = make_updater(make_config(fixtures, exploding));
        auto failed = bad.try_check(index_source());
        CHECK_CODE(failed, ErrorCode::IO);
        CHECK(failed.error().cause().find("exploded") != std::string::npos);

        exploding->failure = FixtureTransport::Failure::ThrowFreshenError;
        auto typed = bad.try_check(index_source());
        CHECK_CODE(typed, ErrorCode::SizeLimit);  // a thrown freshen::Error keeps its code

        auto noisy = updater.try_check(index_source(), nullptr,
                                       [](const freshen::Progress&) { throw std::runtime_error("handler bug"); });
        CHECK_OK(noisy);  // the handler's exception is swallowed; the operation still succeeds
    }
#endif

    std::puts("selftest ok");
}

// ---- process scenarios ----

void run_parent(const std::string& fixtures, const std::string& root, const std::string& mode, const std::string& helper) {
    auto transport = std::make_shared<FixtureTransport>(fixtures);
    freshen::Updater updater = make_updater(make_config(fixtures, transport));
    auto checked = updater.try_check(freshen::Source::manifest("https://test.invalid/m/2.0.0", "https://test.invalid/s/2.0.0"));
    CHECK_OK(checked);
    CHECK(checked.value().has_value());
    freshen::Candidate candidate = std::move(*checked.value());

    const std::string work = make_directory(fixtures, "work");
    auto prepared = updater.try_prepare(candidate, work);
    CHECK_OK(prepared);

    freshen::InstallPolicy policy = freshen::InstallPolicy::portable(root, "application", {"application"});
    policy.confirmation_timeout = std::chrono::seconds(mode == "timeout" ? 1 : 30);
    const std::vector<std::string> relaunch = {"child", fixtures, root, mode};

    auto armed = prepared.value().try_arm(policy, helper, relaunch);
    CHECK_OK(armed);
    freshen::Handoff handoff = std::move(armed).value();
    CHECK(handoff.helper_process_id() > 0);

    // While the helper waits for this process to exit, nothing has changed.
    std::this_thread::sleep_for(std::chrono::milliseconds(200));
    CHECK(read_text(root + "/application") == "old application");

    if (mode == "busy") {
        // A second update cannot arm while the first holds the installation lock.
        auto second = updater.try_prepare(candidate, work);
        CHECK_OK(second);
        auto other = second.value().try_arm(policy, helper, relaunch);
        CHECK_CODE(other, ErrorCode::Busy);
    }
    if (mode == "cancel" || mode == "busy") {
        CHECK_OK(handoff.try_cancel());
        auto again = handoff.try_cancel();
        CHECK_CODE(again, ErrorCode::AlreadyConsumed);
    }
    // Destroying the handoff must NOT cancel a requested installation.
}

int run_child(const std::string& root, const std::string& mode) {
    if (mode == "fail") {
        return 1;  // exits without confirming: the helper must roll back
    }
    if (mode == "timeout") {
        sleep(3);  // stays alive without confirming: recorded as Failed, never killed
        return 0;
    }
    auto confirmed = freshen::host::confirm_startup(root);
    CHECK_OK(confirmed);
    CHECK(confirmed.value());
    return 0;
}

}  // namespace

int main(int argc, char** argv) {
    // Self-helper dispatch comes first, before anything else in the program.
    if (freshen::host::run_helper_if_requested(argc, argv)) {
        return 0;
    }
    if (argc == 3 && std::string(argv[1]) == "selftest") {
        selftest(argv[2]);
        return 0;
    }
    if (argc == 6 && std::string(argv[1]) == "parent") {
        run_parent(argv[2], argv[3], argv[4], argv[5]);
        return 0;
    }
    if (argc == 5 && std::string(argv[1]) == "child") {
        return run_child(argv[3], argv[4]);
    }
    std::fprintf(stderr, "usage: wrapper_test selftest DIR | parent DIR ROOT MODE HELPER | child DIR ROOT MODE\n");
    return 64;
}
