#import <XCTest/XCTest.h>

#import "FSHTestEnvironment.h"
#import "FreshenObjC.h"

/// Real processes end to end, mirroring the Rust `helper_process` test, the C harness
/// and the .NET tests: a parent application arms an update and exits, the native
/// `freshen-helper` installs it, and a restarted native application confirms (or fails to).
@interface FSHEndToEndTests : XCTestCase
@end

@implementation FSHEndToEndTests

- (void)runScenario:(NSString *)mode expecting:(FSHInstallState)expected {
    NSString *scenario = [FSHTestEnvironment newScratchDirectoryNamed:[@"e2e-" stringByAppendingString:mode]];
    NSString *root = [scenario stringByAppendingPathComponent:@"installation"];
    NSFileManager *files = [NSFileManager defaultManager];
    [files createDirectoryAtPath:root withIntermediateDirectories:YES attributes:nil error:nil];
    [@"old application" writeToFile:[root stringByAppendingPathComponent:@"application"] atomically:YES encoding:NSUTF8StringEncoding error:nil];
    [@"keep me" writeToFile:[root stringByAppendingPathComponent:@"user-settings"] atomically:YES encoding:NSUTF8StringEncoding error:nil];
    NSURL *rootURL = [NSURL fileURLWithPath:root];

    [self runParentWithRoot:root mode:mode];

    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:60];
    FSHInstallationStatus *status = nil;
    while (YES) {
        NSError *error = nil;
        XCTAssertTrue([FSHHost installationStatusForRootURL:rootURL status:&status error:&error], @"%@", error);
        if (status.state == expected) {
            break;
        }
        if ([deadline timeIntervalSinceNow] < 0) {
            XCTFail(@"%@: timed out with state %ld (%@)", mode, (long)status.state, status.detail);
            return;
        }
        [NSThread sleepForTimeInterval:0.1];
    }

    if ([mode isEqualToString:@"timeout"]) {
        // An unconfirmed but running process is left alone; once the fixture exits, recovery rolls back.
        [NSThread sleepForTimeInterval:3];
        NSError *error = nil;
        FSHInstallationStatus *recovered = [FSHHost recoverInstallationAtRootURL:rootURL error:&error];
        XCTAssertNotNil(recovered, @"%@", error);
        XCTAssertEqual(recovered.state, FSHInstallStateRolledBack);
    }

    XCTAssertEqualObjects([NSString stringWithContentsOfFile:[root stringByAppendingPathComponent:@"user-settings"] encoding:NSUTF8StringEncoding error:nil], @"keep me");
    NSData *application = [NSData dataWithContentsOfFile:[root stringByAppendingPathComponent:@"application"]];
    if ([mode isEqualToString:@"success"]) {
        XCTAssertEqualObjects(application, [NSData dataWithContentsOfFile:FSHTestEnvironment.confirmAppPath]);
    } else {
        XCTAssertEqualObjects([[NSString alloc] initWithData:application encoding:NSUTF8StringEncoding], @"old application", @"%@: the old application must be intact", mode);
    }

    // A committed status may be observed just before the helper drops its lock.
    while (YES) {
        NSError *error = nil;
        if ([FSHHost discardFinishedInstallationAtRootURL:rootURL error:&error]) {
            break;
        }
        if (error.code != FSHErrorCodeBusy || [deadline timeIntervalSinceNow] < 0) {
            XCTFail(@"%@: cleanup failed: %@", mode, error);
            return;
        }
        [NSThread sleepForTimeInterval:0.1];
    }
    NSError *error = nil;
    FSHInstallationStatus *after = (FSHInstallationStatus *)@"sentinel";
    XCTAssertTrue([FSHHost installationStatusForRootURL:rootURL status:&after error:&error], @"%@", error);
    XCTAssertNil(after);
}

- (void)runParentWithRoot:(NSString *)root mode:(NSString *)mode {
    NSTask *parent = [[NSTask alloc] init];
    parent.executableURL = [NSURL fileURLWithPath:FSHTestEnvironment.e2ePath];
    parent.arguments = @[FSHTestEnvironment.fixturesDirectory, root, mode, FSHTestEnvironment.helperPath];
    NSPipe *output = [NSPipe pipe];
    parent.standardOutput = output;
    parent.standardError = output;
    NSError *error = nil;
    XCTAssertTrue([parent launchAndReturnError:&error], @"%@", error);
    NSData *captured = [output.fileHandleForReading readDataToEndOfFile];
    [parent waitUntilExit];
    XCTAssertEqual(parent.terminationStatus, 0, @"%@: the parent application failed:\n%@", mode,
                   [[NSString alloc] initWithData:captured encoding:NSUTF8StringEncoding]);
}

- (void)testSuccessCommits {
    [self runScenario:@"success" expecting:FSHInstallStateCommitted];
}

- (void)testCancelLeavesItPrepared {
    [self runScenario:@"cancel" expecting:FSHInstallStatePrepared];
}

- (void)testBusyLockRefusesASecondUpdate {
    [self runScenario:@"busy" expecting:FSHInstallStatePrepared];
}

- (void)testFailedStartupRollsBack {
    [self runScenario:@"fail" expecting:FSHInstallStateRolledBack];
}

- (void)testUnconfirmedStartupFailsThenRecoveryRollsBack {
    [self runScenario:@"timeout" expecting:FSHInstallStateFailed];
}

@end
