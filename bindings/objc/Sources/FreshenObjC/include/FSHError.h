#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

/// Domain of every error this library returns.
FOUNDATION_EXPORT NSErrorDomain const FSHErrorDomain;

/// Classifies a failure. Codes below 1000 come from the native freshen library;
/// codes from 1000 are raised by this Objective-C layer.
typedef NS_ERROR_ENUM(FSHErrorDomain, FSHErrorCode) {
    /// The operation was cancelled through its ``FSHCancellation``.
    FSHErrorCodeCancelled = 1,
    /// A manifest signature did not verify against the pinned public keys.
    FSHErrorCodeSignature = 2,
    /// The update, policy or configuration is invalid.
    FSHErrorCodeInvalid = 3,
    /// A package contains an unsafe path.
    FSHErrorCodeUnsafePath = 4,
    /// A download or extraction exceeded its configured limit.
    FSHErrorCodeSizeLimit = 5,
    /// A downloaded file did not match its signed hash.
    FSHErrorCodeHashMismatch = 6,
    /// Another update owns the installation lock. Retry later.
    FSHErrorCodeBusy = 7,
    /// The helper process failed.
    FSHErrorCodeHelper = 8,
    /// The installation requires recovery before another update.
    FSHErrorCodeRecovery = 9,
    /// A filesystem or transport failure, including one raised by a custom ``FSHTransport``.
    /// A failure reported by a custom transport is attached as ``NSUnderlyingErrorKey``.
    FSHErrorCodeIO = 10,
    /// A manifest or index was not valid JSON.
    FSHErrorCodeJSON = 11,
    /// The built-in HTTPS transport failed.
    FSHErrorCodeHTTP = 12,
    /// A package archive was malformed.
    FSHErrorCodeZip = 13,
    /// An error this version of the bindings does not classify. Read the description.
    FSHErrorCodeUnknown = 100,
    /// The native library rejected an argument.
    FSHErrorCodeInvalidArgument = 101,
    /// A panic was caught inside the native library. Discard the objects involved.
    FSHErrorCodePanic = 102,

    /// This ``FSHUpdater`` is already running an operation. An updater runs one at a time.
    FSHErrorCodeOperationInProgress = 1001,
    /// The prepared update was already armed, or the handoff was already cancelled.
    FSHErrorCodeAlreadyConsumed = 1002,
};

NS_ASSUME_NONNULL_END
