#import "FSHHost.h"

#import "FSHInternal.h"

BOOL FSHRunHelperIfRequested(int argc, const char *_Nonnull const *_Nonnull argv) {
    if (argc < 2) {
        return NO;
    }
    NSMutableArray<NSString *> *arguments = [NSMutableArray arrayWithCapacity:(NSUInteger)argc - 1];
    for (int index = 1; index < argc; index++) {
        NSString *argument = [NSString stringWithUTF8String:argv[index]];
        if (argument == nil) {
            return NO;  // Not valid UTF-8: certainly not a helper launch of ours.
        }
        [arguments addObject:argument];
    }
    BOOL handled = NO;
    NSError *error = nil;
    if ([FSHHost runHelperWithArguments:arguments handled:&handled error:&error]) {
        return handled;
    }
    // A launch that named the helper but failed must still not start the application.
    BOOL helperLaunch = [arguments.firstObject isEqualToString:@"--freshen-apply"];
    if (helperLaunch) {
        fprintf(stderr, "freshen helper failed: %s\n", error.localizedDescription.UTF8String);
    }
    return helperLaunch;
}

static FSHInstallState FSHStateFromNative(int32_t state) {
    switch (state) {
        case FRESHEN_STATE_PREPARED:
            return FSHInstallStatePrepared;
        case FRESHEN_STATE_APPLYING:
            return FSHInstallStateApplying;
        case FRESHEN_STATE_AWAITING_CONFIRMATION:
            return FSHInstallStateAwaitingConfirmation;
        case FRESHEN_STATE_COMMITTED:
            return FSHInstallStateCommitted;
        case FRESHEN_STATE_ROLLING_BACK:
            return FSHInstallStateRollingBack;
        case FRESHEN_STATE_ROLLED_BACK:
            return FSHInstallStateRolledBack;
        case FRESHEN_STATE_FAILED:
            return FSHInstallStateFailed;
        default:
            return FSHInstallStateUnknown;
    }
}

@implementation FSHHost

+ (uint32_t)nativeABIVersion {
    return freshen_abi_version();
}

+ (NSString *)nativeLibraryVersion {
    const char *version = freshen_library_version();
    return (version != NULL ? [NSString stringWithUTF8String:version] : nil) ?: @"";
}

+ (BOOL)runHelperWithArguments:(NSArray<NSString *> *)arguments handled:(BOOL *)handled error:(NSError **)error {
    *handled = NO;
    FSHArena *arena = [[FSHArena alloc] init];
    const char *const *argv = [arena cStringArrayForStrings:arguments name:@"arguments"];
    if (arena.failure != nil) {
        FSH_SET_ERROR(error, arena.failure);
        return NO;
    }
    bool nativeHandled = false;
    int32_t status = freshen_run_helper(arguments.count, argv, &nativeHandled);
    if (status != FRESHEN_OK) {
        FSH_SET_ERROR(error, FSHErrorFromStatus(status, nil));
        return NO;
    }
    *handled = nativeHandled ? YES : NO;
    return YES;
}

+ (BOOL)confirmStartupForInstallationRootURL:(NSURL *)rootURL confirmed:(BOOL *)confirmed error:(NSError **)error {
    *confirmed = NO;
    NSString *path = FSHFilePath(rootURL, @"rootURL", error);
    if (path == nil) {
        return NO;
    }
    FSHArena *arena = [[FSHArena alloc] init];
    const char *root = [arena cStringForString:path name:@"rootURL"];
    if (arena.failure != nil) {
        FSH_SET_ERROR(error, arena.failure);
        return NO;
    }
    bool nativeConfirmed = false;
    int32_t status = freshen_confirm_startup(root, &nativeConfirmed);
    if (status != FRESHEN_OK) {
        FSH_SET_ERROR(error, FSHErrorFromStatus(status, nil));
        return NO;
    }
    *confirmed = nativeConfirmed ? YES : NO;
    return YES;
}

/// Shared by status and recovery. Takes ownership of both native strings before
/// deciding whether to fail, so nothing leaks.
+ (BOOL)readStatusAtRootURL:(NSURL *)rootURL
                    recover:(BOOL)recover
                     status:(FSHInstallationStatus *_Nullable *_Nonnull)outStatus
                      error:(NSError **)error {
    *outStatus = nil;
    NSString *path = FSHFilePath(rootURL, @"rootURL", error);
    if (path == nil) {
        return NO;
    }
    FSHArena *arena = [[FSHArena alloc] init];
    const char *root = [arena cStringForString:path name:@"rootURL"];
    if (arena.failure != nil) {
        FSH_SET_ERROR(error, arena.failure);
        return NO;
    }
    int32_t state = -1;
    char *version = NULL;
    char *detail = NULL;
    int32_t status = recover ? freshen_recover_installation(root, &state, &version, &detail)
                             : freshen_installation_status(root, &state, &version, &detail);
    NSString *versionText = FSHTakeString(version) ?: @"";
    NSString *detailText = FSHTakeString(detail) ?: @"";
    if (status != FRESHEN_OK) {
        FSH_SET_ERROR(error, FSHErrorFromStatus(status, nil));
        return NO;
    }
    if (state != FRESHEN_STATE_NONE) {
        *outStatus = [[FSHInstallationStatus alloc] initWithState:FSHStateFromNative(state)
                                                          version:versionText
                                                           detail:detailText];
    }
    return YES;
}

+ (BOOL)installationStatusForRootURL:(NSURL *)rootURL
                              status:(FSHInstallationStatus *_Nullable *_Nonnull)status
                               error:(NSError **)error {
    return [self readStatusAtRootURL:rootURL recover:NO status:status error:error];
}

+ (FSHInstallationStatus *)recoverInstallationAtRootURL:(NSURL *)rootURL error:(NSError **)error {
    FSHInstallationStatus *status = nil;
    if (![self readStatusAtRootURL:rootURL recover:YES status:&status error:error]) {
        return nil;
    }
    return status;
}

+ (BOOL)discardFinishedInstallationAtRootURL:(NSURL *)rootURL error:(NSError **)error {
    NSString *path = FSHFilePath(rootURL, @"rootURL", error);
    if (path == nil) {
        return NO;
    }
    FSHArena *arena = [[FSHArena alloc] init];
    const char *root = [arena cStringForString:path name:@"rootURL"];
    if (arena.failure != nil) {
        FSH_SET_ERROR(error, arena.failure);
        return NO;
    }
    int32_t status = freshen_discard_finished_installation(root);
    if (status != FRESHEN_OK) {
        FSH_SET_ERROR(error, FSHErrorFromStatus(status, nil));
        return NO;
    }
    return YES;
}

@end
