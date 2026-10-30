#import "FSHFixtureTransport.h"

static NSString *_Nullable FSHFixtureFileForPath(NSString *path) {
    static NSDictionary<NSString *, NSString *> *table;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        table = @{
            @"/index": @"index.json",
            @"/m/1.5.0": @"v1.5.0.manifest",
            @"/s/1.5.0": @"v1.5.0.sig",
            @"/m/2.0.0": @"v2.0.0.manifest",
            @"/s/2.0.0": @"v2.0.0.sig",
            @"/bad-sig": @"tampered.sig",
            @"/package": @"package.zip",
            @"/huge": @"huge",
        };
    });
    return table[path];
}

@implementation FSHFixtureTransport {
    NSString *_directory;
    NSInteger _calls;
}

- (instancetype)initWithDirectory:(NSString *)directory {
    self = [super init];
    if (self != nil) {
        _directory = [directory copy];
    }
    return self;
}

- (NSInteger)calls {
    @synchronized(self) {
        return _calls;
    }
}

- (BOOL)downloadURL:(NSURL *)url toSink:(FSHDownloadSink *)sink limit:(uint64_t)limit error:(NSError **)error {
    @synchronized(self) {
        _calls++;
    }
    void (^before)(void) = self.beforeDownload;
    if (before != nil) {
        before();
    }

    NSString *file = FSHFixtureFileForPath(url.path ?: @"");
    if (![url.host isEqualToString:@"test.invalid"] || file == nil) {
        if (error != NULL) {
            *error = [NSError errorWithDomain:NSCocoaErrorDomain
                                         code:NSFileReadNoSuchFileError
                                     userInfo:@{NSLocalizedDescriptionKey: [NSString stringWithFormat:@"no fixture for %@", url]}];
        }
        return NO;
    }
    NSData *data = [NSData dataWithContentsOfFile:[_directory stringByAppendingPathComponent:file] options:0 error:error];
    if (data == nil) {
        return NO;
    }
    if (![file isEqualToString:@"huge"]) {
        // "huge" deliberately declares no total.
        if (![sink setTotalBytes:data.length error:error]) {
            return NO;
        }
    }

    NSUInteger passes = (self.overrunPackage && [file isEqualToString:@"package.zip"]) ? 2 : 1;
    BOOL first = YES;
    for (NSUInteger pass = 0; pass < passes; pass++) {
        for (NSUInteger offset = 0; offset < data.length; offset += 4096) {
            NSUInteger count = MIN((NSUInteger)4096, data.length - offset);
            if (![sink writeBytes:(const uint8_t *)data.bytes + offset length:count error:error]) {
                return NO;  // propagate cancelled / size-limit errors unchanged
            }
            if (first) {
                first = NO;
                void (^after)(void) = self.afterFirstChunk;
                if (after != nil) {
                    after();
                }
            }
        }
    }
    return YES;
}

@end
