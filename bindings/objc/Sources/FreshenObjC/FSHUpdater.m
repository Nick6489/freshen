#import "FSHInternal.h"

#pragma mark - Cancellation

@implementation FSHCancellation {
    struct FreshenCancel *_handle;
}

- (instancetype)init {
    self = [super init];
    if (self != nil) {
        _handle = freshen_cancel_new();
        if (_handle == NULL) {
            return nil;
        }
    }
    return self;
}

- (struct FreshenCancel *)nativeHandle {
    return _handle;
}

- (void)cancel {
    freshen_cancel_cancel(_handle);
}

- (void)dealloc {
    freshen_cancel_free(_handle);
}

@end

#pragma mark - Configuration

@implementation FSHUpdaterConfiguration

- (instancetype)initWithProduct:(NSString *)product
                        channel:(NSString *)channel
                 currentVersion:(NSString *)currentVersion
                         target:(NSString *)target
                    trustedKeys:(NSArray<NSData *> *)trustedKeys {
    self = [super init];
    if (self != nil) {
        _product = [product copy];
        _channel = [channel copy];
        _currentVersion = [currentVersion copy];
        _target = [target copy];
        _trustedKeys = [trustedKeys copy];
        _maxDownloadBytes = 256ULL * 1024 * 1024;
        _maxUnpackedBytes = 1024ULL * 1024 * 1024;
        _httpTimeout = 300;
    }
    return self;
}

@end

#pragma mark - Candidate, prepared update, handoff

static NSArray<FSHReleaseNote *> *FSHHistory(size_t count,
                                             char *_Nullable (^version)(size_t),
                                             char *_Nullable (^notes)(size_t)) {
    NSMutableArray<FSHReleaseNote *> *history = [NSMutableArray arrayWithCapacity:count];
    for (size_t index = 0; index < count; index++) {
        [history addObject:[[FSHReleaseNote alloc] initWithVersion:FSHTakeString(version(index)) ?: @""
                                                             notes:FSHTakeString(notes(index)) ?: @""]];
    }
    return history;
}

@implementation FSHCandidate {
    struct FreshenCandidate *_handle;
}

- (instancetype)initWithNativeHandle:(struct FreshenCandidate *)handle {
    self = [super init];
    if (self != nil) {
        _handle = handle;
        _version = FSHTakeString(freshen_candidate_version(handle)) ?: @"";
        _notes = FSHTakeString(freshen_candidate_notes(handle)) ?: @"";
        _downloadSize = freshen_candidate_download_size(handle);
        _history = FSHHistory(
            freshen_candidate_history_count(handle),
            ^char *(size_t index) {
                return freshen_candidate_history_version(handle, index);
            },
            ^char *(size_t index) {
                return freshen_candidate_history_notes(handle, index);
            });
    }
    return self;
}

- (struct FreshenCandidate *)nativeHandle {
    return _handle;
}

- (void)dealloc {
    freshen_candidate_free(_handle);
}

@end

@implementation FSHPreparedUpdate {
    struct FreshenPrepared *_handle;
    BOOL _consumed;
}

- (instancetype)initWithNativeHandle:(struct FreshenPrepared *)handle {
    self = [super init];
    if (self != nil) {
        _handle = handle;
        _version = FSHTakeString(freshen_prepared_version(handle)) ?: @"";
        _notes = FSHTakeString(freshen_prepared_notes(handle)) ?: @"";
        _history = FSHHistory(
            freshen_prepared_history_count(handle),
            ^char *(size_t index) {
                return freshen_prepared_history_version(handle, index);
            },
            ^char *(size_t index) {
                return freshen_prepared_history_notes(handle, index);
            });
    }
    return self;
}

- (FSHHandoff *)armWithPolicy:(FSHInstallPolicy *)policy
          helperExecutableURL:(NSURL *)helperExecutableURL
            relaunchArguments:(NSArray<NSString *> *)relaunchArguments
                        error:(NSError **)error {
    // Reject arguments that are invalid on their own before consuming the update.
    FSHArena *arena = [[FSHArena alloc] init];
    NSError *validation = nil;
    struct FreshenInstallPolicy native = [policy nativePolicyWithArena:arena error:&validation];
    NSString *helperPath = (validation == nil) ? FSHFilePath(helperExecutableURL, @"helperExecutableURL", &validation) : nil;
    NSArray<NSString *> *arguments = relaunchArguments ?: @[];
    const char *helper = (helperPath != nil) ? [arena cStringForString:helperPath name:@"helperExecutableURL"] : "";
    const char *const *relaunch = [arena cStringArrayForStrings:arguments name:@"relaunchArguments"];
    if (validation == nil) {
        validation = arena.failure;
    }
    if (validation != nil) {
        FSH_SET_ERROR(error, validation);
        return nil;
    }

    @synchronized(self) {
        if (_consumed) {
            FSH_SET_ERROR(error, FSHMakeError(FSHErrorCodeAlreadyConsumed, @"This update was already armed.", nil));
            return nil;
        }
        // Native arming consumes the update whether or not it succeeds.
        _consumed = YES;
    }

    struct FreshenHandoff *raw = NULL;
    int32_t status = freshen_arm(_handle, &native, helper, relaunch, arguments.count, &raw);
    if (status != FRESHEN_OK) {
        FSH_SET_ERROR(error, FSHErrorFromStatus(status, nil));
        return nil;
    }
    return [[FSHHandoff alloc] initWithNativeHandle:raw];
}

- (void)armWithPolicy:(FSHInstallPolicy *)policy
    helperExecutableURL:(NSURL *)helperExecutableURL
      relaunchArguments:(NSArray<NSString *> *)relaunchArguments
        completionQueue:(dispatch_queue_t)completionQueue
             completion:(FSHArmCompletion)completion {
    dispatch_queue_t target = completionQueue ?: dispatch_get_main_queue();
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSError *error = nil;
        FSHHandoff *handoff = [self armWithPolicy:policy
                              helperExecutableURL:helperExecutableURL
                                relaunchArguments:relaunchArguments
                                            error:&error];
        dispatch_async(target, ^{
            completion(handoff, handoff != nil ? nil : error);
        });
    });
}

- (void)dealloc {
    // Deletes the staged files unless the update was armed.
    freshen_prepared_free(_handle);
}

@end

@implementation FSHHandoff {
    struct FreshenHandoff *_handle;
    BOOL _cancelled;
}

- (instancetype)initWithNativeHandle:(struct FreshenHandoff *)handle {
    self = [super init];
    if (self != nil) {
        _handle = handle;
        _helperProcessIdentifier = (pid_t)freshen_handoff_helper_pid(handle);
    }
    return self;
}

- (BOOL)cancelWithError:(NSError **)error {
    @synchronized(self) {
        if (_cancelled) {
            FSH_SET_ERROR(error, FSHMakeError(FSHErrorCodeAlreadyConsumed, @"This handoff was already cancelled.", nil));
            return NO;
        }
        _cancelled = YES;
    }
    int32_t status = freshen_handoff_cancel(_handle);
    if (status != FRESHEN_OK) {
        FSH_SET_ERROR(error, FSHErrorFromStatus(status, nil));
        return NO;
    }
    return YES;
}

- (void)dealloc {
    // Freeing does not cancel a requested installation.
    freshen_handoff_free(_handle);
}

@end

#pragma mark - Updater

@implementation FSHUpdater {
    struct FreshenUpdater *_updater;
    FSHTransportBox *_transportBox;
    NSLock *_operationLock;
}

- (instancetype)initWithConfiguration:(FSHUpdaterConfiguration *)configuration error:(NSError **)error {
    self = [super init];
    if (self == nil) {
        return nil;
    }

    NSArray<NSData *> *keys = configuration.trustedKeys;
    if (keys.count == 0) {
        FSH_SET_ERROR(error, FSHMakeError(FSHErrorCodeInvalidArgument, @"At least one trusted public key is required.", nil));
        return nil;
    }
    FSH_KEEP_ALIVE NSMutableData *flatKeys = [NSMutableData dataWithCapacity:keys.count * 32];
    for (NSData *key in keys) {
        if (key.length != 32) {
            FSH_SET_ERROR(error, FSHMakeError(FSHErrorCodeInvalidArgument,
                                              @"Every trusted key must be a 32-byte Ed25519 public key.", nil));
            return nil;
        }
        [flatKeys appendData:key];
    }
    if (configuration.httpTimeout < 0) {
        FSH_SET_ERROR(error, FSHMakeError(FSHErrorCodeInvalidArgument, @"httpTimeout must not be negative.", nil));
        return nil;
    }

    FSHArena *arena = [[FSHArena alloc] init];
    struct FreshenUpdaterConfig native;
    memset(&native, 0, sizeof native);
    native.struct_size = sizeof native;
    native.product = [arena cStringForString:configuration.product name:@"product"];
    native.channel = [arena cStringForString:configuration.channel name:@"channel"];
    native.current_version = [arena cStringForString:configuration.currentVersion name:@"currentVersion"];
    native.target = [arena cStringForString:configuration.target name:@"target"];
    native.trusted_keys = (const uint8_t *)flatKeys.bytes;
    native.trusted_key_count = keys.count;
    native.max_download = configuration.maxDownloadBytes;
    native.max_unpacked = configuration.maxUnpackedBytes;
    native.http_timeout_secs = (uint64_t)ceil(configuration.httpTimeout);

    struct FreshenTransport transport;
    memset(&transport, 0, sizeof transport);
    id<FSHTransport> custom = configuration.transport;
    if (custom != nil) {
        _transportBox = [[FSHTransportBox alloc] initWithTransport:custom];
        transport.struct_size = sizeof transport;
        transport.user = (__bridge void *)_transportBox;
        transport.download = FSHTransportCallback;
        native.transport = &transport;
    }
    if (arena.failure != nil) {
        FSH_SET_ERROR(error, arena.failure);
        return nil;
    }

    int32_t status = freshen_updater_new(&native, &_updater);
    if (status != FRESHEN_OK) {
        FSH_SET_ERROR(error, FSHErrorFromStatus(status, nil));
        return nil;
    }
    _operationLock = [[NSLock alloc] init];
    return self;
}

- (void)dealloc {
    freshen_updater_free(_updater);
}

static NSError *FSHBusyError(void) {
    return FSHMakeError(FSHErrorCodeOperationInProgress,
                        @"An update operation is already running on this FSHUpdater.", nil);
}

#pragma mark Check

- (BOOL)checkForUpdateFromSource:(FSHReleaseSource *)source
                    cancellation:(FSHCancellation *)cancellation
                        progress:(FSHProgressHandler)progress
                       candidate:(FSHCandidate *_Nullable *_Nonnull)outCandidate
                           error:(NSError **)error {
    *outCandidate = nil;
    if (![_operationLock tryLock]) {
        FSH_SET_ERROR(error, FSHBusyError());
        return NO;
    }
    @try {
        FSHArena *arena = [[FSHArena alloc] init];
        struct FreshenSource native = [source nativeSourceWithArena:arena];
        if (arena.failure != nil) {
            FSH_SET_ERROR(error, arena.failure);
            return NO;
        }
        FSH_KEEP_ALIVE FSHProgressBox *box = (progress != nil) ? [[FSHProgressBox alloc] initWithHandler:progress] : nil;
        FSH_KEEP_ALIVE FSHCancellation *pinned = cancellation;

        struct FreshenCandidate *raw = NULL;
        int32_t status = freshen_check(_updater, &native, pinned.nativeHandle,
                                       (box != nil) ? FSHProgressCallback : NULL, (__bridge void *)box, &raw);
        if (status != FRESHEN_OK) {
            FSH_SET_ERROR(error, FSHErrorFromStatus(status, [_transportBox takeLastError]));
            return NO;
        }
        if (raw != NULL) {
            *outCandidate = [[FSHCandidate alloc] initWithNativeHandle:raw];
        }
        return YES;
    } @finally {
        [_operationLock unlock];
    }
}

- (void)checkForUpdateFromSource:(FSHReleaseSource *)source
                    cancellation:(FSHCancellation *)cancellation
                        progress:(FSHProgressHandler)progress
                 completionQueue:(dispatch_queue_t)completionQueue
                      completion:(FSHCheckCompletion)completion {
    dispatch_queue_t target = completionQueue ?: dispatch_get_main_queue();
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        FSHCandidate *candidate = nil;
        NSError *error = nil;
        BOOL succeeded = [self checkForUpdateFromSource:source
                                           cancellation:cancellation
                                               progress:progress
                                              candidate:&candidate
                                                  error:&error];
        dispatch_async(target, ^{
            completion(succeeded ? candidate : nil, succeeded ? nil : error);
        });
    });
}

#pragma mark Prepare

- (FSHPreparedUpdate *)prepareCandidate:(FSHCandidate *)candidate
                  temporaryDirectoryURL:(NSURL *)temporaryDirectoryURL
                           cancellation:(FSHCancellation *)cancellation
                               progress:(FSHProgressHandler)progress
                                  error:(NSError **)error {
    NSString *directory = FSHFilePath(temporaryDirectoryURL, @"temporaryDirectoryURL", error);
    if (directory == nil) {
        return nil;
    }
    if (![_operationLock tryLock]) {
        FSH_SET_ERROR(error, FSHBusyError());
        return nil;
    }
    @try {
        FSHArena *arena = [[FSHArena alloc] init];
        const char *parent = [arena cStringForString:directory name:@"temporaryDirectoryURL"];
        if (arena.failure != nil) {
            FSH_SET_ERROR(error, arena.failure);
            return nil;
        }
        FSH_KEEP_ALIVE FSHProgressBox *box = (progress != nil) ? [[FSHProgressBox alloc] initWithHandler:progress] : nil;
        FSH_KEEP_ALIVE FSHCancellation *pinnedCancellation = cancellation;
        FSH_KEEP_ALIVE FSHCandidate *pinnedCandidate = candidate;

        struct FreshenPrepared *raw = NULL;
        int32_t status = freshen_prepare(_updater, pinnedCandidate.nativeHandle, parent, pinnedCancellation.nativeHandle,
                                         (box != nil) ? FSHProgressCallback : NULL, (__bridge void *)box, &raw);
        if (status != FRESHEN_OK) {
            FSH_SET_ERROR(error, FSHErrorFromStatus(status, [_transportBox takeLastError]));
            return nil;
        }
        return [[FSHPreparedUpdate alloc] initWithNativeHandle:raw];
    } @finally {
        [_operationLock unlock];
    }
}

- (void)prepareCandidate:(FSHCandidate *)candidate
    temporaryDirectoryURL:(NSURL *)temporaryDirectoryURL
             cancellation:(FSHCancellation *)cancellation
                 progress:(FSHProgressHandler)progress
          completionQueue:(dispatch_queue_t)completionQueue
               completion:(FSHPrepareCompletion)completion {
    dispatch_queue_t target = completionQueue ?: dispatch_get_main_queue();
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSError *error = nil;
        FSHPreparedUpdate *prepared = [self prepareCandidate:candidate
                                       temporaryDirectoryURL:temporaryDirectoryURL
                                                cancellation:cancellation
                                                    progress:progress
                                                       error:&error];
        dispatch_async(target, ^{
            completion(prepared, prepared != nil ? nil : error);
        });
    });
}

@end
