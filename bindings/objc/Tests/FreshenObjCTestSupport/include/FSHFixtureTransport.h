#import <Foundation/Foundation.h>

#import "FreshenObjC.h"

NS_ASSUME_NONNULL_BEGIN

/// A URL from a literal, failing loudly if it does not parse. (`+URLWithString:` is
/// nullable, and a silent nil in a test would surface as a confusing failure later.)
static inline NSURL *FSHTestURL(NSString *string) {
    NSURL *url = [NSURL URLWithString:string];
    if (url == nil) {
        [NSException raise:NSInternalInconsistencyException format:@"bad test URL %@", string];
    }
    return url;
}

/// Serves the signed fixtures written by the `make_fixtures` example under
/// `https://test.invalid/`, never touching the network. Knobs let tests misbehave
/// on purpose.
@interface FSHFixtureTransport : NSObject <FSHTransport>
- (instancetype)initWithDirectory:(NSString *)directory NS_DESIGNATED_INITIALIZER;
- (instancetype)init NS_UNAVAILABLE;

/// How many times the native library called this transport.
@property(nonatomic, readonly) NSInteger calls;
/// Writes the package twice, ignoring the limit, to prove the sink enforces it.
@property(nonatomic) BOOL overrunPackage;
/// Invoked after the first chunk of every download (for example to cancel mid-download).
@property(nonatomic, copy, nullable) void (^afterFirstChunk)(void);
/// Invoked at the start of every download (for example to block).
@property(nonatomic, copy, nullable) void (^beforeDownload)(void);
@end

NS_ASSUME_NONNULL_END
