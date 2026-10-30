// A C++ consumer of the freshen C ABI: what a Qt or Win32 application would write.
//
// It proves the generated header is valid C++ (no C-only constructs, correct
// extern "C" linkage) and that the API is pleasant to wrap with RAII. It is built
// under -Wall -Wextra -Werror -pedantic for several language standards by
// tests/cpp_header.rs, and the C++17 build is run against signed fixtures.
//
//   consumer FIXTURES_DIRECTORY WORK_DIRECTORY
#include "freshen.h"

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <memory>
#include <string>
#include <vector>

static_assert(FRESHEN_ABI_VERSION == 1u, "these bindings are written for C ABI version 1");
static_assert(FRESHEN_OK == 0, "success is zero");
static_assert(FRESHEN_ERR_CANCELLED != FRESHEN_ERR_SIGNATURE, "distinct status codes");

namespace {

// RAII owners, one per handle type. Freeing null is a documented no-op.
struct StringFree {
    void operator()(char *text) const noexcept { freshen_string_free(text); }
};
struct UpdaterFree {
    void operator()(FreshenUpdater *value) const noexcept { freshen_updater_free(value); }
};
struct CandidateFree {
    void operator()(FreshenCandidate *value) const noexcept { freshen_candidate_free(value); }
};
struct PreparedFree {
    void operator()(FreshenPrepared *value) const noexcept { freshen_prepared_free(value); }
};
struct CancelFree {
    void operator()(FreshenCancel *value) const noexcept { freshen_cancel_free(value); }
};

using OwnedString = std::unique_ptr<char, StringFree>;
using OwnedUpdater = std::unique_ptr<FreshenUpdater, UpdaterFree>;
using OwnedCandidate = std::unique_ptr<FreshenCandidate, CandidateFree>;
using OwnedPrepared = std::unique_ptr<FreshenPrepared, PreparedFree>;
using OwnedCancel = std::unique_ptr<FreshenCancel, CancelFree>;

int failures = 0;

#define EXPECT(condition)                                                                \
    do {                                                                                 \
        if (!(condition)) {                                                              \
            std::fprintf(stderr, "FAIL %s:%d: %s (last error: %s)\n", __FILE__, __LINE__, \
                         #condition, freshen_last_error_message());                      \
            ++failures;                                                                  \
        }                                                                                \
    } while (false)

// The host transport: serves the signed fixtures. Callbacks are called from C, so
// they have C linkage, must not throw, and report failure through a status code.
struct Host {
    std::string directory;
    int calls = 0;
};

const char *fixture_for(const std::string &path) {
    static const struct {
        const char *url;
        const char *file;
    } table[] = {
        {"index", "index.json"},     {"m/1.5.0", "v1.5.0.manifest"}, {"s/1.5.0", "v1.5.0.sig"},
        {"m/2.0.0", "v2.0.0.manifest"}, {"s/2.0.0", "v2.0.0.sig"},   {"package", "package.zip"},
    };
    for (const auto &entry : table) {
        if (path == entry.url) {
            return entry.file;
        }
    }
    return nullptr;
}

}  // namespace

extern "C" {

static int32_t host_download(void *user, const char *url, FreshenSink *sink, uint64_t limit) {
    (void)limit;  // the sink enforces it
    Host *host = static_cast<Host *>(user);
    ++host->calls;
    const std::string prefix = "https://test.invalid/";
    const std::string text = url;
    if (text.compare(0, prefix.size(), prefix) != 0) {
        return FRESHEN_ERR_IO;
    }
    const char *file = fixture_for(text.substr(prefix.size()));
    if (file == nullptr) {
        return FRESHEN_ERR_IO;
    }
    std::ifstream stream(host->directory + "/" + file, std::ios::binary);
    if (!stream) {
        return FRESHEN_ERR_IO;
    }
    std::vector<char> contents((std::istreambuf_iterator<char>(stream)),
                               std::istreambuf_iterator<char>());
    int32_t status = freshen_sink_set_total(sink, contents.size());
    if (status != FRESHEN_OK) {
        return status;
    }
    for (std::size_t offset = 0; offset < contents.size(); offset += 4096) {
        const std::size_t count = std::min<std::size_t>(4096, contents.size() - offset);
        status = freshen_sink_write(sink, reinterpret_cast<const uint8_t *>(contents.data()) + offset,
                                    count);
        if (status != FRESHEN_OK) {
            return status;  // propagate cancelled / size-limit unchanged
        }
    }
    return FRESHEN_OK;
}

}  // extern "C"

int main(int argc, char **argv) {
    if (argc != 3) {
        std::fprintf(stderr, "usage: consumer FIXTURES_DIRECTORY WORK_DIRECTORY\n");
        return 64;
    }
    Host host;
    host.directory = argv[1];
    const std::string work = argv[2];

    EXPECT(freshen_abi_version() == FRESHEN_ABI_VERSION);

    // Cancellation handles: a clone shares the flag.
    {
        OwnedCancel cancel(freshen_cancel_new());
        OwnedCancel alias(freshen_cancel_clone(cancel.get()));
        EXPECT(cancel != nullptr && alias != nullptr);
        freshen_cancel_cancel(alias.get());
    }

    // Value-initialized structs, with struct_size set for forward compatibility.
    std::vector<uint8_t> key;
    {
        std::ifstream stream(host.directory + "/pubkey", std::ios::binary);
        key.assign(std::istreambuf_iterator<char>(stream), std::istreambuf_iterator<char>());
    }
    EXPECT(key.size() == 32);

    FreshenTransport transport = {};
    transport.struct_size = sizeof transport;
    transport.user = &host;
    transport.download = host_download;

    FreshenUpdaterConfig config = {};
    config.struct_size = sizeof config;
    config.product = "ffi-test";
    config.channel = "stable";
    config.current_version = "1.0.0";
    config.target = "test-target";
    config.trusted_keys = key.data();
    config.trusted_key_count = 1;
    config.max_download = 512ULL * 1024 * 1024;
    config.max_unpacked = 64ULL * 1024 * 1024;
    config.transport = &transport;

    FreshenUpdater *raw_updater = nullptr;
    EXPECT(freshen_updater_new(&config, &raw_updater) == FRESHEN_OK);
    OwnedUpdater updater(raw_updater);
    if (!updater) {
        return 1;
    }

    // Check against a signed index.
    FreshenSource source = {};
    source.struct_size = sizeof source;
    source.kind = FRESHEN_SOURCE_INDEX;
    source.a = "https://test.invalid/index";
    FreshenCandidate *raw_candidate = nullptr;
    EXPECT(freshen_check(updater.get(), &source, nullptr, nullptr, nullptr, &raw_candidate) ==
           FRESHEN_OK);
    OwnedCandidate candidate(raw_candidate);
    if (!candidate) {
        return 1;
    }
    OwnedString version(freshen_candidate_version(candidate.get()));
    EXPECT(version && std::string(version.get()) == "2.0.0");
    OwnedString notes(freshen_candidate_history_notes(candidate.get(), 0));
    EXPECT(notes && std::string(notes.get()) == "notes one");
    EXPECT(freshen_candidate_history_count(candidate.get()) == 2);
    EXPECT(freshen_candidate_history_version(candidate.get(), 9) == nullptr);

    // A bad argument is reported, not crashed on.
    FreshenUpdater *none = reinterpret_cast<FreshenUpdater *>(0x1);
    EXPECT(freshen_updater_new(nullptr, &none) == FRESHEN_ERR_INVALID_ARGUMENT);
    EXPECT(none == nullptr);

    // Prepare, then drop it: the staged files go away with the owner.
    FreshenPrepared *raw_prepared = nullptr;
    EXPECT(freshen_prepare(updater.get(), candidate.get(), work.c_str(), nullptr, nullptr, nullptr,
                           &raw_prepared) == FRESHEN_OK);
    OwnedPrepared prepared(raw_prepared);
    OwnedString prepared_version(freshen_prepared_version(prepared.get()));
    EXPECT(prepared_version && std::string(prepared_version.get()) == "2.0.0");
    EXPECT(host.calls >= 5);

    if (failures == 0) {
        std::puts("cpp consumer ok");
    }
    return failures == 0 ? 0 : 1;
}
