#import <Foundation/Foundation.h>

#import "FSHError.h"
#import "FSHTransport.h"
#import "FSHTypes.h"

NS_ASSUME_NONNULL_BEGIN

@class FSHCandidate;
@class FSHHandoff;
@class FSHPreparedUpdate;

#pragma mark - Cancellation

/// Cancels operations. Thread-safe: call ``-cancel`` from any thread.
@interface FSHCancellation : NSObject
/// Requests cancellation. Idempotent. A read blocked on the network is bounded by
/// the request timeout.
- (void)cancel;
@end

#pragma mark - Configuration

/// Settings for an ``FSHUpdater``.
@interface FSHUpdaterConfiguration : NSObject
/// Signed product name that releases must match.
@property(nonatomic, copy) NSString *product;
/// Signed channel that releases must match, such as `stable`.
@property(nonatomic, copy) NSString *channel;
/// Semantic version of the running application.
@property(nonatomic, copy) NSString *currentVersion;
/// Distribution target, for example `aarch64-apple-darwin`.
@property(nonatomic, copy) NSString *target;
/// Pinned Ed25519 public keys, 32 bytes each. Ship old and new keys together to rotate.
@property(nonatomic, copy) NSArray<NSData *> *trustedKeys;
/// Largest package download accepted. Defaults to 256 MiB.
@property(nonatomic) uint64_t maxDownloadBytes;
/// Largest total unpacked size accepted. Defaults to 1 GiB.
@property(nonatomic) uint64_t maxUnpackedBytes;
/// Total timeout of the built-in HTTPS transport. Defaults to five minutes.
/// Ignored when `transport` is set.
@property(nonatomic) NSTimeInterval httpTimeout;
/// Optional custom transport for authentication, offline storage or tests.
/// Nil uses the built-in HTTPS transport.
@property(nonatomic, strong, nullable) id<FSHTransport> transport;

- (instancetype)initWithProduct:(NSString *)product
                        channel:(NSString *)channel
                 currentVersion:(NSString *)currentVersion
                         target:(NSString *)target
                    trustedKeys:(NSArray<NSData *> *)trustedKeys NS_DESIGNATED_INITIALIZER;
- (instancetype)init NS_UNAVAILABLE;
@end

#pragma mark - Updater

/// Completion for a check. `candidate` is nil and `error` is nil when the
/// application is up to date.
typedef void (^FSHCheckCompletion)(FSHCandidate *_Nullable candidate, NSError *_Nullable error);
typedef void (^FSHPrepareCompletion)(FSHPreparedUpdate *_Nullable prepared, NSError *_Nullable error);
typedef void (^FSHArmCompletion)(FSHHandoff *_Nullable handoff, NSError *_Nullable error);

/// Checks for and prepares authenticated updates.
///
/// The synchronous methods block; call them on a worker thread or use the
/// block-based variants, which run on a background queue and call back on
/// `completionQueue` (the main queue when nil). An updater runs one operation at a
/// time: a second concurrent call fails with ``FSHErrorCodeOperationInProgress``.
@interface FSHUpdater : NSObject
- (nullable instancetype)initWithConfiguration:(FSHUpdaterConfiguration *)configuration
                                         error:(NSError **)error NS_DESIGNATED_INITIALIZER;
- (instancetype)init NS_UNAVAILABLE;

/// Checks for a newer authenticated release. Blocking.
///
/// A failed fetch or a bad signature is an error, never "up to date".
///
/// @param outCandidate Set to the newest applicable release, or nil when the
///   application is up to date.
/// @return `YES` on success (including up to date), `NO` with `error` set on failure.
- (BOOL)checkForUpdateFromSource:(FSHReleaseSource *)source
                    cancellation:(nullable FSHCancellation *)cancellation
                        progress:(nullable FSHProgressHandler)progress
                       candidate:(FSHCandidate *_Nullable *_Nonnull)outCandidate
                           error:(NSError **)error;

/// Block-based check. See ``-checkForUpdateFromSource:cancellation:progress:candidate:error:``.
- (void)checkForUpdateFromSource:(FSHReleaseSource *)source
                    cancellation:(nullable FSHCancellation *)cancellation
                        progress:(nullable FSHProgressHandler)progress
                 completionQueue:(nullable dispatch_queue_t)completionQueue
                      completion:(FSHCheckCompletion)completion;

/// Downloads, verifies and extracts a candidate into a private directory beneath
/// `temporaryDirectoryURL`. The candidate stays valid. Blocking.
- (nullable FSHPreparedUpdate *)prepareCandidate:(FSHCandidate *)candidate
                              temporaryDirectoryURL:(NSURL *)temporaryDirectoryURL
                                       cancellation:(nullable FSHCancellation *)cancellation
                                           progress:(nullable FSHProgressHandler)progress
                                              error:(NSError **)error;

/// Block-based prepare.
- (void)prepareCandidate:(FSHCandidate *)candidate
    temporaryDirectoryURL:(NSURL *)temporaryDirectoryURL
             cancellation:(nullable FSHCancellation *)cancellation
                 progress:(nullable FSHProgressHandler)progress
          completionQueue:(nullable dispatch_queue_t)completionQueue
               completion:(FSHPrepareCompletion)completion;
@end

#pragma mark - Candidate, prepared update, handoff

/// A verified newer release that has not been downloaded yet.
@interface FSHCandidate : NSObject
/// Version of the selected (newest) release.
@property(nonatomic, readonly, copy) NSString *version;
/// Notes of the selected release; possibly empty.
@property(nonatomic, readonly, copy) NSString *notes;
/// Size in bytes of the package that preparing will download.
@property(nonatomic, readonly) uint64_t downloadSize;
/// Authenticated releases newer than the installed version, oldest first, through
/// the selected one. This is the available history, not a guarantee of completeness.
@property(nonatomic, readonly, copy) NSArray<FSHReleaseNote *> *history;
- (instancetype)init NS_UNAVAILABLE;
@end

/// A fully downloaded, verified and staged update. Holding one lets the user defer
/// installation. Releasing it without arming deletes its temporary files and leaves
/// the installation untouched.
@interface FSHPreparedUpdate : NSObject
@property(nonatomic, readonly, copy) NSString *version;
@property(nonatomic, readonly, copy) NSString *notes;
@property(nonatomic, readonly, copy) NSArray<FSHReleaseNote *> *history;

/// Copies the verified staging into the installation and starts a helper that
/// waits for this process to exit. Returns once the helper holds the installation
/// lock. Save your work and quit, or call ``-cancelWithError:`` on the handoff if
/// shutdown is declined. Blocking.
///
/// This update is consumed whether the call succeeds or fails; a second call fails
/// with ``FSHErrorCodeAlreadyConsumed``. An argument that is invalid on its own
/// (such as an out-of-range timeout) is rejected first and does not consume it.
///
/// @param policy The application's local ownership policy.
/// @param helperExecutableURL A self-contained helper: the `freshen-helper` binary
///   shipped with the application, or the application executable itself if it calls
///   ``FSHRunHelperIfRequested`` first thing in `main`.
/// @param relaunchArguments Arguments the restarted application receives.
- (nullable FSHHandoff *)armWithPolicy:(FSHInstallPolicy *)policy
                   helperExecutableURL:(NSURL *)helperExecutableURL
                     relaunchArguments:(nullable NSArray<NSString *> *)relaunchArguments
                                 error:(NSError **)error;

/// Block-based arm.
- (void)armWithPolicy:(FSHInstallPolicy *)policy
    helperExecutableURL:(NSURL *)helperExecutableURL
      relaunchArguments:(nullable NSArray<NSString *> *)relaunchArguments
        completionQueue:(nullable dispatch_queue_t)completionQueue
             completion:(FSHArmCompletion)completion;

- (instancetype)init NS_UNAVAILABLE;
@end

/// An armed update: a helper holds the installation lock and waits for this
/// process to exit. **Releasing a handoff does not cancel the installation.**
@interface FSHHandoff : NSObject
/// Process identifier of the waiting helper.
@property(nonatomic, readonly) pid_t helperProcessIdentifier;

/// Cancels the requested installation because shutdown was declined. Blocking.
/// A handoff can be cancelled once; a second call fails with
/// ``FSHErrorCodeAlreadyConsumed``.
- (BOOL)cancelWithError:(NSError **)error;
- (instancetype)init NS_UNAVAILABLE;
@end

NS_ASSUME_NONNULL_END
