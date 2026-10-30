#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

#pragma mark - Progress

/// The stage of an update operation.
typedef NS_ENUM(NSInteger, FSHProgressKind) {
    /// An event this version of the bindings does not know. Ignore it.
    FSHProgressKindUnknown = 0,
    /// A check has started.
    FSHProgressKindChecking = 1,
    /// The package is downloading.
    FSHProgressKindDownloading = 2,
    /// The downloaded package is being verified.
    FSHProgressKindVerifying = 3,
    /// A file is being extracted.
    FSHProgressKindExtracting = 4,
    /// The update is fully prepared.
    FSHProgressKindReady = 5,
};

/// A progress report.
@interface FSHProgress : NSObject
@property(nonatomic, readonly) FSHProgressKind kind;
/// Bytes downloaded so far (downloading only).
@property(nonatomic, readonly) uint64_t bytesReceived;
/// Total size in bytes when known (downloading only).
@property(nonatomic, readonly, nullable) NSNumber *totalBytes;
/// The file being extracted (extracting only).
@property(nonatomic, readonly, copy, nullable) NSString *path;

- (instancetype)initWithKind:(FSHProgressKind)kind
               bytesReceived:(uint64_t)bytesReceived
                  totalBytes:(nullable NSNumber *)totalBytes
                        path:(nullable NSString *)path NS_DESIGNATED_INITIALIZER;
- (instancetype)init NS_UNAVAILABLE;
@end

/// Receives progress. It is called on the worker thread performing the operation;
/// hop to the main queue yourself before touching UI.
typedef void (^FSHProgressHandler)(FSHProgress *progress);

#pragma mark - Release notes and sources

/// Authenticated notes for one release.
@interface FSHReleaseNote : NSObject
@property(nonatomic, readonly, copy) NSString *version;
/// Possibly empty. The host owns formatting, localization and the empty-notes fallback.
@property(nonatomic, readonly, copy) NSString *notes;

- (instancetype)initWithVersion:(NSString *)version notes:(NSString *)notes NS_DESIGNATED_INITIALIZER;
- (instancetype)init NS_UNAVAILABLE;
@end

/// A signed manifest and its detached signature.
@interface FSHManifestLocation : NSObject
@property(nonatomic, readonly, copy) NSURL *documentURL;
@property(nonatomic, readonly, copy) NSURL *signatureURL;

- (instancetype)initWithDocumentURL:(NSURL *)documentURL
                       signatureURL:(NSURL *)signatureURL NS_DESIGNATED_INITIALIZER;
- (instancetype)init NS_UNAVAILABLE;
@end

/// Where to look for signed releases. Every source shares the same signature checks.
@interface FSHReleaseSource : NSObject
/// A JSON array of manifest locations (at most 100, 1 MiB). The index only locates
/// manifests; each one is authenticated separately.
+ (instancetype)sourceWithIndexURL:(NSURL *)indexURL;
/// A single signed manifest and its detached signature.
+ (instancetype)sourceWithManifestURL:(NSURL *)manifestURL signatureURL:(NSURL *)signatureURL;
/// Manifest locations supplied by the host's own discovery (at most 100).
+ (instancetype)sourceWithManifestLocations:(NSArray<FSHManifestLocation *> *)locations;
/// The first 100 releases of a GitHub repository. Release metadata itself is not trusted.
+ (instancetype)sourceWithGitHubOwner:(NSString *)owner repository:(NSString *)repository;
- (instancetype)init NS_UNAVAILABLE;
@end

#pragma mark - Installation

/// Where an installation transaction stands.
typedef NS_ENUM(NSInteger, FSHInstallState) {
    /// A state this version of the bindings does not know. Read the detail text.
    FSHInstallStateUnknown = 100,
    /// Staged and waiting; nothing has been replaced.
    FSHInstallStatePrepared = 1,
    /// Files are being replaced.
    FSHInstallStateApplying = 2,
    /// Installed; waiting for the new version to confirm startup.
    FSHInstallStateAwaitingConfirmation = 3,
    /// Installed and confirmed.
    FSHInstallStateCommitted = 4,
    /// A failed update is being restored.
    FSHInstallStateRollingBack = 5,
    /// A failed update was restored; the old version is intact.
    FSHInstallStateRolledBack = 6,
    /// The update failed and recovery material was kept.
    FSHInstallStateFailed = 7,
};

/// The last installation transaction under an installation root.
@interface FSHInstallationStatus : NSObject
@property(nonatomic, readonly) FSHInstallState state;
@property(nonatomic, readonly, copy) NSString *version;
/// Human-readable detail for diagnostics.
@property(nonatomic, readonly, copy) NSString *detail;

- (instancetype)initWithState:(FSHInstallState)state
                      version:(NSString *)version
                       detail:(NSString *)detail NS_DESIGNATED_INITIALIZER;
- (instancetype)init NS_UNAVAILABLE;
@end

/// The application's local ownership policy for an install. It is decided by the
/// application and never inferred from a download.
@interface FSHInstallPolicy : NSObject
/// A portable directory of application-owned files. Only `ownedFiles` are replaced
/// (plus files strictly beneath `expandableDirectories`); the manifest cannot
/// introduce anything else.
///
/// @param rootURL The existing, writable installation directory.
/// @param executable Relative path of the executable; must be one of the owned files.
/// @param ownedFiles Relative paths of every program file the application owns.
/// @param expandableDirectories Relative directories under which additional signed files may be installed.
+ (instancetype)portablePolicyWithRootURL:(NSURL *)rootURL
                               executable:(NSString *)executable
                               ownedFiles:(NSArray<NSString *> *)ownedFiles
                    expandableDirectories:(nullable NSArray<NSString *> *)expandableDirectories;

/// A macOS application bundle, replaced where it is (never moved to /Applications).
/// The signed archive must include the bundle prefix, for example
/// `MyApp.app/Contents/MacOS/MyApp`, and the bundle must be signed and notarized
/// before packaging.
///
/// @param parentDirectoryURL The existing directory that contains the bundle.
/// @param bundleName The bundle's directory name, for example `MyApp.app`.
/// @param executable Relative path of the executable inside the archive.
/// @param bundleIdentifier The expected bundle identifier.
/// @param teamIdentifier The expected Apple Team ID.
+ (instancetype)macBundlePolicyWithParentDirectoryURL:(NSURL *)parentDirectoryURL
                                           bundleName:(NSString *)bundleName
                                           executable:(NSString *)executable
                                     bundleIdentifier:(NSString *)bundleIdentifier
                                       teamIdentifier:(NSString *)teamIdentifier;

/// How long the helper waits for this process to exit. Zero keeps the default (120 seconds).
@property(nonatomic) NSTimeInterval exitTimeout;
/// How long the helper waits for the new version to confirm startup. Zero keeps the default (120 seconds).
@property(nonatomic) NSTimeInterval confirmationTimeout;

- (instancetype)init NS_UNAVAILABLE;
@end

NS_ASSUME_NONNULL_END
