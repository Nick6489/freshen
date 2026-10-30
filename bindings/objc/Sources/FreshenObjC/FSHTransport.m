#import "FSHInternal.h"

@implementation FSHDownloadSink {
    struct FreshenSink *_sink;
}

- (instancetype)initWithNativeSink:(struct FreshenSink *)sink {
    self = [super init];
    if (self != nil) {
        _sink = sink;
    }
    return self;
}

- (void)invalidate {
    _sink = NULL;
}

- (BOOL)isCancelled {
    struct FreshenSink *sink = _sink;
    return sink == NULL || freshen_sink_is_cancelled(sink);
}

- (struct FreshenSink *)liveSink:(NSError **)error {
    if (_sink == NULL) {
        FSH_SET_ERROR(error, FSHMakeError(FSHErrorCodeInvalidArgument,
                                          @"An FSHDownloadSink is only valid during the transport call it was passed to.",
                                          nil));
    }
    return _sink;
}

- (BOOL)finish:(int32_t)status error:(NSError **)error {
    if (status == FRESHEN_OK) {
        return YES;
    }
    // The native message is thread-local; read it here, on the thread that failed.
    FSH_SET_ERROR(error, FSHErrorFromStatus(status, nil));
    return NO;
}

- (BOOL)setTotalBytes:(uint64_t)totalBytes error:(NSError **)error {
    struct FreshenSink *sink = [self liveSink:error];
    if (sink == NULL) {
        return NO;
    }
    return [self finish:freshen_sink_set_total(sink, totalBytes) error:error];
}

- (BOOL)writeBytes:(const void *)bytes length:(NSUInteger)length error:(NSError **)error {
    struct FreshenSink *sink = [self liveSink:error];
    if (sink == NULL) {
        return NO;
    }
    return [self finish:freshen_sink_write(sink, bytes, length) error:error];
}

- (BOOL)writeData:(NSData *)data error:(NSError **)error {
    return [self writeBytes:data.bytes length:data.length error:error];
}

@end

@implementation FSHTransportBox

- (instancetype)initWithTransport:(id<FSHTransport>)transport {
    self = [super init];
    if (self != nil) {
        _transport = transport;
    }
    return self;
}

- (NSError *)takeLastError {
    NSError *error = _lastError;
    _lastError = nil;
    return error;
}

@end

int32_t FSHTransportCallback(void *user, const char *url, struct FreshenSink *sink, uint64_t limit) {
    // Nothing may unwind into native code: every failure becomes a status code.
    FSHTransportBox *box = (__bridge FSHTransportBox *)user;
    FSHDownloadSink *wrapper = [[FSHDownloadSink alloc] initWithNativeSink:sink];
    int32_t result = FRESHEN_OK;
    @autoreleasepool {
        @try {
            NSString *text = [NSString stringWithUTF8String:url];
            NSURL *target = (text != nil) ? [NSURL URLWithString:text] : nil;
            if (target == nil) {
                box.lastError = FSHMakeError(FSHErrorCodeIO, @"The transport was given a URL it could not parse.", nil);
                result = FRESHEN_ERR_IO;
            } else {
                NSError *error = nil;
                if (![box.transport downloadURL:target toSink:wrapper limit:limit error:&error]) {
                    box.lastError = error;
                    result = FSHStatusForTransportError(error);
                }
            }
        } @catch (NSException *exception) {
            NSString *reason = exception.reason ?: exception.name;
            box.lastError = FSHMakeError(FSHErrorCodeIO,
                                         [NSString stringWithFormat:@"The transport raised an exception: %@", reason], nil);
            result = FRESHEN_ERR_IO;
        }
    }
    [wrapper invalidate];
    return result;
}

@implementation FSHProgressBox {
    FSHProgressHandler _handler;
}

- (instancetype)initWithHandler:(FSHProgressHandler)handler {
    self = [super init];
    if (self != nil) {
        _handler = [handler copy];
    }
    return self;
}

- (void)reportKind:(int32_t)kind received:(uint64_t)received total:(uint64_t)total path:(const char *)path {
    FSHProgressKind mapped;
    switch (kind) {
        case FRESHEN_EVENT_CHECKING:
            mapped = FSHProgressKindChecking;
            break;
        case FRESHEN_EVENT_DOWNLOADING:
            mapped = FSHProgressKindDownloading;
            break;
        case FRESHEN_EVENT_VERIFYING:
            mapped = FSHProgressKindVerifying;
            break;
        case FRESHEN_EVENT_EXTRACTING:
            mapped = FSHProgressKindExtracting;
            break;
        case FRESHEN_EVENT_READY:
            mapped = FSHProgressKindReady;
            break;
        default:
            mapped = FSHProgressKindUnknown;
            break;
    }
    NSNumber *totalNumber = (total == FRESHEN_TOTAL_UNKNOWN) ? nil : [NSNumber numberWithUnsignedLongLong:total];
    NSString *pathString = (path != NULL) ? [NSString stringWithUTF8String:path] : nil;
    _handler([[FSHProgress alloc] initWithKind:mapped bytesReceived:received totalBytes:totalNumber path:pathString]);
}

@end

void FSHProgressCallback(void *user, int32_t kind, uint64_t received, uint64_t total, const char *path) {
    if (user == NULL) {
        return;
    }
    FSHProgressBox *box = (__bridge FSHProgressBox *)user;
    @autoreleasepool {
        @try {
            [box reportKind:kind received:received total:total path:path];
        } @catch (NSException *exception) {
            // A misbehaving handler must not take down the update or unwind into native code.
        }
    }
}
