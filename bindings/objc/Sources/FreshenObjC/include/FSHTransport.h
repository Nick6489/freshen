#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

/// Receives the bytes of one download. It is valid only during the
/// ``FSHTransport`` call it was passed to; never keep it.
///
/// The size limit, cancellation and progress reporting are enforced by the sink
/// inside the native library, so a transport cannot skip them.
@interface FSHDownloadSink : NSObject
/// Whether the operation has been cancelled. Poll this while blocked on the network.
@property(nonatomic, readonly, getter=isCancelled) BOOL cancelled;

/// Reports the total size when it is known (for example from Content-Length) so
/// progress can show a percentage. A total above the limit fails immediately.
- (BOOL)setTotalBytes:(uint64_t)totalBytes error:(NSError **)error;

/// Appends downloaded bytes. Fails with ``FSHErrorCodeCancelled`` once the
/// operation is cancelled, and ``FSHErrorCodeSizeLimit`` past the limit; return
/// that error from your transport unchanged.
- (BOOL)writeBytes:(const void *)bytes length:(NSUInteger)length error:(NSError **)error;

/// Appends downloaded data.
- (BOOL)writeData:(NSData *)data error:(NSError **)error;

- (instancetype)init NS_UNAVAILABLE;
@end

/// Supplies downloads: custom authentication, offline storage, or test fixtures.
///
/// It runs on the thread performing the operation, so blocking is fine. Every
/// byte must go through the sink. URLs that are not HTTPS, or that embed
/// credentials, are rejected before the transport is called. Never let an
/// Objective-C exception escape; it is caught and reported as an I/O error.
@protocol FSHTransport <NSObject>
/// Fetches `url` and writes every byte to `sink`.
///
/// @param url The HTTPS URL to fetch.
/// @param sink Receives the bytes.
/// @param limit Informational maximum size; the sink enforces it.
/// @param error On failure, the error to report. Return a sink error unchanged so its
///   code (cancelled, size limit) is preserved; any other error becomes an
///   ``FSHErrorCodeIO`` failure with yours attached as the underlying error.
/// @return `YES` when every byte was written.
- (BOOL)downloadURL:(NSURL *)url
             toSink:(FSHDownloadSink *)sink
              limit:(uint64_t)limit
              error:(NSError **)error;
@end

NS_ASSUME_NONNULL_END
