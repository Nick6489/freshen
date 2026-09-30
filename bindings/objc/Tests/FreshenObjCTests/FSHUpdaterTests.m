#import <XCTest/XCTest.h>

#import "FSHFixtureTransport.h"
#import "FSHTestEnvironment.h"
#import "FreshenObjC.h"

/// Asserts an error is in FSHErrorDomain with the given code.
#define FSHAssertError(error, expectedCode)                              \
    do {                                                                 \
        XCTAssertNotNil(error);                                          \
        XCTAssertEqualObjects((error).domain, FSHErrorDomain);           \
        XCTAssertEqual((error).code, (NSInteger)(expectedCode));         \
    } while (0)

/// Collects progress reports so tests can inspect them.
@interface FSHRecorder : NSObject
- (FSHProgressHandler)handler;
- (NSArray<FSHProgress *> *)events;
@end

@implementation FSHRecorder {
    NSMutableArray<FSHProgress *> *_events;
}

- (instancetype)init {
    self = [super init];
    if (self != nil) {
        _events = [NSMutableArray array];
    }
    return self;
}

- (FSHProgressHandler)handler {
    return ^(FSHProgress *progress) {
        @synchronized(self) {
            [self->_events addObject:progress];
        }
    };
}

- (NSArray<FSHProgress *> *)events {
    @synchronized(self) {
        return [_events copy];
    }
}

@end

@interface FSHUpdaterTests : XCTestCase
@end

@implementation FSHUpdaterTests

#pragma mark Helpers

- (FSHReleaseSource *)indexSource {
    return [FSHReleaseSource sourceWithIndexURL:FSHTestURL(@"https://test.invalid/index")];
}

- (FSHUpdaterConfiguration *)configurationWithTransport:(FSHFixtureTransport *)transport
                                                current:(NSString *)current
                                            maxDownload:(uint64_t)maxDownload {
    NSData *key = [NSData dataWithContentsOfFile:[FSHTestEnvironment.fixturesDirectory stringByAppendingPathComponent:@"pubkey"]];
    FSHUpdaterConfiguration *configuration = [[FSHUpdaterConfiguration alloc] initWithProduct:@"ffi-test"
                                                                                      channel:@"stable"
                                                                               currentVersion:current
                                                                                       target:@"test-target"
                                                                                  trustedKeys:@[key]];
    configuration.maxDownloadBytes = maxDownload;
    configuration.maxUnpackedBytes = 64ULL * 1024 * 1024;
    configuration.transport = transport;
    return configuration;
}

- (FSHFixtureTransport *)newTransport {
    return [[FSHFixtureTransport alloc] initWithDirectory:FSHTestEnvironment.fixturesDirectory];
}

- (FSHUpdater *)updaterWithTransport:(FSHFixtureTransport *)transport current:(NSString *)current maxDownload:(uint64_t)maxDownload {
    NSError *error = nil;
    FSHUpdater *updater = [[FSHUpdater alloc] initWithConfiguration:[self configurationWithTransport:transport
                                                                                            current:current
                                                                                        maxDownload:maxDownload]
                                                              error:&error];
    XCTAssertNotNil(updater, @"%@", error);
    return updater;
}

- (FSHUpdater *)updaterWithTransport:(FSHFixtureTransport *)transport {
    return [self updaterWithTransport:transport current:@"1.0.0" maxDownload:512ULL * 1024 * 1024];
}

/// Checks and returns the candidate, failing the test if there is none.
- (FSHCandidate *)checkedCandidateFrom:(FSHUpdater *)updater {
    FSHCandidate *candidate = nil;
    NSError *error = nil;
    XCTAssertTrue([updater checkForUpdateFromSource:[self indexSource] cancellation:nil progress:nil candidate:&candidate error:&error], @"%@", error);
    XCTAssertNotNil(candidate);
    return candidate;
}

#pragma mark Library

- (void)testNativeLibraryReportsTheExpectedABI {
    XCTAssertEqual(FSHHost.nativeABIVersion, 1u);
    XCTAssertGreaterThan(FSHHost.nativeLibraryVersion.length, 0u);
}

#pragma mark Check

- (void)testCheckReturnsTheNewestReleaseWithAuthenticatedHistory {
    FSHFixtureTransport *transport = [self newTransport];
    FSHRecorder *recorder = [[FSHRecorder alloc] init];
    FSHUpdater *updater = [self updaterWithTransport:transport];
    FSHCandidate *candidate = nil;
    NSError *error = nil;

    XCTAssertTrue([updater checkForUpdateFromSource:[self indexSource] cancellation:nil progress:recorder.handler candidate:&candidate error:&error], @"%@", error);

    XCTAssertEqualObjects(candidate.version, @"2.0.0");
    XCTAssertEqualObjects(candidate.notes, @"notes two");
    XCTAssertGreaterThan(candidate.downloadSize, 0ULL);
    NSArray<FSHReleaseNote *> *expected = @[[[FSHReleaseNote alloc] initWithVersion:@"1.5.0" notes:@"notes one"],
                                            [[FSHReleaseNote alloc] initWithVersion:@"2.0.0" notes:@"notes two"]];
    XCTAssertEqualObjects(candidate.history, expected);
    // A check reports only that it started; download progress belongs to prepare.
    XCTAssertEqual(recorder.events.count, 1u);
    XCTAssertEqual(recorder.events.firstObject.kind, FSHProgressKindChecking);
}

- (void)testCheckWhenAlreadyCurrentSucceedsWithNoCandidate {
    FSHUpdater *updater = [self updaterWithTransport:[self newTransport] current:@"2.0.0" maxDownload:1024];
    FSHCandidate *candidate = (FSHCandidate *)@"sentinel";
    NSError *error = nil;

    XCTAssertTrue([updater checkForUpdateFromSource:[self indexSource] cancellation:nil progress:nil candidate:&candidate error:&error], @"%@", error);

    XCTAssertNil(candidate);
    XCTAssertNil(error);
}

- (void)testCheckRejectsASignatureThatDoesNotVerify {
    FSHUpdater *updater = [self updaterWithTransport:[self newTransport]];
    FSHReleaseSource *forged = [FSHReleaseSource sourceWithManifestURL:FSHTestURL(@"https://test.invalid/m/2.0.0")
                                                          signatureURL:FSHTestURL(@"https://test.invalid/bad-sig")];
    FSHCandidate *candidate = nil;
    NSError *error = nil;

    XCTAssertFalse([updater checkForUpdateFromSource:forged cancellation:nil progress:nil candidate:&candidate error:&error]);

    FSHAssertError(error, FSHErrorCodeSignature);
    XCTAssertNil(candidate);
}

- (void)testCheckRefusesPlainHTTPAndEmbeddedCredentialsBeforeCallingTheHost {
    for (NSString *document in @[@"http://test.invalid/m/2.0.0", @"https://user:pass@test.invalid/m/2.0.0"]) {
        FSHFixtureTransport *transport = [self newTransport];
        FSHUpdater *updater = [self updaterWithTransport:transport];
        FSHReleaseSource *source = [FSHReleaseSource sourceWithManifestURL:FSHTestURL(document)
                                                              signatureURL:FSHTestURL(@"https://test.invalid/s/2.0.0")];
        FSHCandidate *candidate = nil;
        NSError *error = nil;

        XCTAssertFalse([updater checkForUpdateFromSource:source cancellation:nil progress:nil candidate:&candidate error:&error]);

        FSHAssertError(error, FSHErrorCodeInvalid);
        XCTAssertEqual(transport.calls, 0);
    }
}

- (void)testTheSinkEnforcesTheSizeLimitEvenWhenTheHostIgnoresIt {
    FSHUpdater *updater = [self updaterWithTransport:[self newTransport]];
    FSHReleaseSource *huge = [FSHReleaseSource sourceWithIndexURL:FSHTestURL(@"https://test.invalid/huge")];
    FSHCandidate *candidate = nil;
    NSError *error = nil;

    XCTAssertFalse([updater checkForUpdateFromSource:huge cancellation:nil progress:nil candidate:&candidate error:&error]);

    FSHAssertError(error, FSHErrorCodeSizeLimit);
}

- (void)testAHostFailureSurfacesAsIOWithTheUnderlyingErrorAttached {
    FSHUpdater *updater = [self updaterWithTransport:[self newTransport]];
    FSHReleaseSource *missing = [FSHReleaseSource sourceWithIndexURL:FSHTestURL(@"https://test.invalid/nothing-here")];
    FSHCandidate *candidate = nil;
    NSError *error = nil;

    XCTAssertFalse([updater checkForUpdateFromSource:missing cancellation:nil progress:nil candidate:&candidate error:&error]);

    FSHAssertError(error, FSHErrorCodeIO);
    NSError *underlying = error.userInfo[NSUnderlyingErrorKey];
    XCTAssertEqualObjects(underlying.domain, NSCocoaErrorDomain);
}

- (void)testCheckWithAnAlreadyCancelledTokenFailsWithoutCallingTheHost {
    FSHFixtureTransport *transport = [self newTransport];
    FSHUpdater *updater = [self updaterWithTransport:transport];
    FSHCancellation *cancellation = [[FSHCancellation alloc] init];
    [cancellation cancel];
    FSHCandidate *candidate = nil;
    NSError *error = nil;

    XCTAssertFalse([updater checkForUpdateFromSource:[self indexSource] cancellation:cancellation progress:nil candidate:&candidate error:&error]);

    FSHAssertError(error, FSHErrorCodeCancelled);
    XCTAssertEqual(transport.calls, 0);
}

- (void)testCheckCompletesOnTheRequestedQueue {
    FSHUpdater *updater = [self updaterWithTransport:[self newTransport]];
    XCTestExpectation *done = [self expectationWithDescription:@"check completed"];
    dispatch_queue_t queue = dispatch_queue_create("freshen.test.completion", NULL);

    [updater checkForUpdateFromSource:[self indexSource]
                         cancellation:nil
                             progress:nil
                      completionQueue:queue
                           completion:^(FSHCandidate *candidate, NSError *error) {
                               XCTAssertNil(error);
                               XCTAssertEqualObjects(candidate.version, @"2.0.0");
                               [done fulfill];
                           }];

    [self waitForExpectations:@[done] timeout:60];
}

#pragma mark Prepare

- (void)testPrepareReportsProgressAndReleasingItRemovesTheStagedFiles {
    FSHFixtureTransport *transport = [self newTransport];
    NSString *work = [FSHTestEnvironment newScratchDirectoryNamed:@"prepare"];
    FSHRecorder *recorder = [[FSHRecorder alloc] init];
    FSHUpdater *updater = [self updaterWithTransport:transport];
    FSHCandidate *candidate = [self checkedCandidateFrom:updater];

    @autoreleasepool {
        NSError *error = nil;
        FSHPreparedUpdate *prepared = [updater prepareCandidate:candidate
                                          temporaryDirectoryURL:[NSURL fileURLWithPath:work]
                                                   cancellation:nil
                                                       progress:recorder.handler
                                                          error:&error];

        XCTAssertNotNil(prepared, @"%@", error);
        XCTAssertEqualObjects(prepared.version, @"2.0.0");
        XCTAssertEqualObjects(prepared.notes, @"notes two");
        XCTAssertEqual(prepared.history.count, 2u);
        NSMutableArray<FSHProgress *> *downloading = [NSMutableArray array];
        BOOL sawVerifying = NO;
        BOOL sawExtracting = NO;
        for (FSHProgress *event in recorder.events) {
            if (event.kind == FSHProgressKindDownloading) {
                [downloading addObject:event];
            }
            sawVerifying = sawVerifying || event.kind == FSHProgressKindVerifying;
            sawExtracting = sawExtracting || (event.kind == FSHProgressKindExtracting && [event.path isEqualToString:@"application"]);
        }
        XCTAssertGreaterThan(downloading.count, 0u);
        XCTAssertEqualObjects(downloading.lastObject.totalBytes, @(candidate.downloadSize));
        XCTAssertEqual(downloading.lastObject.bytesReceived, candidate.downloadSize);
        XCTAssertTrue(sawVerifying);
        XCTAssertTrue(sawExtracting);
        XCTAssertEqual(recorder.events.lastObject.kind, FSHProgressKindReady);
        XCTAssertEqual([FSHTestEnvironment entryCountInDirectory:work], 1u);
    }

    XCTAssertEqual([FSHTestEnvironment entryCountInDirectory:work], 0u);
}

- (void)testPrepareEnforcesTheConfiguredDownloadCeiling {
    NSString *work = [FSHTestEnvironment newScratchDirectoryNamed:@"ceiling"];
    FSHUpdater *updater = [self updaterWithTransport:[self newTransport] current:@"1.0.0" maxDownload:10];
    FSHCandidate *candidate = [self checkedCandidateFrom:updater];
    NSError *error = nil;

    FSHPreparedUpdate *prepared = [updater prepareCandidate:candidate temporaryDirectoryURL:[NSURL fileURLWithPath:work] cancellation:nil progress:nil error:&error];

    XCTAssertNil(prepared);
    FSHAssertError(error, FSHErrorCodeSizeLimit);
    XCTAssertEqual([FSHTestEnvironment entryCountInDirectory:work], 0u);
}

- (void)testPrepareStopsAHostThatWritesMoreThanTheSignedPackageSize {
    // Without sink enforcement the surplus would only surface later as a hash
    // mismatch, so SizeLimit here pins the sink's own behavior.
    FSHFixtureTransport *transport = [self newTransport];
    NSString *work = [FSHTestEnvironment newScratchDirectoryNamed:@"overrun"];
    FSHUpdater *updater = [self updaterWithTransport:transport];
    FSHCandidate *candidate = [self checkedCandidateFrom:updater];
    transport.overrunPackage = YES;
    NSError *error = nil;

    FSHPreparedUpdate *prepared = [updater prepareCandidate:candidate temporaryDirectoryURL:[NSURL fileURLWithPath:work] cancellation:nil progress:nil error:&error];

    XCTAssertNil(prepared);
    FSHAssertError(error, FSHErrorCodeSizeLimit);
    XCTAssertEqual([FSHTestEnvironment entryCountInDirectory:work], 0u);
}

- (void)testPrepareCancelledMidDownloadStopsAndLeavesNothingBehind {
    FSHFixtureTransport *transport = [self newTransport];
    NSString *work = [FSHTestEnvironment newScratchDirectoryNamed:@"cancel"];
    FSHUpdater *updater = [self updaterWithTransport:transport];
    FSHCandidate *candidate = [self checkedCandidateFrom:updater];
    FSHCancellation *cancellation = [[FSHCancellation alloc] init];
    transport.afterFirstChunk = ^{
        [cancellation cancel];
    };
    NSError *error = nil;

    FSHPreparedUpdate *prepared = [updater prepareCandidate:candidate temporaryDirectoryURL:[NSURL fileURLWithPath:work] cancellation:cancellation progress:nil error:&error];

    XCTAssertNil(prepared);
    FSHAssertError(error, FSHErrorCodeCancelled);
    XCTAssertEqual([FSHTestEnvironment entryCountInDirectory:work], 0u);
}

#pragma mark Arm

- (void)testArmingConsumesTheUpdateEvenWhenItFails {
    NSString *work = [FSHTestEnvironment newScratchDirectoryNamed:@"arm"];
    FSHUpdater *updater = [self updaterWithTransport:[self newTransport]];
    FSHCandidate *candidate = [self checkedCandidateFrom:updater];
    NSError *error = nil;
    FSHPreparedUpdate *prepared = [updater prepareCandidate:candidate temporaryDirectoryURL:[NSURL fileURLWithPath:work] cancellation:nil progress:nil error:&error];
    XCTAssertNotNil(prepared, @"%@", error);
    NSURL *missingRoot = [NSURL fileURLWithPath:[work stringByAppendingPathComponent:@"does-not-exist"]];
    FSHInstallPolicy *policy = [FSHInstallPolicy portablePolicyWithRootURL:missingRoot executable:@"application" ownedFiles:@[@"application"] expandableDirectories:nil];
    NSURL *helper = [NSURL fileURLWithPath:FSHTestEnvironment.helperPath];

    NSError *first = nil;
    XCTAssertNil([prepared armWithPolicy:policy helperExecutableURL:helper relaunchArguments:nil error:&first]);
    XCTAssertNotNil(first);
    NSError *second = nil;
    XCTAssertNil([prepared armWithPolicy:policy helperExecutableURL:helper relaunchArguments:nil error:&second]);

    FSHAssertError(second, FSHErrorCodeAlreadyConsumed);
}

- (void)testAnInvalidPolicyIsRejectedBeforeTheUpdateIsConsumed {
    NSString *work = [FSHTestEnvironment newScratchDirectoryNamed:@"arm-policy"];
    NSString *root = [FSHTestEnvironment newScratchDirectoryNamed:@"root"];
    FSHUpdater *updater = [self updaterWithTransport:[self newTransport]];
    FSHCandidate *candidate = [self checkedCandidateFrom:updater];
    NSError *error = nil;
    FSHPreparedUpdate *prepared = [updater prepareCandidate:candidate temporaryDirectoryURL:[NSURL fileURLWithPath:work] cancellation:nil progress:nil error:&error];
    XCTAssertNotNil(prepared, @"%@", error);
    NSURL *helper = [NSURL fileURLWithPath:FSHTestEnvironment.helperPath];
    FSHInstallPolicy *tooLong = [FSHInstallPolicy portablePolicyWithRootURL:[NSURL fileURLWithPath:root] executable:@"application" ownedFiles:@[@"application"] expandableDirectories:nil];
    tooLong.exitTimeout = 2 * 60 * 60;

    NSError *rejected = nil;
    XCTAssertNil([prepared armWithPolicy:tooLong helperExecutableURL:helper relaunchArguments:nil error:&rejected]);
    FSHAssertError(rejected, FSHErrorCodeInvalidArgument);

    // Still armable: the failed validation did not burn the prepared update.
    FSHInstallPolicy *valid = [FSHInstallPolicy portablePolicyWithRootURL:[NSURL fileURLWithPath:root] executable:@"application" ownedFiles:@[@"application"] expandableDirectories:nil];
    NSError *armError = nil;
    FSHHandoff *handoff = [prepared armWithPolicy:valid helperExecutableURL:helper relaunchArguments:@[@"success", root] error:&armError];
    XCTAssertNotNil(handoff, @"%@", armError);
    XCTAssertGreaterThan(handoff.helperProcessIdentifier, 0);
    NSError *cancelError = nil;
    XCTAssertTrue([handoff cancelWithError:&cancelError], @"%@", cancelError);
}

#pragma mark Concurrency and lifetime

- (void)testAnUpdaterRunsOneOperationAtATime {
    FSHFixtureTransport *transport = [self newTransport];
    dispatch_semaphore_t entered = dispatch_semaphore_create(0);
    dispatch_semaphore_t release = dispatch_semaphore_create(0);
    transport.beforeDownload = ^{
        dispatch_semaphore_signal(entered);
        dispatch_semaphore_wait(release, DISPATCH_TIME_FOREVER);
    };
    FSHUpdater *updater = [self updaterWithTransport:transport];
    XCTestExpectation *firstDone = [self expectationWithDescription:@"first check finished"];
    [updater checkForUpdateFromSource:[self indexSource]
                         cancellation:nil
                             progress:nil
                      completionQueue:dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0)
                           completion:^(FSHCandidate *candidate, NSError *error) {
                               XCTAssertNil(error);
                               [firstDone fulfill];
                           }];
    XCTAssertEqual(dispatch_semaphore_wait(entered, dispatch_time(DISPATCH_TIME_NOW, 30 * NSEC_PER_SEC)), 0);

    FSHCandidate *candidate = nil;
    NSError *error = nil;
    XCTAssertFalse([updater checkForUpdateFromSource:[self indexSource] cancellation:nil progress:nil candidate:&candidate error:&error]);
    FSHAssertError(error, FSHErrorCodeOperationInProgress);

    dispatch_semaphore_signal(release);
    [self waitForExpectations:@[firstDone] timeout:60];
}

- (void)testConstructionRejectsBadConfigurations {
    NSData *key = [NSData dataWithContentsOfFile:[FSHTestEnvironment.fixturesDirectory stringByAppendingPathComponent:@"pubkey"]];
    NSError *error = nil;

    FSHUpdaterConfiguration *noKeys = [[FSHUpdaterConfiguration alloc] initWithProduct:@"p" channel:@"stable" currentVersion:@"1.0.0" target:@"t" trustedKeys:@[]];
    XCTAssertNil([[FSHUpdater alloc] initWithConfiguration:noKeys error:&error]);
    FSHAssertError(error, FSHErrorCodeInvalidArgument);

    error = nil;
    NSData *shortKey = [key subdataWithRange:NSMakeRange(0, 31)];
    FSHUpdaterConfiguration *badKey = [[FSHUpdaterConfiguration alloc] initWithProduct:@"p" channel:@"stable" currentVersion:@"1.0.0" target:@"t" trustedKeys:@[shortKey]];
    XCTAssertNil([[FSHUpdater alloc] initWithConfiguration:badKey error:&error]);
    FSHAssertError(error, FSHErrorCodeInvalidArgument);

    error = nil;
    FSHUpdaterConfiguration *badVersion = [[FSHUpdaterConfiguration alloc] initWithProduct:@"p" channel:@"stable" currentVersion:@"not-semver" target:@"t" trustedKeys:@[key]];
    XCTAssertNil([[FSHUpdater alloc] initWithConfiguration:badVersion error:&error]);
    FSHAssertError(error, FSHErrorCodeInvalidArgument);
}

- (void)testHostHelpersBehaveOnAnOrdinaryLaunch {
    NSString *root = [FSHTestEnvironment newScratchDirectoryNamed:@"ordinary"];
    NSURL *rootURL = [NSURL fileURLWithPath:root];
    NSError *error = nil;

    BOOL handled = YES;
    XCTAssertTrue([FSHHost runHelperWithArguments:@[@"--not-a-helper"] handled:&handled error:&error], @"%@", error);
    XCTAssertFalse(handled);

    BOOL confirmed = YES;
    XCTAssertTrue([FSHHost confirmStartupForInstallationRootURL:rootURL confirmed:&confirmed error:&error], @"%@", error);
    XCTAssertFalse(confirmed);

    FSHInstallationStatus *status = (FSHInstallationStatus *)@"sentinel";
    XCTAssertTrue([FSHHost installationStatusForRootURL:rootURL status:&status error:&error], @"%@", error);
    XCTAssertNil(status);

    const char *arguments[] = {"program", "--not-a-helper"};
    XCTAssertFalse(FSHRunHelperIfRequested(2, arguments));
}

@end
