#import <Foundation/Foundation.h>

#import "FSHTypes.h"

NS_ASSUME_NONNULL_BEGIN

/// Self-helper dispatch for an application whose own executable doubles as the
/// update helper. Call this first in `main`, before any UI or single-instance
/// lock:
///
///     int main(int argc, const char *argv[]) {
///         if (FSHRunHelperIfRequested(argc, argv)) { return 0; }
///         return NSApplicationMain(argc, argv);
///     }
///
/// @return `YES` when this process was launched as the update helper and has
///   finished; return from `main` without starting the application. `NO` for an
///   ordinary launch, or if the helper request failed (the failure is written to stderr).
FOUNDATION_EXPORT BOOL FSHRunHelperIfRequested(int argc, const char *_Nonnull const *_Nonnull argv);

/// Process-level entry points: helper dispatch, startup confirmation and recovery.
@interface FSHHost : NSObject
/// The C ABI version of the native library linked into this build.
@property(class, nonatomic, readonly) uint32_t nativeABIVersion;
/// The version of the native library.
@property(class, nonatomic, readonly, copy) NSString *nativeLibraryVersion;

/// Like ``FSHRunHelperIfRequested`` but reports failures. `arguments` excludes the program name.
+ (BOOL)runHelperWithArguments:(NSArray<NSString *> *)arguments
                       handled:(BOOL *)handled
                         error:(NSError **)error;

/// Call from the restarted application once its essential initialization has
/// succeeded. A harmless no-op for an ordinary launch.
///
/// @param confirmed Set to `YES` only when this launch was an update awaiting confirmation.
+ (BOOL)confirmStartupForInstallationRootURL:(NSURL *)rootURL
                                   confirmed:(BOOL *)confirmed
                                       error:(NSError **)error;

/// Read-only status of the last installation transaction. Safe to show on a later launch.
///
/// @param status Set to the status, or nil when there is no transaction.
+ (BOOL)installationStatusForRootURL:(NSURL *)rootURL
                              status:(FSHInstallationStatus *_Nullable *_Nonnull)status
                               error:(NSError **)error;

/// Resumes a pending rollback once every application process is stopped. Run it
/// from a maintenance process when the target executable may be locked.
+ (nullable FSHInstallationStatus *)recoverInstallationAtRootURL:(NSURL *)rootURL
                                                           error:(NSError **)error;

/// Removes a finished (committed or rolled back) or cancelled transaction and its
/// backups, which are otherwise kept. Fails with ``FSHErrorCodeBusy`` while a
/// helper is still active; retry later.
+ (BOOL)discardFinishedInstallationAtRootURL:(NSURL *)rootURL error:(NSError **)error;

- (instancetype)init NS_UNAVAILABLE;
@end

NS_ASSUME_NONNULL_END
