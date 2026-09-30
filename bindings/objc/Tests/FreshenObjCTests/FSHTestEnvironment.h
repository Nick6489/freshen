#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

/// Paths prepared by Tools/run-tests.sh, passed in through the environment.
@interface FSHTestEnvironment : NSObject
/// Signed releases 1.5.0 and 2.0.0 whose package installs the confirm_app example.
@property(class, nonatomic, readonly) NSString *fixturesDirectory;
/// The `freshen-helper` executable built by cargo.
@property(class, nonatomic, readonly) NSString *helperPath;
/// The `confirm_app` example built by cargo (what an update installs).
@property(class, nonatomic, readonly) NSString *confirmAppPath;
/// The built FreshenObjCE2E parent application.
@property(class, nonatomic, readonly) NSString *e2ePath;

/// A fresh empty directory under the temporary directory.
+ (NSString *)newScratchDirectoryNamed:(NSString *)name;
/// The number of entries directly inside `directory`.
+ (NSUInteger)entryCountInDirectory:(NSString *)directory;

- (instancetype)init NS_UNAVAILABLE;
@end

NS_ASSUME_NONNULL_END
