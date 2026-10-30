// freshen.hpp inside a real Qt 6 program: the pattern a Qt desktop client would use.
//
//   * A QObject owns the Updater and exposes the update flow as signals.
//   * Blocking operations run on Qt's thread pool via QtConcurrent, never on the GUI thread.
//   * Progress and results are emitted from the worker; because the QObject lives on the
//     main thread, Qt queues those signals, so slots run on the GUI thread.
//   * Cancellation is requested from the GUI thread while a worker is downloading.
//
// Qt headers come first on purpose: Qt defines macros (signals, slots, emit, foreach) and
// freshen.hpp must coexist with them.
#include <QtConcurrent>
#include <QtCore>

#include "fixture_transport.hpp"

#include <cstdio>
#include <future>
#include <memory>
#include <optional>

namespace {

int failures = 0;

#define EXPECT(condition)                                                                       \
    do {                                                                                        \
        if (!(condition)) {                                                                     \
            std::fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #condition);          \
            ++failures;                                                                         \
        }                                                                                       \
    } while (false)

}  // namespace

/// The update flow as Qt signals. Everything blocking happens on a pool thread.
class UpdateController : public QObject {
    Q_OBJECT
public:
    explicit UpdateController(freshen::Updater& updater) : updater_(updater) {}

    void check(const QUrl& index) {
        const std::string url = index.toString().toStdString();
        (void)QtConcurrent::run([this, url] {
            auto checked = updater_.try_check(freshen::Source::index(url), &cancellation_, progress_handler());
            if (!checked.ok()) {
                emit failed(static_cast<int>(checked.error().code()), QString::fromUtf8(checked.error().what()));
                return;
            }
            if (!checked.value().has_value()) {
                emit checkFinished(false, QString(), QString());
                return;
            }
            const QString version = QString::fromStdString(checked.value()->version());
            const QString notes = QString::fromStdString(checked.value()->notes());
            {
                std::lock_guard<std::mutex> lock(mutex_);
                candidate_ = std::move(checked.value());
            }
            emit checkFinished(true, version, notes);
        });
    }

    void checkManifest(const QUrl& manifest, const QUrl& signature) {
        const std::string document = manifest.toString().toStdString();
        const std::string sig = signature.toString().toStdString();
        (void)QtConcurrent::run([this, document, sig] {
            auto checked = updater_.try_check(freshen::Source::manifest(document, sig), &cancellation_);
            if (!checked.ok()) {
                emit failed(static_cast<int>(checked.error().code()), QString::fromUtf8(checked.error().what()));
            }
        });
    }

    void prepare(const QString& directory) {
        const std::string dir = directory.toStdString();
        (void)QtConcurrent::run([this, dir] {
            std::lock_guard<std::mutex> lock(mutex_);
            if (!candidate_.has_value()) {
                emit failed(-1, QStringLiteral("no candidate"));
                return;
            }
            auto prepared = updater_.try_prepare(*candidate_, dir, &cancellation_, progress_handler());
            if (!prepared.ok()) {
                emit failed(static_cast<int>(prepared.error().code()), QString::fromUtf8(prepared.error().what()));
                return;
            }
            emit prepareFinished(QString::fromStdString(prepared.value().version()));
        });
    }

    /// Safe from the GUI thread while a worker is running.
    void cancel() { cancellation_.cancel(); }

    /// Starts over with a fresh cancellation flag (a cancelled flag stays cancelled).
    void reset_cancellation() { cancellation_ = freshen::Cancellation(); }

signals:
    void progress(quint64 received, quint64 total);
    void checkFinished(bool updateAvailable, const QString& version, const QString& notes);
    void prepareFinished(const QString& version);
    void failed(int code, const QString& message);

private:
    freshen::ProgressHandler progress_handler() {
        return [this](const freshen::Progress& event) {
            if (event.kind == freshen::Progress::Kind::Downloading) {
                emit progress(event.bytes_received, event.total_bytes.value_or(0));
            }
        };
    }

    freshen::Updater& updater_;
    freshen::Cancellation cancellation_;
    std::mutex mutex_;
    std::optional<freshen::Candidate> candidate_;
};

namespace {

/// Runs the event loop until `done` is true or the timeout passes.
bool spin_until(const std::function<bool()>& done, int timeout_ms = 60000) {
    QElapsedTimer timer;
    timer.start();
    while (!done() && timer.elapsed() < timeout_ms) {
        QCoreApplication::processEvents(QEventLoop::AllEvents, 50);
        QThread::msleep(5);
    }
    return done();
}

}  // namespace

int main(int argc, char** argv) {
    QCoreApplication app(argc, argv);
    if (argc != 3) {
        std::fprintf(stderr, "usage: qt_test FIXTURES WORK_DIRECTORY\n");
        return 64;
    }
    const QString fixtures = QString::fromLocal8Bit(argv[1]);
    const QString work = QString::fromLocal8Bit(argv[2]);

    QFile key_file(fixtures + QStringLiteral("/pubkey"));
    if (!key_file.open(QIODevice::ReadOnly)) {
        std::fprintf(stderr, "cannot read the public key fixture\n");
        return 1;
    }
    const QByteArray key_bytes = key_file.readAll();
    EXPECT(key_bytes.size() == 32);

    auto transport = std::make_shared<FixtureTransport>(fixtures.toStdString());
    freshen::UpdaterConfig config;
    config.product = "ffi-test";
    config.channel = "stable";
    config.current_version = "1.0.0";
    config.target = "test-target";
    std::array<std::uint8_t, 32> key{};
    for (int index = 0; index < 32 && index < key_bytes.size(); ++index) {
        key[static_cast<std::size_t>(index)] = static_cast<std::uint8_t>(key_bytes[index]);
    }
    config.trusted_keys = {key};
    config.transport = transport;
    auto created = freshen::Updater::try_create(config);
    if (!created.ok()) {
        std::fprintf(stderr, "cannot create the updater: %s\n", created.error().what());
        return 1;
    }
    freshen::Updater updater = std::move(created).value();

    UpdateController controller(updater);
    bool checked = false;
    bool available = false;
    QString version;
    QString notes;
    bool prepared = false;
    QString prepared_version;
    std::optional<int> failure_code;
    QString failure_message;
    int progress_count = 0;
    bool progress_on_gui_thread = true;
    QObject::connect(&controller, &UpdateController::checkFinished, &app,
                     [&](bool has_update, const QString& v, const QString& n) {
                         checked = true;
                         available = has_update;
                         version = v;
                         notes = n;
                     });
    QObject::connect(&controller, &UpdateController::prepareFinished, &app, [&](const QString& v) {
        prepared = true;
        prepared_version = v;
    });
    QObject::connect(&controller, &UpdateController::failed, &app, [&](int code, const QString& message) {
        failure_code = code;
        failure_message = message;
    });
    QObject::connect(&controller, &UpdateController::progress, &app, [&](quint64, quint64) {
        ++progress_count;
        // Signals emitted from a pool thread must arrive on the GUI thread.
        progress_on_gui_thread = progress_on_gui_thread && QThread::currentThread() == app.thread();
    });

    // 1. Check on a worker thread; the result arrives as a signal on the GUI thread.
    controller.check(QUrl(QStringLiteral("https://test.invalid/index")));
    EXPECT(spin_until([&] { return checked; }));
    EXPECT(available && version == QStringLiteral("2.0.0") && notes == QStringLiteral("notes two"));
    EXPECT(!failure_code.has_value());

    // 2. Prepare; progress is reported through signals delivered on the GUI thread.
    controller.prepare(work);
    EXPECT(spin_until([&] { return prepared; }));
    EXPECT(prepared_version == QStringLiteral("2.0.0"));
    EXPECT(progress_count > 0 && progress_on_gui_thread);

    // 3. A failure is reported as a signal, not an exception.
    failure_code.reset();
    controller.checkManifest(QUrl(QStringLiteral("https://test.invalid/m/2.0.0")),
                             QUrl(QStringLiteral("https://test.invalid/bad-sig")));
    EXPECT(spin_until([&] { return failure_code.has_value(); }));
    EXPECT(failure_code == static_cast<int>(freshen::ErrorCode::Signature));

    // 4. The GUI thread cancels a download in flight. The worker pauses after its first chunk
    //    until the GUI thread has asked, so the outcome does not depend on timing.
    controller.reset_cancellation();
    failure_code.reset();
    prepared = false;
    std::promise<void> reached;
    std::promise<void> released;
    std::shared_future<void> release_future = released.get_future().share();
    transport->after_first_chunk = [&reached, release_future] {
        static std::once_flag once;
        std::call_once(once, [&reached] { reached.set_value(); });
        release_future.wait();
    };
    controller.prepare(work);
    EXPECT(reached.get_future().wait_for(std::chrono::seconds(30)) == std::future_status::ready);
    controller.cancel();  // from the GUI thread, while the worker is mid-download
    released.set_value();
    EXPECT(spin_until([&] { return failure_code.has_value(); }));
    EXPECT(failure_code == static_cast<int>(freshen::ErrorCode::Cancelled));
    EXPECT(!prepared);

    // Wait for the pool to go idle so nothing outlives the objects above.
    QThreadPool::globalInstance()->waitForDone();
    if (failures == 0) {
        std::puts("qt integration ok");
    }
    return failures == 0 ? 0 : 1;
}

#include "qt_test.moc"
