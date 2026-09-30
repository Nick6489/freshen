/*
 * C conformance harness for the freshen C ABI.
 *
 * One executable plays every role, exactly like a host application that uses
 * itself as the update helper:
 *
 *   harness selftest <fixtures>                          in-process ABI checks
 *   harness parent <fixtures> <root> <mode> <helper>     check, prepare, arm
 *   harness child <fixtures> <root> <mode>               the restarted application
 *   harness --freshen-apply ...                          the update helper
 *
 * Driven by tests/c_harness.rs, which builds the signed fixtures. Any failed
 * check prints the location and the last freshen error, then exits nonzero.
 */
#define _POSIX_C_SOURCE 200809L
#include "freshen.h"

#include <dirent.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#define CHECK(cond)                                                               \
    do {                                                                          \
        if (!(cond)) {                                                            \
            fprintf(stderr, "FAIL %s:%d: %s (last error: %s)\n", __FILE__,        \
                    __LINE__, #cond, freshen_last_error_message());               \
            exit(1);                                                              \
        }                                                                         \
    } while (0)

#define CHECK_CODE(expr, expected)                                                \
    do {                                                                          \
        int32_t rc_ = (expr);                                                     \
        if (rc_ != (expected)) {                                                  \
            fprintf(stderr, "FAIL %s:%d: %s returned %d, expected %d (%s)\n",     \
                    __FILE__, __LINE__, #expr, (int)rc_, (int)(expected),         \
                    freshen_last_error_message());                                \
            exit(1);                                                              \
        }                                                                         \
    } while (0)

static char *path_join(const char *dir, const char *name) {
    size_t length = strlen(dir) + strlen(name) + 2;
    char *out = malloc(length);
    CHECK(out != NULL);
    snprintf(out, length, "%s/%s", dir, name);
    return out;
}

static unsigned char *read_file(const char *dir, const char *name, size_t *length) {
    char *path = path_join(dir, name);
    FILE *file = fopen(path, "rb");
    CHECK(file != NULL);
    CHECK(fseek(file, 0, SEEK_END) == 0);
    long size = ftell(file);
    CHECK(size >= 0);
    CHECK(fseek(file, 0, SEEK_SET) == 0);
    unsigned char *data = malloc((size_t)size + 1);
    CHECK(data != NULL);
    CHECK(fread(data, 1, (size_t)size, file) == (size_t)size);
    fclose(file);
    free(path);
    *length = (size_t)size;
    return data;
}

static void sleep_ms(long milliseconds) {
    struct timespec pause = {milliseconds / 1000, (milliseconds % 1000) * 1000000L};
    nanosleep(&pause, NULL);
}

static int count_entries(const char *dir) {
    DIR *handle = opendir(dir);
    CHECK(handle != NULL);
    int count = 0;
    struct dirent *entry;
    while ((entry = readdir(handle)) != NULL) {
        if (strcmp(entry->d_name, ".") != 0 && strcmp(entry->d_name, "..") != 0) {
            count++;
        }
    }
    closedir(handle);
    return count;
}

/* ---- host transport: serves the signed fixtures, never touches the network ---- */

typedef struct {
    const char *dir;
    FreshenCancel *cancel_after_first_write;
    int calls;
    int overrun_package; /* write the package twice, ignoring `limit` */
} Host;

static const char *fixture_for(const char *name) {
    static const char *const table[][2] = {
        {"index", "index.json"},        {"m/1.5.0", "v1.5.0.manifest"},
        {"s/1.5.0", "v1.5.0.sig"},      {"m/2.0.0", "v2.0.0.manifest"},
        {"s/2.0.0", "v2.0.0.sig"},      {"bad-sig", "tampered.sig"},
        {"package", "package.zip"},     {"huge", "huge"},
    };
    for (size_t i = 0; i < sizeof table / sizeof table[0]; i++) {
        if (strcmp(name, table[i][0]) == 0) {
            return table[i][1];
        }
    }
    return NULL;
}

static int32_t host_download(void *user, const char *url, FreshenSink *sink, uint64_t limit) {
    Host *host = user;
    (void)limit; /* the sink enforces it; a careless host cannot exceed it */
    host->calls++;
    static const char prefix[] = "https://test.invalid/";
    if (strncmp(url, prefix, sizeof prefix - 1) != 0) {
        return FRESHEN_ERR_IO;
    }
    const char *file = fixture_for(url + sizeof prefix - 1);
    if (file == NULL) {
        return FRESHEN_ERR_IO;
    }
    char *path = path_join(host->dir, file);
    FILE *stream = fopen(path, "rb");
    free(path);
    if (stream == NULL) {
        return FRESHEN_ERR_IO;
    }
    if (strcmp(file, "huge") != 0) { /* "huge" deliberately declares no total */
        fseek(stream, 0, SEEK_END);
        int32_t rc = freshen_sink_set_total(sink, (uint64_t)ftell(stream));
        fseek(stream, 0, SEEK_SET);
        if (rc != FRESHEN_OK) {
            fclose(stream);
            return rc;
        }
    }
    unsigned char buffer[4096];
    size_t count;
    int first = 1;
    int passes = (host->overrun_package && strcmp(file, "package.zip") == 0) ? 2 : 1;
    for (int pass = 0; pass < passes; pass++) {
        fseek(stream, 0, SEEK_SET);
        while ((count = fread(buffer, 1, sizeof buffer, stream)) > 0) {
            int32_t rc = freshen_sink_write(sink, buffer, count);
            if (rc != FRESHEN_OK) {
                fclose(stream);
                return rc; /* propagate CANCELLED / SIZE_LIMIT unchanged */
            }
            if (first && host->cancel_after_first_write != NULL) {
                freshen_cancel_cancel(host->cancel_after_first_write);
            }
            first = 0;
        }
    }
    fclose(stream);
    return FRESHEN_OK;
}

/* ---- progress events ---- */

typedef struct {
    int kinds[512];
    int count;
    uint64_t max_received;
    char first_path[128];
    int saw_known_total;
} Events;

static void on_event(void *user, int32_t kind, uint64_t received, uint64_t total, const char *path) {
    Events *events = user;
    if (events->count < 512) {
        events->kinds[events->count++] = (int)kind;
    }
    if (kind == FRESHEN_EVENT_DOWNLOADING) {
        if (received > events->max_received) {
            events->max_received = received;
        }
        if (total != FRESHEN_TOTAL_UNKNOWN) {
            events->saw_known_total = 1;
        }
    }
    if (kind == FRESHEN_EVENT_EXTRACTING && path != NULL && events->first_path[0] == '\0') {
        snprintf(events->first_path, sizeof events->first_path, "%s", path);
    }
}

static int has_kind(const Events *events, int kind) {
    for (int i = 0; i < events->count; i++) {
        if (events->kinds[i] == kind) {
            return 1;
        }
    }
    return 0;
}

/* ---- construction helpers ---- */

static FreshenUpdater *make_updater(Host *host, const char *version, uint64_t max_download,
                                    const unsigned char *key) {
    FreshenTransport transport;
    memset(&transport, 0, sizeof transport);
    transport.struct_size = sizeof transport;
    transport.user = host;
    transport.download = host_download;

    FreshenUpdaterConfig config;
    memset(&config, 0, sizeof config);
    config.struct_size = sizeof config;
    config.product = "ffi-test";
    config.channel = "stable";
    config.current_version = version;
    config.target = "test-target";
    config.trusted_keys = key;
    config.trusted_key_count = 1;
    config.max_download = max_download;
    config.max_unpacked = 64u * 1024u * 1024u;
    config.transport = &transport; /* copied by freshen_updater_new */

    FreshenUpdater *updater = NULL;
    CHECK_CODE(freshen_updater_new(&config, &updater), FRESHEN_OK);
    CHECK(updater != NULL);
    return updater;
}

static FreshenSource index_source(const char *url) {
    FreshenSource source;
    memset(&source, 0, sizeof source);
    source.struct_size = sizeof source;
    source.kind = FRESHEN_SOURCE_INDEX;
    source.a = url;
    return source;
}

static FreshenSource manifest_source(const char *document, const char *signature) {
    FreshenSource source;
    memset(&source, 0, sizeof source);
    source.struct_size = sizeof source;
    source.kind = FRESHEN_SOURCE_MANIFEST;
    source.a = document;
    source.b = signature;
    return source;
}

static void expect_string(char *actual, const char *expected) {
    CHECK(actual != NULL);
    if (strcmp(actual, expected) != 0) {
        fprintf(stderr, "FAIL: got \"%s\", expected \"%s\"\n", actual, expected);
        exit(1);
    }
    freshen_string_free(actual);
}

/* ---- in-process ABI checks ---- */

static void selftest(const char *dir) {
    size_t key_length = 0;
    unsigned char *key = read_file(dir, "pubkey", &key_length);
    CHECK(key_length == 32);
    Host host = {dir, NULL, 0, 0};

    /* ABI identity */
    CHECK(freshen_abi_version() == FRESHEN_ABI_VERSION);
    CHECK(strlen(freshen_library_version()) > 0);

    /* Null and invalid arguments never crash; they report INVALID_ARGUMENT. */
    {
        FreshenUpdater *updater = (FreshenUpdater *)0x1;
        CHECK_CODE(freshen_updater_new(NULL, &updater), FRESHEN_ERR_INVALID_ARGUMENT);
        CHECK(updater == NULL);
        CHECK(strlen(freshen_last_error_message()) > 0);
        FreshenCandidate *candidate = NULL;
        CHECK_CODE(freshen_check(NULL, NULL, NULL, NULL, NULL, &candidate), FRESHEN_ERR_INVALID_ARGUMENT);
        CHECK(candidate == NULL);
        CHECK_CODE(freshen_sink_write(NULL, NULL, 0), FRESHEN_ERR_INVALID_ARGUMENT);
        CHECK_CODE(freshen_discard_finished_installation(NULL), FRESHEN_ERR_INVALID_ARGUMENT);
        CHECK(freshen_candidate_version(NULL) == NULL);
        CHECK(freshen_candidate_history_count(NULL) == 0);
        freshen_string_free(NULL);
        freshen_updater_free(NULL);
        freshen_candidate_free(NULL);
        freshen_prepared_free(NULL);
        freshen_handoff_free(NULL);
        freshen_cancel_free(NULL);
        freshen_cancel_cancel(NULL);
    }

    /* A config with a struct_size that is too small is rejected, not read past. */
    {
        FreshenUpdaterConfig tiny;
        memset(&tiny, 0, sizeof tiny);
        tiny.struct_size = 4;
        FreshenUpdater *updater = NULL;
        CHECK_CODE(freshen_updater_new(&tiny, &updater), FRESHEN_ERR_INVALID_ARGUMENT);
    }

    /* A signed index yields the newest release plus authenticated history. */
    FreshenUpdater *updater = make_updater(&host, "1.0.0", 512u * 1024u * 1024u, key);
    FreshenSource index = index_source("https://test.invalid/index");
    Events events;
    memset(&events, 0, sizeof events);
    FreshenCandidate *candidate = NULL;
    CHECK_CODE(freshen_check(updater, &index, NULL, on_event, &events, &candidate), FRESHEN_OK);
    CHECK(candidate != NULL);
    /* A check reports only that it started; download progress belongs to prepare. */
    CHECK(events.count == 1 && events.kinds[0] == FRESHEN_EVENT_CHECKING);
    expect_string(freshen_candidate_version(candidate), "2.0.0");
    expect_string(freshen_candidate_notes(candidate), "notes two");
    CHECK(freshen_candidate_download_size(candidate) > 0);
    CHECK(freshen_candidate_history_count(candidate) == 2);
    expect_string(freshen_candidate_history_version(candidate, 0), "1.5.0");
    expect_string(freshen_candidate_history_notes(candidate, 0), "notes one");
    expect_string(freshen_candidate_history_version(candidate, 1), "2.0.0");
    expect_string(freshen_candidate_history_notes(candidate, 1), "notes two");
    CHECK(freshen_candidate_history_version(candidate, 2) == NULL);

    /* Already current: OK with no candidate, distinct from any error. */
    {
        FreshenUpdater *current = make_updater(&host, "2.0.0", 1024, key);
        FreshenCandidate *none = (FreshenCandidate *)0x1;
        CHECK_CODE(freshen_check(current, &index, NULL, NULL, NULL, &none), FRESHEN_OK);
        CHECK(none == NULL);
        freshen_updater_free(current);
    }

    /* A signature that does not verify is an error, never "up to date". */
    {
        FreshenSource forged = manifest_source("https://test.invalid/m/2.0.0", "https://test.invalid/bad-sig");
        FreshenCandidate *none = NULL;
        CHECK_CODE(freshen_check(updater, &forged, NULL, NULL, NULL, &none), FRESHEN_ERR_SIGNATURE);
        CHECK(none == NULL);
    }

    /* Plain HTTP and embedded credentials are refused before the host is called. */
    {
        int before = host.calls;
        FreshenCandidate *none = NULL;
        FreshenSource http = manifest_source("http://test.invalid/m/2.0.0", "https://test.invalid/s/2.0.0");
        CHECK_CODE(freshen_check(updater, &http, NULL, NULL, NULL, &none), FRESHEN_ERR_INVALID);
        FreshenSource credentials = manifest_source("https://user:pass@test.invalid/m/2.0.0", "https://test.invalid/s/2.0.0");
        CHECK_CODE(freshen_check(updater, &credentials, NULL, NULL, NULL, &none), FRESHEN_ERR_INVALID);
        CHECK(host.calls == before);
    }

    /* The sink enforces the size limit even though this host ignores `limit`. */
    {
        FreshenSource huge = index_source("https://test.invalid/huge");
        FreshenCandidate *none = NULL;
        CHECK_CODE(freshen_check(updater, &huge, NULL, NULL, NULL, &none), FRESHEN_ERR_SIZE_LIMIT);
    }

    /* A host that fails reports a transport error with a message. */
    {
        FreshenSource missing = index_source("https://test.invalid/nothing-here");
        FreshenCandidate *none = NULL;
        CHECK_CODE(freshen_check(updater, &missing, NULL, NULL, NULL, &none), FRESHEN_ERR_IO);
        CHECK(strlen(freshen_last_error_message()) > 0);
    }

    /* Cancellation before the operation starts. */
    {
        FreshenCancel *cancel = freshen_cancel_new();
        CHECK(cancel != NULL);
        FreshenCancel *alias = freshen_cancel_clone(cancel); /* shares the flag */
        CHECK(alias != NULL);
        freshen_cancel_cancel(alias);
        FreshenCandidate *none = NULL;
        CHECK_CODE(freshen_check(updater, &index, cancel, NULL, NULL, &none), FRESHEN_ERR_CANCELLED);
        freshen_cancel_free(alias);
        freshen_cancel_free(cancel);
    }

    /* Preparing enforces the configured download ceiling. */
    char *work_template = path_join(dir, "prepare-XXXXXX");
    CHECK(mkdtemp(work_template) != NULL);
    {
        FreshenUpdater *tiny = make_updater(&host, "1.0.0", 10, key);
        FreshenCandidate *small = NULL;
        CHECK_CODE(freshen_check(tiny, &index, NULL, NULL, NULL, &small), FRESHEN_OK);
        FreshenPrepared *none = NULL;
        CHECK_CODE(freshen_prepare(tiny, small, work_template, NULL, NULL, NULL, &none), FRESHEN_ERR_SIZE_LIMIT);
        CHECK(none == NULL);
        freshen_candidate_free(small);
        freshen_updater_free(tiny);
    }

    /* A host that writes more than the signed package size is stopped by the
     * sink itself with SIZE_LIMIT. Without sink enforcement the surplus would
     * only surface later as a hash mismatch, so this pins the sink's behavior. */
    {
        Host greedy = {dir, NULL, 0, 1};
        FreshenUpdater *over = make_updater(&greedy, "1.0.0", 512u * 1024u * 1024u, key);
        FreshenCandidate *fresh = NULL;
        CHECK_CODE(freshen_check(over, &index, NULL, NULL, NULL, &fresh), FRESHEN_OK);
        FreshenPrepared *none = NULL;
        CHECK_CODE(freshen_prepare(over, fresh, work_template, NULL, NULL, NULL, &none), FRESHEN_ERR_SIZE_LIMIT);
        CHECK(none == NULL);
        CHECK(count_entries(work_template) == 0);
        freshen_candidate_free(fresh);
        freshen_updater_free(over);
    }

    /* Cancelling in the middle of a download stops it. */
    {
        Host cancelling = {dir, NULL, 0, 0};
        FreshenCancel *cancel = freshen_cancel_new();
        FreshenUpdater *mid = make_updater(&cancelling, "1.0.0", 512u * 1024u * 1024u, key);
        FreshenCandidate *fresh = NULL;
        CHECK_CODE(freshen_check(mid, &index, NULL, NULL, NULL, &fresh), FRESHEN_OK);
        /* From now on the host trips the flag after the first chunk it writes,
         * so the cancel arrives while the package is still downloading. */
        cancelling.cancel_after_first_write = cancel;
        FreshenPrepared *none = NULL;
        CHECK_CODE(freshen_prepare(mid, fresh, work_template, cancel, NULL, NULL, &none), FRESHEN_ERR_CANCELLED);
        CHECK(none == NULL);
        CHECK(count_entries(work_template) == 0); /* nothing left behind */
        freshen_candidate_free(fresh);
        freshen_updater_free(mid);
        freshen_cancel_free(cancel);
    }

    /* Full prepare with progress, then dropping it removes the staged files. */
    {
        Events prepare_events;
        memset(&prepare_events, 0, sizeof prepare_events);
        FreshenPrepared *prepared = NULL;
        CHECK_CODE(freshen_prepare(updater, candidate, work_template, NULL, on_event, &prepare_events, &prepared), FRESHEN_OK);
        CHECK(prepared != NULL);
        CHECK(prepare_events.saw_known_total);
        CHECK(prepare_events.max_received == freshen_candidate_download_size(candidate));
        CHECK(has_kind(&prepare_events, FRESHEN_EVENT_VERIFYING));
        CHECK(has_kind(&prepare_events, FRESHEN_EVENT_EXTRACTING));
        CHECK(strcmp(prepare_events.first_path, "application") == 0);
        CHECK(prepare_events.kinds[prepare_events.count - 1] == FRESHEN_EVENT_READY);
        expect_string(freshen_prepared_version(prepared), "2.0.0");
        expect_string(freshen_prepared_notes(prepared), "notes two");
        CHECK(freshen_prepared_history_count(prepared) == 2);
        expect_string(freshen_prepared_history_notes(prepared, 0), "notes one");
        CHECK(count_entries(work_template) == 1);

        /* A bad policy fails, and arming consumes the prepared update regardless. */
        FreshenInstallPolicy policy;
        memset(&policy, 0, sizeof policy);
        policy.struct_size = sizeof policy;
        policy.layout = 99;
        FreshenHandoff *handoff = NULL;
        CHECK_CODE(freshen_arm(prepared, &policy, "/nonexistent", NULL, 0, &handoff), FRESHEN_ERR_INVALID_ARGUMENT);
        CHECK(handoff == NULL);
        CHECK(freshen_prepared_version(prepared) == NULL);
        CHECK_CODE(freshen_arm(prepared, &policy, "/nonexistent", NULL, 0, &handoff), FRESHEN_ERR_INVALID_ARGUMENT);
        freshen_prepared_free(prepared);
        CHECK(count_entries(work_template) == 0);
    }

    /* Installation status of a directory that never had an update. */
    {
        int32_t state = -1;
        char *version = (char *)0x1;
        CHECK_CODE(freshen_installation_status(work_template, &state, &version, NULL), FRESHEN_OK);
        CHECK(state == FRESHEN_STATE_NONE);
        CHECK(version == NULL);
    }

    freshen_candidate_free(candidate);
    freshen_updater_free(updater);
    free(work_template);
    free(key);
    puts("selftest ok");
}

/* ---- process scenarios ---- */

static void run_parent(const char *dir, const char *root, const char *mode, const char *helper) {
    size_t key_length = 0;
    unsigned char *key = read_file(dir, "pubkey", &key_length);
    Host host = {dir, NULL, 0, 0};
    FreshenUpdater *updater = make_updater(&host, "1.0.0", 512u * 1024u * 1024u, key);
    FreshenSource source = manifest_source("https://test.invalid/m/2.0.0", "https://test.invalid/s/2.0.0");
    FreshenCandidate *candidate = NULL;
    CHECK_CODE(freshen_check(updater, &source, NULL, NULL, NULL, &candidate), FRESHEN_OK);
    CHECK(candidate != NULL);

    char *work = path_join(dir, "work");
    if (mkdir(work, 0700) != 0) {
        CHECK(errno == EEXIST);
    }
    FreshenPrepared *prepared = NULL;
    CHECK_CODE(freshen_prepare(updater, candidate, work, NULL, NULL, NULL, &prepared), FRESHEN_OK);

    const char *owned[] = {"application"};
    FreshenInstallPolicy policy;
    memset(&policy, 0, sizeof policy);
    policy.struct_size = sizeof policy;
    policy.layout = FRESHEN_LAYOUT_PORTABLE;
    policy.root = root;
    policy.executable = "application";
    policy.owned_files = owned;
    policy.owned_file_count = 1;
    policy.confirmation_timeout_secs = strcmp(mode, "timeout") == 0 ? 1 : 30;
    const char *relaunch[] = {"child", dir, root, mode};

    FreshenHandoff *handoff = NULL;
    CHECK_CODE(freshen_arm(prepared, &policy, helper, relaunch, 4, &handoff), FRESHEN_OK);
    CHECK(handoff != NULL);
    CHECK(freshen_handoff_helper_pid(handoff) > 0);
    CHECK(freshen_prepared_version(prepared) == NULL); /* consumed */

    /* While the helper waits for this process to exit, nothing has changed. */
    sleep_ms(200);
    size_t length = 0;
    unsigned char *current = read_file(root, "application", &length);
    CHECK(length == strlen("old application") && memcmp(current, "old application", length) == 0);
    free(current);

    if (strcmp(mode, "busy") == 0) {
        /* A second update cannot arm while the first holds the installation lock. */
        FreshenPrepared *second = NULL;
        CHECK_CODE(freshen_prepare(updater, candidate, work, NULL, NULL, NULL, &second), FRESHEN_OK);
        FreshenHandoff *other = NULL;
        CHECK_CODE(freshen_arm(second, &policy, helper, relaunch, 4, &other), FRESHEN_ERR_BUSY);
        CHECK(other == NULL);
        freshen_prepared_free(second);
    }
    if (strcmp(mode, "cancel") == 0 || strcmp(mode, "busy") == 0) {
        CHECK_CODE(freshen_handoff_cancel(handoff), FRESHEN_OK);
        CHECK_CODE(freshen_handoff_cancel(handoff), FRESHEN_ERR_INVALID_ARGUMENT); /* already consumed */
    }
    /* Freeing the handle must NOT cancel a requested installation. */
    freshen_handoff_free(handoff);
    freshen_prepared_free(prepared);
    freshen_candidate_free(candidate);
    freshen_updater_free(updater);
    free(work);
    free(key);
}

static int run_child(const char *root, const char *mode) {
    if (strcmp(mode, "fail") == 0) {
        return 1; /* exits without confirming: the helper must roll back */
    }
    if (strcmp(mode, "timeout") == 0) {
        sleep(3); /* stays alive without confirming: recorded as Failed, never killed */
        return 0;
    }
    bool confirmed = false;
    CHECK_CODE(freshen_confirm_startup(root, &confirmed), FRESHEN_OK);
    CHECK(confirmed);
    return 0;
}

int main(int argc, char **argv) {
    /* Self-helper dispatch comes first, before anything else in the program. */
    bool handled = false;
    CHECK_CODE(freshen_run_helper((size_t)(argc - 1), (const char *const *)(argv + 1), &handled), FRESHEN_OK);
    if (handled) {
        return 0;
    }
    if (argc == 3 && strcmp(argv[1], "selftest") == 0) {
        selftest(argv[2]);
        return 0;
    }
    if (argc == 6 && strcmp(argv[1], "parent") == 0) {
        run_parent(argv[2], argv[3], argv[4], argv[5]);
        return 0;
    }
    if (argc == 5 && strcmp(argv[1], "child") == 0) {
        return run_child(argv[3], argv[4]);
    }
    fprintf(stderr, "usage: harness selftest DIR | parent DIR ROOT MODE HELPER | child DIR ROOT MODE\n");
    return 64;
}
