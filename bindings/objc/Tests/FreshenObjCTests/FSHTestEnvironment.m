#import "FSHTestEnvironment.h"

static NSString *FSHRequiredVariable(NSString *name) {
    NSString *value = [NSProcessInfo processInfo].environment[name];
    if (value.length == 0) {
        [NSException raise:NSInternalInconsistencyException
                    format:@"%@ is not set. Run the tests with Tools/run-tests.sh, which builds the fixtures and helper first.", name];
    }
    return value;
}

@implementation FSHTestEnvironment

+ (NSString *)fixturesDirectory {
    return FSHRequiredVariable(@"FRESHEN_TEST_FIXTURES");
}

+ (NSString *)helperPath {
    return FSHRequiredVariable(@"FRESHEN_TEST_HELPER");
}

+ (NSString *)confirmAppPath {
    return FSHRequiredVariable(@"FRESHEN_TEST_CONFIRM_APP");
}

+ (NSString *)e2ePath {
    return FSHRequiredVariable(@"FRESHEN_TEST_E2E");
}

+ (NSString *)newScratchDirectoryNamed:(NSString *)name {
    NSString *unique = [NSString stringWithFormat:@"freshen-objc-%@-%@", name, [NSUUID UUID].UUIDString];
    NSString *path = [NSTemporaryDirectory() stringByAppendingPathComponent:unique];
    [[NSFileManager defaultManager] createDirectoryAtPath:path withIntermediateDirectories:YES attributes:nil error:nil];
    return path;
}

+ (NSUInteger)entryCountInDirectory:(NSString *)directory {
    return [[NSFileManager defaultManager] contentsOfDirectoryAtPath:directory error:nil].count;
}

@end
