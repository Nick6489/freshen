using System;
using System.IO;
using System.Threading;
using Freshen;
using Freshen.TestSupport;

// The "application" that performs an update, then exits so the helper can install it.
// Driven by Freshen.Tests, which then watches the installation reach its end state.
//
//   Freshen.E2E FIXTURES INSTALLATION_ROOT MODE HELPER_EXECUTABLE
//
// MODE: success | cancel | fail | timeout | busy. The mode is also passed to the
// restarted application (the confirm_app example), which decides whether to confirm.

// A host whose own executable is the helper would dispatch like this first thing.
// Here the helper is the separate freshen-helper binary, so this returns false.
if (FreshenHost.RunHelper(args))
{
    return 0;
}

if (args.Length != 4)
{
    Console.Error.WriteLine("usage: Freshen.E2E FIXTURES INSTALLATION_ROOT MODE HELPER_EXECUTABLE");
    return 64;
}

string fixtures = args[0], root = args[1], mode = args[2], helper = args[3];
// The installed file name is recorded beside the fixtures (Windows needs a ".exe").
string appName = File.ReadAllText(Path.Combine(fixtures, "appname"));

static void Require(bool condition, string what)
{
    if (!condition)
    {
        Console.Error.WriteLine($"E2E check failed: {what}");
        Environment.Exit(1);
    }
}

using var updater = new Updater(new UpdaterOptions
{
    Product = "ffi-test",
    Channel = "stable",
    CurrentVersion = "1.0.0",
    Target = "test-target",
    TrustedKeys = new[] { File.ReadAllBytes(Path.Combine(fixtures, "pubkey")) },
    Transport = new FixtureTransport(fixtures),
});

using Candidate candidate = updater.Check(ReleaseSource.Manifest(
    new Uri("https://test.invalid/m/2.0.0"),
    new Uri("https://test.invalid/s/2.0.0")))
    ?? throw new InvalidOperationException("expected an update");

string work = Path.Combine(fixtures, "work");
Directory.CreateDirectory(work);
using PreparedUpdate prepared = updater.Prepare(candidate, work);

InstallPolicy policy = InstallPolicy.Portable(root, appName, new[] { appName });
policy.ConfirmationTimeout = mode == "timeout" ? TimeSpan.FromSeconds(1) : TimeSpan.FromSeconds(30);

using Handoff handoff = prepared.Arm(policy, helper, new[] { mode, root });
Require(handoff.HelperProcessId > 0, "helper pid");

// While the helper waits for this process to exit, nothing has changed.
Thread.Sleep(200);
Require(File.ReadAllText(Path.Combine(root, appName)) == "old application", "application untouched before exit");

if (mode == "busy")
{
    // A second update cannot arm while the first holds the installation lock.
    using PreparedUpdate second = updater.Prepare(candidate, work);
    try
    {
        second.Arm(policy, helper, new[] { mode, root });
        Require(false, "second Arm should have failed");
    }
    catch (FreshenException error)
    {
        Require(error.Code == FreshenErrorCode.Busy, $"expected Busy, got {error.Code}: {error.Message}");
    }
}

if (mode is "cancel" or "busy")
{
    handoff.Cancel();
    try
    {
        handoff.Cancel();
        Require(false, "second Cancel should have failed");
    }
    catch (InvalidOperationException)
    {
    }
}

// Disposing the handoff must NOT cancel a requested installation; exiting lets the helper proceed.
return 0;
