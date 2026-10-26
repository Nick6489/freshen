using System;
using System.Diagnostics;
using System.IO;
using System.Threading;
using Xunit;

namespace Freshen.Tests;

/// <summary>
/// Real processes end to end, mirroring the Rust <c>helper_process</c> test and the C harness:
/// a managed parent application arms an update and exits, the native <c>freshen-helper</c>
/// installs it, and a restarted native application confirms (or fails to).
/// </summary>
[Collection("rust")]
public sealed class EndToEndTests
{
    private readonly RustArtifacts _rust;

    public EndToEndTests(RustArtifacts rust)
    {
        _rust = rust;
    }

    [Theory]
    [InlineData("success", InstallState.Committed)]
    [InlineData("cancel", InstallState.Prepared)]
    [InlineData("busy", InstallState.Prepared)]
    [InlineData("fail", InstallState.RolledBack)]
    [InlineData("timeout", InstallState.Failed)]
    public void UpdateScenario(string mode, InstallState expected)
    {
        string scenario = _rust.NewScratchDirectory("e2e-" + mode);
        string root = Path.Combine(scenario, "installation");
        Directory.CreateDirectory(root);
        File.WriteAllText(Path.Combine(root, _rust.AppName), "old application");
        File.WriteAllText(Path.Combine(root, "user-settings"), "keep me");

        RunParent(root, mode);

        var deadline = DateTime.UtcNow + TimeSpan.FromSeconds(60);
        InstallationStatus? status;
        while ((status = FreshenHost.GetInstallationStatus(root))?.State != expected)
        {
            Assert.True(
                DateTime.UtcNow < deadline,
                $"{mode}: timed out with {status?.State} ({status?.Detail}); helper error: {ReadHelperError(root)}");
            Thread.Sleep(100);
        }

        if (mode == "timeout")
        {
            // An unconfirmed but running process is left alone; once the fixture exits, recovery rolls back.
            Thread.Sleep(TimeSpan.FromSeconds(3));
            Assert.Equal(InstallState.RolledBack, FreshenHost.RecoverInstallation(root).State);
        }

        Assert.Equal("keep me", File.ReadAllText(Path.Combine(root, "user-settings")));
        byte[] application = File.ReadAllBytes(Path.Combine(root, _rust.AppName));
        if (mode == "success")
        {
            Assert.Equal(File.ReadAllBytes(_rust.ConfirmAppPath), application);
        }
        else
        {
            Assert.Equal("old application", System.Text.Encoding.UTF8.GetString(application));
        }

        // A committed status may be observed just before the helper drops its lock.
        while (true)
        {
            try
            {
                FreshenHost.DiscardFinishedInstallation(root);
                break;
            }
            catch (FreshenException error) when (error.Code == FreshenErrorCode.Busy && DateTime.UtcNow < deadline)
            {
                Thread.Sleep(100);
            }
        }

        Assert.Null(FreshenHost.GetInstallationStatus(root));
    }

    private void RunParent(string root, string mode)
    {
        var start = new ProcessStartInfo(Environment.GetEnvironmentVariable("DOTNET_HOST_PATH") ?? "dotnet")
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
        };
        start.ArgumentList.Add(RustArtifacts.E2EAssemblyPath);
        start.ArgumentList.Add(_rust.FixturesDirectory);
        start.ArgumentList.Add(root);
        start.ArgumentList.Add(mode);
        start.ArgumentList.Add(_rust.HelperPath);

        using Process parent = Process.Start(start)!;
        // Drain both pipes at once; see RustArtifacts.Run.
        var stdout = parent.StandardOutput.ReadToEndAsync();
        var stderr = parent.StandardError.ReadToEndAsync();
        parent.WaitForExit();
        string output = stdout.GetAwaiter().GetResult() + stderr.GetAwaiter().GetResult();
        Assert.True(parent.ExitCode == 0, $"{mode}: the parent application failed ({parent.ExitCode}):\n{output}");
    }

    private static string ReadHelperError(string root)
    {
        try
        {
            return File.ReadAllText(Path.Combine(root, ".freshen", "transaction", "helper-error.txt"));
        }
        catch (IOException)
        {
            return "(none)";
        }
    }
}
