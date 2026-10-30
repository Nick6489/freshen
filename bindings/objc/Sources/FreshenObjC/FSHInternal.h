// Private to the FreshenObjC target: the C ABI and helpers shared by the
// implementation files. Nothing here is part of the public API.
#import <Foundation/Foundation.h>

#include "freshen.h"

#import "FSHError.h"
#import "FSHTransport.h"
#import "FSHTypes.h"
#import "FSHUpdater.h"

NS_ASSUME_NONNULL_BEGIN

/// Keeps a local variable alive until the end of its scope. Under ARC an object
/// can otherwise be released after its last use, which for a pointer handed to
/// native code is before the native call has finished with it.
#define FSH_KEEP_ALIVE __attribute__((objc_precise_lifetime))

#pragma mark - Errors

/// An error in ``FSHErrorDomain``.
NSError *FSHMakeError(NSInteger code, NSString *description, NSError *_Nullable underlying);

/// Turns a native status into an error. Must run on the thread that made the failing
/// call, before any other freshen call, because the native message is thread-local.
NSError *FSHErrorFromStatus(int32_t status, NSError *_Nullable underlying);

/// The native status a transport's error maps to.
int32_t FSHStatusForTransportError(NSError *_Nullable error);

#pragma mark - Marshaling

/// Owns UTF-8 copies of strings and arrays handed to native code for one call.
/// Invalid input (a string containing NUL) is recorded in `failure` and an empty
/// string is returned so the caller can finish building and check once.
@interface FSHArena : NSObject
@property(nonatomic, readonly, nullable) NSError *failure;
- (const char *)cStringForString:(NSString *)string name:(NSString *)name;
/// A native array of NUL-terminated strings, or NULL for an empty array.
- (const char *_Nonnull *_Nullable)cStringArrayForStrings:(NSArray<NSString *> *)strings
                                                     name:(NSString *)name;
/// Zero-filled storage that lives as long as the arena.
- (void *)zeroedBytesOfLength:(NSUInteger)length;
@end

/// The filesystem path of a file URL, or nil with an error if it is not one.
NSString *_Nullable FSHFilePath(NSURL *url, NSString *name, NSError *_Nullable *_Nullable error);

/// Writes `value` through an optional out-error.
#define FSH_SET_ERROR(errorPointer, value) \
    do {                                   \
        if ((errorPointer) != NULL) {      \
            *(errorPointer) = (value);     \
        }                                  \
    } while (0)

#pragma mark - Native handle ownership

@interface FSHCancellation ()
/// The native handle; valid while this object is alive.
@property(nonatomic, readonly) struct FreshenCancel *nativeHandle;
@end

@interface FSHCandidate ()
@property(nonatomic, readonly) struct FreshenCandidate *nativeHandle;
/// Takes ownership of a native candidate and reads its data once, up front.
- (instancetype)initWithNativeHandle:(struct FreshenCandidate *)handle NS_DESIGNATED_INITIALIZER;
@end

@interface FSHPreparedUpdate ()
/// Takes ownership of a native prepared update and reads its data once, up front.
- (instancetype)initWithNativeHandle:(struct FreshenPrepared *)handle NS_DESIGNATED_INITIALIZER;
@end

@interface FSHHandoff ()
- (instancetype)initWithNativeHandle:(struct FreshenHandoff *)handle NS_DESIGNATED_INITIALIZER;
@end

#pragma mark - Callbacks

/// Managed state the native updater carries for its custom transport. The updater
/// retains it for as long as the native updater exists.
@interface FSHTransportBox : NSObject
@property(nonatomic, readonly) id<FSHTransport> transport;
/// The most recent error the transport reported; consumed when the operation returns.
@property(nonatomic, strong, nullable) NSError *lastError;
- (instancetype)initWithTransport:(id<FSHTransport>)transport NS_DESIGNATED_INITIALIZER;
- (instancetype)init NS_UNAVAILABLE;
- (nullable NSError *)takeLastError;
@end

/// The native download callback for a custom transport (`user` is an ``FSHTransportBox``).
int32_t FSHTransportCallback(void *_Nullable user, const char *url, struct FreshenSink *sink, uint64_t limit);

/// Holds a progress handler for the duration of one operation.
@interface FSHProgressBox : NSObject
- (instancetype)initWithHandler:(FSHProgressHandler)handler NS_DESIGNATED_INITIALIZER;
- (instancetype)init NS_UNAVAILABLE;
- (void)reportKind:(int32_t)kind received:(uint64_t)received total:(uint64_t)total path:(nullable const char *)path;
@end

/// The native progress callback (`user` is an ``FSHProgressBox``).
void FSHProgressCallback(void *_Nullable user, int32_t kind, uint64_t received, uint64_t total, const char *_Nullable path);

@interface FSHDownloadSink ()
/// Wraps the native sink for one download. It is invalidated when the callback returns.
- (instancetype)initWithNativeSink:(struct FreshenSink *)sink NS_DESIGNATED_INITIALIZER;
- (void)invalidate;
@end

#pragma mark - Native form of value objects

@interface FSHReleaseSource ()
/// The native description of this source. Strings and arrays live as long as `arena`.
- (struct FreshenSource)nativeSourceWithArena:(FSHArena *)arena;
@end

@interface FSHInstallPolicy ()
- (struct FreshenInstallPolicy)nativePolicyWithArena:(FSHArena *)arena
                                               error:(NSError **)error;
@end

/// Reads a library-owned string into an NSString and frees it. Nil stays nil.
NSString *_Nullable FSHTakeString(char *_Nullable text);

NS_ASSUME_NONNULL_END
