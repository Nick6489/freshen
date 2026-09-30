// Proves freshen.hpp works in a program built WITHOUT exceptions (-fno-exceptions), using only
// the non-throwing try_*() API. Built by ffi/tests/cpp_wrapper.rs with -fno-exceptions
// -DFRESHEN_NO_EXCEPTIONS and run against signed fixtures.
//
//   no_exceptions FIXTURES WORK_DIRECTORY
#include "fixture_transport.hpp"

#include <cstdio>
#include <fstream>
#include <iterator>
#include <memory>
#include <string>

static_assert(FRESHEN_HAS_EXCEPTIONS == 0, "this file must be built without exceptions");

#define REQUIRE(condition)                                                                       \
    do {                                                                                         \
        if (!(condition)) {                                                                      \
            std::fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #condition);            \
            return 1;                                                                            \
        }                                                                                        \
    } while (false)

int main(int argc, char** argv) {
    if (argc != 3) {
        std::fprintf(stderr, "usage: no_exceptions FIXTURES WORK_DIRECTORY\n");
        return 64;
    }
    const std::string fixtures = argv[1];
    const std::string work = argv[2];

    std::ifstream key_stream(fixtures + "/pubkey", std::ios::binary);
    const std::string key_bytes((std::istreambuf_iterator<char>(key_stream)), std::istreambuf_iterator<char>());
    REQUIRE(key_bytes.size() == 32);

    freshen::UpdaterConfig config;
    config.product = "ffi-test";
    config.channel = "stable";
    config.current_version = "1.0.0";
    config.target = "test-target";
    std::array<std::uint8_t, 32> key{};
    for (std::size_t index = 0; index < 32; ++index) {
        key[index] = static_cast<std::uint8_t>(key_bytes[index]);
    }
    config.trusted_keys = {key};
    config.transport = std::make_shared<FixtureTransport>(fixtures);

    auto created = freshen::Updater::try_create(config);
    REQUIRE(created.ok());
    freshen::Updater updater = std::move(created).value();

    auto checked = updater.try_check(freshen::Source::index("https://test.invalid/index"));
    REQUIRE(checked.ok());
    REQUIRE(checked.value().has_value());
    const freshen::Candidate& candidate = *checked.value();
    REQUIRE(candidate.version() == "2.0.0");
    REQUIRE(candidate.history().size() == 2);

    // Errors are values, not exceptions.
    auto forged = updater.try_check(freshen::Source::manifest("https://test.invalid/m/2.0.0", "https://test.invalid/bad-sig"));
    REQUIRE(!forged.ok());
    REQUIRE(forged.error().code() == freshen::ErrorCode::Signature);

    auto prepared = updater.try_prepare(candidate, work);
    REQUIRE(prepared.ok());
    REQUIRE(prepared.value().version() == "2.0.0");

    std::puts("no-exceptions consumer ok");
    return 0;
}
