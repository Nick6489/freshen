#import "FSHInternal.h"

NSErrorDomain const FSHErrorDomain = @"FSHErrorDomain";

NSError *FSHMakeError(NSInteger code, NSString *description, NSError *underlying) {
    NSMutableDictionary<NSErrorUserInfoKey, id> *info =
        [NSMutableDictionary dictionaryWithObject:description forKey:NSLocalizedDescriptionKey];
    if (underlying != nil) {
        info[NSUnderlyingErrorKey] = underlying;
    }
    return [NSError errorWithDomain:FSHErrorDomain code:code userInfo:info];
}

NSError *FSHErrorFromStatus(int32_t status, NSError *underlying) {
    const char *message = freshen_last_error_message();
    NSString *text = (message != NULL) ? [NSString stringWithUTF8String:message] : nil;
    if (text.length == 0) {
        text = [NSString stringWithFormat:@"freshen operation failed (status %d)", (int)status];
    }
    return FSHMakeError(status, text, underlying);
}

int32_t FSHStatusForTransportError(NSError *error) {
    if (error == nil) {
        return FRESHEN_ERR_IO;
    }
    if ([error.domain isEqualToString:FSHErrorDomain] && error.code > 0 && error.code < 1000) {
        return (int32_t)error.code;
    }
    if ([error.domain isEqualToString:NSCocoaErrorDomain] && error.code == NSUserCancelledError) {
        return FRESHEN_ERR_CANCELLED;
    }
    return FRESHEN_ERR_IO;
}

NSString *FSHTakeString(char *text) {
    if (text == NULL) {
        return nil;
    }
    NSString *result = [NSString stringWithUTF8String:text];
    freshen_string_free(text);
    return result;
}

NSString *FSHFilePath(NSURL *url, NSString *name, NSError **error) {
    if (![url isFileURL]) {
        FSH_SET_ERROR(error, FSHMakeError(FSHErrorCodeInvalidArgument,
                                          [NSString stringWithFormat:@"%@ must be a file URL", name], nil));
        return nil;
    }
    return url.path;
}

@implementation FSHArena {
    NSMutableArray<NSData *> *_storage;
    NSError *_failure;
}

- (instancetype)init {
    self = [super init];
    if (self != nil) {
        _storage = [NSMutableArray array];
    }
    return self;
}

- (NSError *)failure {
    return _failure;
}

- (const char *)cStringForString:(NSString *)string name:(NSString *)name {
    NSData *utf8 = [string dataUsingEncoding:NSUTF8StringEncoding];
    if (utf8 == nil || (utf8.length > 0 && memchr(utf8.bytes, 0, utf8.length) != NULL)) {
        if (_failure == nil) {
            _failure = FSHMakeError(FSHErrorCodeInvalidArgument,
                                    [NSString stringWithFormat:@"%@ must be valid text without NUL characters", name],
                                    nil);
        }
        return "";
    }
    NSMutableData *buffer = [NSMutableData dataWithData:utf8];
    const uint8_t terminator = 0;
    [buffer appendBytes:&terminator length:1];
    [_storage addObject:buffer];
    return (const char *)buffer.bytes;
}

- (const char *_Nonnull *_Nullable)cStringArrayForStrings:(NSArray<NSString *> *)strings name:(NSString *)name {
    if (strings.count == 0) {
        return NULL;
    }
    NSMutableData *pointers = [NSMutableData dataWithLength:strings.count * sizeof(const char *)];
    const char **slots = (const char **)pointers.mutableBytes;
    NSUInteger index = 0;
    for (NSString *string in strings) {
        slots[index++] = [self cStringForString:string name:name];
    }
    [_storage addObject:pointers];
    return slots;
}

- (void *)zeroedBytesOfLength:(NSUInteger)length {
    NSMutableData *block = [NSMutableData dataWithLength:length];
    [_storage addObject:block];
    return block.mutableBytes;
}

@end
