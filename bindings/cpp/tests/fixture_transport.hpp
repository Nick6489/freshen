// A Transport that serves the signed fixtures written by the make_fixtures example under
// https://test.invalid/, never touching the network. Knobs let tests misbehave on purpose.
#ifndef FRESHEN_CPP_TESTS_FIXTURE_TRANSPORT_HPP
#define FRESHEN_CPP_TESTS_FIXTURE_TRANSPORT_HPP

#include "freshen/freshen.hpp"

#include <algorithm>
#include <atomic>
#include <cstdint>
#include <fstream>
#include <functional>
#include <iterator>
#include <string>
#include <vector>

class FixtureTransport : public freshen::Transport {
public:
    enum class Failure { None, ThrowStdException, ThrowFreshenError };

    explicit FixtureTransport(std::string directory) : directory_(std::move(directory)) {}

    int calls() const { return calls_.load(); }

    /// Writes the package twice, ignoring the limit, to prove the sink enforces it.
    bool overrun_package = false;
    /// Invoked after the first chunk of every download (for example to cancel mid-download).
    std::function<void()> after_first_chunk;
    /// Invoked at the start of every download (for example to block).
    std::function<void()> before_download;
    /// Only honored when exceptions are available.
    Failure failure = Failure::None;

    freshen::Status download(const std::string& url, freshen::DownloadSink& sink, std::uint64_t) override {
        ++calls_;
        if (before_download) {
            before_download();
        }
#if FRESHEN_HAS_EXCEPTIONS
        if (failure == Failure::ThrowStdException) {
            throw std::runtime_error("the transport exploded");
        }
        if (failure == Failure::ThrowFreshenError) {
            throw freshen::Error(freshen::ErrorCode::SizeLimit, "thrown size limit");
        }
#endif
        const std::string prefix = "https://test.invalid/";
        if (url.compare(0, prefix.size(), prefix) != 0) {
            return freshen::Error(freshen::ErrorCode::IO, "unexpected host in " + url);
        }
        const std::string name = file_for(url.substr(prefix.size()));
        if (name.empty()) {
            return freshen::Error(freshen::ErrorCode::IO, "no fixture for " + url);
        }
        std::ifstream stream(directory_ + "/" + name, std::ios::binary);
        if (!stream) {
            return freshen::Error(freshen::ErrorCode::IO, "cannot open fixture " + name);
        }
        const std::vector<char> data((std::istreambuf_iterator<char>(stream)), std::istreambuf_iterator<char>());
        if (name != "huge") {
            // "huge" deliberately declares no total.
            if (freshen::Status total = sink.set_total(data.size()); !total.ok()) {
                return total;
            }
        }
        const int passes = (overrun_package && name == "package.zip") ? 2 : 1;
        bool first = true;
        for (int pass = 0; pass < passes; ++pass) {
            for (std::size_t offset = 0; offset < data.size(); offset += 4096) {
                const std::size_t count = std::min<std::size_t>(4096, data.size() - offset);
                if (freshen::Status written = sink.write(data.data() + offset, count); !written.ok()) {
                    return written;  // propagate cancelled / size-limit unchanged
                }
                if (first) {
                    first = false;
                    if (after_first_chunk) {
                        after_first_chunk();
                    }
                }
            }
        }
        return freshen::Status();
    }

private:
    static std::string file_for(const std::string& path) {
        static const struct {
            const char* url;
            const char* file;
        } table[] = {
            {"index", "index.json"},         {"m/1.5.0", "v1.5.0.manifest"}, {"s/1.5.0", "v1.5.0.sig"},
            {"m/2.0.0", "v2.0.0.manifest"},  {"s/2.0.0", "v2.0.0.sig"},      {"bad-sig", "tampered.sig"},
            {"package", "package.zip"},      {"huge", "huge"},
        };
        for (const auto& entry : table) {
            if (path == entry.url) {
                return entry.file;
            }
        }
        return {};
    }

    std::string directory_;
    std::atomic<int> calls_{0};
};

#endif
