// The "application" that performs an update, then exits so the helper can install it.
// Driven by FreshenObjCTests, which then watches the installation reach its end state.
//
//   FreshenObjCE2E FIXTURES INSTALLATION_ROOT MODE HELPER_EXECUTABLE
//
// MODE: success | cancel | fail | timeout | busy. The mode is also passed to the
// restarted application (the confirm_app example), which decides whether to confirm.
#import <Foundation/Foundation.h>

#import "FSHFixtureTransport.h"
#import "FreshenObjC.h"

static void Require(BOOL condition, NSString *what) {
    if (!condition) {
        fprintf(stderr, "E2E check failed: %s\n", what.UTF8String);
        exit(1);
    }
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        // A host whose own executable is the helper would dispatch like this first thing.
        // Here the helper is the separate freshen-helper binary, so this returns NO.
        if (FSHRunHelperIfRequested(argc, argv)) {
            return 0;
        }
        if (argc != 5) {
            fprintf(stderr, "usage: FreshenObjCE2E FIXTURES INSTALLATION_ROOT MODE HELPER_EXECUTABLE\n");
            return 64;
        }
        NSString *fixtures = [NSString stringWithUTF8String:argv[1]];
        NSString *root = [NSString stringWithUTF8String:argv[2]];
        NSString *mode = [NSString stringWithUTF8String:argv[3]];
        NSString *helper = [NSString stringWithUTF8String:argv[4]];

        NSData *key = [NSData dataWithContentsOfFile:[fixtures stringByAppendingPathComponent:@"pubkey"]];
        Require(key.length == 32, @"pubkey fixture");
        FSHUpdaterConfiguration *configuration = [[FSHUpdaterConfiguration alloc] initWithProduct:@"ffi-test"
                                                                                          channel:@"stable"
                                                                                   currentVersion:@"1.0.0"
                                                                                           target:@"test-target"
                                                                                      trustedKeys:@[key]];
        configuration.transport = [[FSHFixtureTransport alloc] initWithDirectory:fixtures];
        NSError *error = nil;
        FSHUpdater *updater = [[FSHUpdater alloc] initWithConfiguration:configuration error:&error];
        Require(updater != nil, [NSString stringWithFormat:@"updater: %@", error]);

        FSHReleaseSource *source =
            [FSHReleaseSource sourceWithManifestURL:FSHTestURL(@"https://test.invalid/m/2.0.0")
                                       signatureURL:FSHTestURL(@"https://test.invalid/s/2.0.0")];
        FSHCandidate *candidate = nil;
        Require([updater checkForUpdateFromSource:source cancellation:nil progress:nil candidate:&candidate error:&error],
                [NSString stringWithFormat:@"check: %@", error]);
        Require(candidate != nil, @"expected an update");

        NSString *work = [fixtures stringByAppendingPathComponent:@"work"];
        [[NSFileManager defaultManager] createDirectoryAtPath:work withIntermediateDirectories:YES attributes:nil error:nil];
        NSURL *workURL = [NSURL fileURLWithPath:work];
        FSHPreparedUpdate *prepared = [updater prepareCandidate:candidate
                                          temporaryDirectoryURL:workURL
                                                   cancellation:nil
                                                       progress:nil
                                                          error:&error];
        Require(prepared != nil, [NSString stringWithFormat:@"prepare: %@", error]);

        FSHInstallPolicy *policy = [FSHInstallPolicy portablePolicyWithRootURL:[NSURL fileURLWithPath:root]
                                                                    executable:@"application"
                                                                    ownedFiles:@[@"application"]
                                                         expandableDirectories:nil];
        policy.confirmationTimeout = [mode isEqualToString:@"timeout"] ? 1 : 30;
        NSURL *helperURL = [NSURL fileURLWithPath:helper];
        FSHHandoff *handoff = [prepared armWithPolicy:policy
                                  helperExecutableURL:helperURL
                                    relaunchArguments:@[mode, root]
                                                error:&error];
        Require(handoff != nil, [NSString stringWithFormat:@"arm: %@", error]);
        Require(handoff.helperProcessIdentifier > 0, @"helper pid");

        // While the helper waits for this process to exit, nothing has changed.
        [NSThread sleepForTimeInterval:0.2];
        NSString *current = [NSString stringWithContentsOfFile:[root stringByAppendingPathComponent:@"application"]
                                                      encoding:NSUTF8StringEncoding
                                                         error:nil];
        Require([current isEqualToString:@"old application"], @"application untouched before exit");

        if ([mode isEqualToString:@"busy"]) {
            // A second update cannot arm while the first holds the installation lock.
            FSHPreparedUpdate *second = [updater prepareCandidate:candidate
                                            temporaryDirectoryURL:workURL
                                                     cancellation:nil
                                                         progress:nil
                                                            error:&error];
            Require(second != nil, @"second prepare");
            NSError *armError = nil;
            FSHHandoff *other = [second armWithPolicy:policy helperExecutableURL:helperURL relaunchArguments:@[mode, root] error:&armError];
            Require(other == nil, @"second arm should have failed");
            Require([armError.domain isEqualToString:FSHErrorDomain] && armError.code == FSHErrorCodeBusy,
                    [NSString stringWithFormat:@"expected Busy, got %@", armError]);
        }
        if ([mode isEqualToString:@"cancel"] || [mode isEqualToString:@"busy"]) {
            Require([handoff cancelWithError:&error], [NSString stringWithFormat:@"cancel: %@", error]);
            NSError *again = nil;
            Require(![handoff cancelWithError:&again] && again.code == FSHErrorCodeAlreadyConsumed,
                    @"second cancel should report already consumed");
        }
        // Releasing the handoff must NOT cancel a requested installation; exiting lets the helper proceed.
    }
    return 0;
}
