#import "FSHInternal.h"

@implementation FSHProgress

- (instancetype)initWithKind:(FSHProgressKind)kind
               bytesReceived:(uint64_t)bytesReceived
                  totalBytes:(NSNumber *)totalBytes
                        path:(NSString *)path {
    self = [super init];
    if (self != nil) {
        _kind = kind;
        _bytesReceived = bytesReceived;
        _totalBytes = totalBytes;
        _path = [path copy];
    }
    return self;
}

@end

@implementation FSHReleaseNote

- (instancetype)initWithVersion:(NSString *)version notes:(NSString *)notes {
    self = [super init];
    if (self != nil) {
        _version = [version copy];
        _notes = [notes copy];
    }
    return self;
}

- (BOOL)isEqual:(id)object {
    if (![object isKindOfClass:[FSHReleaseNote class]]) {
        return NO;
    }
    FSHReleaseNote *other = object;
    return [_version isEqualToString:other.version] && [_notes isEqualToString:other.notes];
}

- (NSUInteger)hash {
    return _version.hash ^ _notes.hash;
}

@end

@implementation FSHManifestLocation

- (instancetype)initWithDocumentURL:(NSURL *)documentURL signatureURL:(NSURL *)signatureURL {
    self = [super init];
    if (self != nil) {
        _documentURL = [documentURL copy];
        _signatureURL = [signatureURL copy];
    }
    return self;
}

@end

@implementation FSHReleaseSource {
    int _kind;
    NSString *_a;
    NSString *_b;
    NSArray<FSHManifestLocation *> *_locations;
}

- (instancetype)initWithKind:(int)kind
                           a:(NSString *)a
                           b:(NSString *)b
                   locations:(NSArray<FSHManifestLocation *> *)locations {
    self = [super init];
    if (self != nil) {
        _kind = kind;
        _a = [a copy];
        _b = [b copy];
        _locations = [locations copy];
    }
    return self;
}

+ (instancetype)sourceWithIndexURL:(NSURL *)indexURL {
    return [[self alloc] initWithKind:FRESHEN_SOURCE_INDEX a:indexURL.absoluteString b:nil locations:nil];
}

+ (instancetype)sourceWithManifestURL:(NSURL *)manifestURL signatureURL:(NSURL *)signatureURL {
    return [[self alloc] initWithKind:FRESHEN_SOURCE_MANIFEST
                                    a:manifestURL.absoluteString
                                    b:signatureURL.absoluteString
                            locations:nil];
}

+ (instancetype)sourceWithManifestLocations:(NSArray<FSHManifestLocation *> *)locations {
    return [[self alloc] initWithKind:FRESHEN_SOURCE_MANIFESTS a:nil b:nil locations:locations];
}

+ (instancetype)sourceWithGitHubOwner:(NSString *)owner repository:(NSString *)repository {
    return [[self alloc] initWithKind:FRESHEN_SOURCE_GITHUB a:owner b:repository locations:nil];
}

- (struct FreshenSource)nativeSourceWithArena:(FSHArena *)arena {
    struct FreshenSource native;
    memset(&native, 0, sizeof native);
    native.struct_size = sizeof native;
    native.kind = _kind;
    if (_a != nil) {
        native.a = [arena cStringForString:_a name:@"source"];
    }
    if (_b != nil) {
        native.b = [arena cStringForString:_b name:@"source"];
    }
    if (_locations.count > 0) {
        struct FreshenLocation *entries =
            [arena zeroedBytesOfLength:_locations.count * sizeof(struct FreshenLocation)];
        NSUInteger index = 0;
        for (FSHManifestLocation *location in _locations) {
            // An unusable URL becomes an empty string, which the native library rejects.
            entries[index].document = [arena cStringForString:location.documentURL.absoluteString ?: @"" name:@"location"];
            entries[index].signature = [arena cStringForString:location.signatureURL.absoluteString ?: @"" name:@"location"];
            index++;
        }
        native.locations = entries;
        native.location_count = _locations.count;
    }
    return native;
}

@end

@implementation FSHInstallationStatus

- (instancetype)initWithState:(FSHInstallState)state version:(NSString *)version detail:(NSString *)detail {
    self = [super init];
    if (self != nil) {
        _state = state;
        _version = [version copy];
        _detail = [detail copy];
    }
    return self;
}

@end

@interface FSHInstallPolicy ()
- (instancetype)initPrivate;
@end

@implementation FSHInstallPolicy {
    int _layout;
    NSString *_root;
    NSString *_executable;
    NSArray<NSString *> *_ownedFiles;
    NSArray<NSString *> *_expandableDirectories;
    NSString *_bundleName;
    NSString *_bundleIdentifier;
    NSString *_teamIdentifier;
    NSError *_pathError;
}

+ (instancetype)portablePolicyWithRootURL:(NSURL *)rootURL
                               executable:(NSString *)executable
                               ownedFiles:(NSArray<NSString *> *)ownedFiles
                    expandableDirectories:(NSArray<NSString *> *)expandableDirectories {
    FSHInstallPolicy *policy = [[self alloc] initPrivate];
    policy->_layout = FRESHEN_LAYOUT_PORTABLE;
    NSError *pathError = nil;
    policy->_root = FSHFilePath(rootURL, @"rootURL", &pathError);
    policy->_pathError = pathError;
    policy->_executable = [executable copy];
    policy->_ownedFiles = [ownedFiles copy];
    policy->_expandableDirectories = [expandableDirectories copy] ?: @[];
    return policy;
}

+ (instancetype)macBundlePolicyWithParentDirectoryURL:(NSURL *)parentDirectoryURL
                                           bundleName:(NSString *)bundleName
                                           executable:(NSString *)executable
                                     bundleIdentifier:(NSString *)bundleIdentifier
                                       teamIdentifier:(NSString *)teamIdentifier {
    FSHInstallPolicy *policy = [[self alloc] initPrivate];
    policy->_layout = FRESHEN_LAYOUT_MAC_BUNDLE;
    NSError *pathError = nil;
    policy->_root = FSHFilePath(parentDirectoryURL, @"parentDirectoryURL", &pathError);
    policy->_pathError = pathError;
    policy->_executable = [executable copy];
    policy->_bundleName = [bundleName copy];
    policy->_bundleIdentifier = [bundleIdentifier copy];
    policy->_teamIdentifier = [teamIdentifier copy];
    return policy;
}

/// `init` is unavailable to clients, so the factories construct through this.
- (instancetype)initPrivate {
    return [super init];
}

static BOOL FSHTimeoutIsValid(NSTimeInterval value) {
    return value == 0 || (value >= 0 && value <= 3600);
}

- (struct FreshenInstallPolicy)nativePolicyWithArena:(FSHArena *)arena error:(NSError **)error {
    struct FreshenInstallPolicy native;
    memset(&native, 0, sizeof native);
    if (_pathError != nil) {
        FSH_SET_ERROR(error, _pathError);
        return native;
    }
    if (!FSHTimeoutIsValid(self.exitTimeout) || !FSHTimeoutIsValid(self.confirmationTimeout)) {
        FSH_SET_ERROR(error, FSHMakeError(FSHErrorCodeInvalidArgument,
                                          @"Timeouts must be zero (the default) or between one second and one hour.",
                                          nil));
        return native;
    }
    native.struct_size = sizeof native;
    native.layout = _layout;
    native.root = [arena cStringForString:_root name:@"root"];
    native.executable = [arena cStringForString:_executable name:@"executable"];
    native.exit_timeout_secs = (uint64_t)ceil(self.exitTimeout);
    native.confirmation_timeout_secs = (uint64_t)ceil(self.confirmationTimeout);
    if (_layout == FRESHEN_LAYOUT_PORTABLE) {
        native.owned_files = [arena cStringArrayForStrings:_ownedFiles name:@"ownedFiles"];
        native.owned_file_count = _ownedFiles.count;
        native.expandable_directories = [arena cStringArrayForStrings:_expandableDirectories name:@"expandableDirectories"];
        native.expandable_directory_count = _expandableDirectories.count;
    } else {
        native.bundle_name = [arena cStringForString:_bundleName name:@"bundleName"];
        native.bundle_id = [arena cStringForString:_bundleIdentifier name:@"bundleIdentifier"];
        native.team_id = [arena cStringForString:_teamIdentifier name:@"teamIdentifier"];
    }
    if (arena.failure != nil) {
        FSH_SET_ERROR(error, arena.failure);
    }
    return native;
}

@end
