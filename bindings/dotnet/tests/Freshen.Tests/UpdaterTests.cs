using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using Freshen.TestSupport;
using Xunit;

namespace Freshen.Tests;

/// <summary>Synchronous progress sink so tests can inspect events deterministically.</summary>
internal sealed class RecordingProgress : IProgress<UpdateProgress>
{
    private readonly object _gate = new();
    private readonly List<UpdateProgress> _events = new();

    public IReadOnlyList<UpdateProgress> Events
    {
        get
        {
            lock (_gate)
            {
                return _events.ToList();
            }
        }
    }

    public void Report(UpdateProgress value)
    {
        lock (_gate)
        {
            _events.Add(value);
        }
    }
}

[Collection("rust")]
public sealed class UpdaterTests
{
    private static readonly ReleaseSource Index = ReleaseSource.Index(new Uri("https://test.invalid/index"));
    private readonly RustArtifacts _rust;

    public UpdaterTests(RustArtifacts rust)
    {
        _rust = rust;
    }

    private UpdaterOptions Options(FixtureTransport transport, string current = "1.0.0", ulong maxDownload = 512UL * 1024 * 1024) => new()
    {
        Product = "ffi-test",
        Channel = "stable",
        CurrentVersion = current,
        Target = "test-target",
        TrustedKeys = new[] { File.ReadAllBytes(Path.Combine(_rust.FixturesDirectory, "pubkey")) },
        MaxDownloadBytes = maxDownload,
        // The debug test application is ~57 MB on Linux and larger on Windows.
        MaxUnpackedBytes = 512UL * 1024 * 1024,
        Transport = transport,
    };

    private FixtureTransport NewTransport() => new(_rust.FixturesDirectory);

    [Fact]
    public void NativeLibraryReportsTheExpectedAbi()
    {
        Assert.Equal(1u, FreshenHost.NativeAbiVersion);
        Assert.False(string.IsNullOrEmpty(FreshenHost.NativeLibraryVersion));
    }

    [Fact]
    public void Check_ReturnsTheNewestReleaseWithAuthenticatedHistory()
    {
        var transport = NewTransport();
        var progress = new RecordingProgress();
        using var updater = new Updater(Options(transport));

        using Candidate candidate = updater.Check(Index, progress)!;

        Assert.NotNull(candidate);
        Assert.Equal("2.0.0", candidate.Version);
        Assert.Equal("notes two", candidate.Notes);
        Assert.True(candidate.DownloadSize > 0);
        Assert.Equal(
            new[] { new ReleaseNote("1.5.0", "notes one"), new ReleaseNote("2.0.0", "notes two") },
            candidate.History);
        // A check reports only that it started; download progress belongs to Prepare.
        Assert.Equal(UpdateProgressKind.Checking, Assert.Single(progress.Events).Kind);
    }

    [Fact]
    public void Check_WhenAlreadyCurrent_ReturnsNullRatherThanAnError()
    {
        using var updater = new Updater(Options(NewTransport(), current: "2.0.0"));
        Assert.Null(updater.Check(Index));
    }

    [Fact]
    public void Check_RejectsASignatureThatDoesNotVerify()
    {
        using var updater = new Updater(Options(NewTransport()));
        var forged = ReleaseSource.Manifest(new Uri("https://test.invalid/m/2.0.0"), new Uri("https://test.invalid/bad-sig"));

        var error = Assert.Throws<FreshenException>(() => updater.Check(forged));

        Assert.Equal(FreshenErrorCode.Signature, error.Code);
    }

    [Theory]
    [InlineData("http://test.invalid/m/2.0.0")]
    [InlineData("https://user:pass@test.invalid/m/2.0.0")]
    public void Check_RefusesPlainHttpAndEmbeddedCredentialsBeforeCallingTheHost(string document)
    {
        var transport = NewTransport();
        using var updater = new Updater(Options(transport));
        var source = ReleaseSource.Manifest(new Uri(document), new Uri("https://test.invalid/s/2.0.0"));

        var error = Assert.Throws<FreshenException>(() => updater.Check(source));

        Assert.Equal(FreshenErrorCode.Invalid, error.Code);
        Assert.Equal(0, transport.Calls);
    }

    [Fact]
    public void TheSinkEnforcesTheSizeLimitEvenWhenTheHostIgnoresIt()
    {
        using var updater = new Updater(Options(NewTransport()));
        var huge = ReleaseSource.Index(new Uri("https://test.invalid/huge"));

        var error = Assert.Throws<FreshenException>(() => updater.Check(huge));

        Assert.Equal(FreshenErrorCode.SizeLimit, error.Code);
    }

    [Fact]
    public void AHostFailureSurfacesAsIoWithTheOriginalExceptionAttached()
    {
        using var updater = new Updater(Options(NewTransport()));
        var missing = ReleaseSource.Index(new Uri("https://test.invalid/nothing-here"));

        var error = Assert.Throws<FreshenException>(() => updater.Check(missing));

        Assert.Equal(FreshenErrorCode.Io, error.Code);
        Assert.IsType<FileNotFoundException>(error.InnerException);
    }

    [Fact]
    public void Check_WithAnAlreadyCancelledToken_ThrowsWithoutCallingTheHost()
    {
        var transport = NewTransport();
        using var updater = new Updater(Options(transport));
        using var cancelled = new CancellationTokenSource();
        cancelled.Cancel();

        Assert.Throws<OperationCanceledException>(() => updater.Check(Index, cancellationToken: cancelled.Token));
        Assert.Equal(0, transport.Calls);
    }

    [Fact]
    public async Task CheckAsync_ReturnsTheCandidate()
    {
        using var updater = new Updater(Options(NewTransport()));
        using Candidate? candidate = await updater.CheckAsync(Index);
        Assert.Equal("2.0.0", candidate?.Version);
    }

    [Fact]
    public void Prepare_ReportsProgressAndDisposingItRemovesTheStagedFiles()
    {
        var transport = NewTransport();
        string work = _rust.NewScratchDirectory("prepare");
        var progress = new RecordingProgress();
        using var updater = new Updater(Options(transport));
        using Candidate candidate = updater.Check(Index)!;

        PreparedUpdate prepared = updater.Prepare(candidate, work, progress);

        Assert.Equal("2.0.0", prepared.Version);
        Assert.Equal("notes two", prepared.Notes);
        Assert.Equal(2, prepared.History.Count);
        var events = progress.Events;
        var downloading = events.Where(e => e.Kind == UpdateProgressKind.Downloading).ToList();
        Assert.NotEmpty(downloading);
        Assert.All(downloading, e => Assert.Equal(candidate.DownloadSize, e.TotalBytes));
        Assert.Equal(candidate.DownloadSize, downloading.Max(e => e.BytesReceived));
        Assert.Contains(events, e => e.Kind == UpdateProgressKind.Verifying);
        Assert.Contains(events, e => e.Kind == UpdateProgressKind.Extracting && e.Path == _rust.AppName);
        Assert.Equal(UpdateProgressKind.Ready, events[^1].Kind);
        Assert.Single(Directory.GetFileSystemEntries(work));

        prepared.Dispose();

        Assert.Empty(Directory.GetFileSystemEntries(work));
    }

    [Fact]
    public void Prepare_EnforcesTheConfiguredDownloadCeiling()
    {
        string work = _rust.NewScratchDirectory("ceiling");
        using var updater = new Updater(Options(NewTransport(), maxDownload: 10));
        using Candidate candidate = updater.Check(Index)!;

        var error = Assert.Throws<FreshenException>(() => updater.Prepare(candidate, work));

        Assert.Equal(FreshenErrorCode.SizeLimit, error.Code);
        Assert.Empty(Directory.GetFileSystemEntries(work));
    }

    [Fact]
    public void Prepare_StopsAHostThatWritesMoreThanTheSignedPackageSize()
    {
        // Without sink enforcement the surplus would only surface later as a hash
        // mismatch, so SizeLimit here pins the sink's own behavior.
        var transport = NewTransport();
        string work = _rust.NewScratchDirectory("overrun");
        using var updater = new Updater(Options(transport));
        using Candidate candidate = updater.Check(Index)!;
        transport.OverrunPackage = true;

        var error = Assert.Throws<FreshenException>(() => updater.Prepare(candidate, work));

        Assert.Equal(FreshenErrorCode.SizeLimit, error.Code);
        Assert.Empty(Directory.GetFileSystemEntries(work));
    }

    [Fact]
    public async Task PrepareAsync_CancelledMidDownload_StopsAndLeavesNothingBehind()
    {
        var transport = NewTransport();
        string work = _rust.NewScratchDirectory("cancel");
        using var updater = new Updater(Options(transport));
        using Candidate candidate = updater.Check(Index)!;
        using var cts = new CancellationTokenSource();
        transport.AfterFirstChunk = cts.Cancel;

        await Assert.ThrowsAnyAsync<OperationCanceledException>(() => updater.PrepareAsync(candidate, work, cancellationToken: cts.Token));

        Assert.Empty(Directory.GetFileSystemEntries(work));
    }

    [Fact]
    public void ArmingConsumesTheUpdateEvenWhenItFails()
    {
        string work = _rust.NewScratchDirectory("arm");
        using var updater = new Updater(Options(NewTransport()));
        using Candidate candidate = updater.Check(Index)!;
        using PreparedUpdate prepared = updater.Prepare(candidate, work);
        var missingRoot = InstallPolicy.Portable(Path.Combine(work, "does-not-exist"), _rust.AppName, new[] { _rust.AppName });

        Assert.Throws<FreshenException>(() => prepared.Arm(missingRoot, _rust.HelperPath));

        Assert.Throws<InvalidOperationException>(() => prepared.Arm(missingRoot, _rust.HelperPath));
    }

    [Fact]
    public void AnInvalidPolicyIsRejectedBeforeTheUpdateIsConsumed()
    {
        string work = _rust.NewScratchDirectory("arm-policy");
        string root = _rust.NewScratchDirectory("root");
        using var updater = new Updater(Options(NewTransport()));
        using Candidate candidate = updater.Check(Index)!;
        using PreparedUpdate prepared = updater.Prepare(candidate, work);
        var tooLong = InstallPolicy.Portable(root, _rust.AppName, new[] { _rust.AppName });
        tooLong.ExitTimeout = TimeSpan.FromHours(2);

        Assert.Throws<ArgumentOutOfRangeException>(() => prepared.Arm(tooLong, _rust.HelperPath));

        // Still armable: the failed validation did not burn the prepared update.
        var valid = InstallPolicy.Portable(root, _rust.AppName, new[] { _rust.AppName });
        using Handoff handoff = prepared.Arm(valid, _rust.HelperPath, new[] { "success", root });
        Assert.True(handoff.HelperProcessId > 0);
        handoff.Cancel();
    }

    [Fact]
    public async Task AnUpdaterRunsOneOperationAtATime()
    {
        var transport = NewTransport();
        using var entered = new ManualResetEventSlim();
        using var release = new ManualResetEventSlim();
        transport.BeforeDownload = () =>
        {
            entered.Set();
            release.Wait();
        };
        using var updater = new Updater(Options(transport));

        Task<Candidate?> first = Task.Run(() => updater.Check(Index));
        Assert.True(entered.Wait(TimeSpan.FromSeconds(30)));
        Assert.Throws<InvalidOperationException>(() => updater.Check(Index));
        release.Set();

        (await first)?.Dispose();
    }

    [Fact]
    public void AnUpdaterCannotBeUsedAfterDispose()
    {
        var updater = new Updater(Options(NewTransport()));
        updater.Dispose();
        updater.Dispose();

        Assert.Throws<ObjectDisposedException>(() => updater.Check(Index));
    }

    [Fact]
    public void ConstructionRejectsBadOptions()
    {
        var transport = NewTransport();

        Assert.Throws<ArgumentException>(() => new Updater(Options(transport).With(keys: Array.Empty<byte[]>())));
        Assert.Throws<ArgumentException>(() => new Updater(Options(transport).With(keys: new[] { new byte[31] })));
        var error = Assert.Throws<FreshenException>(() => new Updater(Options(transport).With(version: "not-semver")));
        Assert.Equal(FreshenErrorCode.InvalidArgument, error.Code);
    }

    [Fact]
    public void HostHelpersBehaveOnAnOrdinaryLaunch()
    {
        string root = _rust.NewScratchDirectory("ordinary");

        Assert.False(FreshenHost.RunHelper(new[] { "--not-a-helper" }));
        Assert.False(FreshenHost.ConfirmStartup(root));
        Assert.Null(FreshenHost.GetInstallationStatus(root));
    }
}

internal static class OptionsExtensions
{
    /// <summary>Copy of the options with selected fields replaced (UpdaterOptions has init-only members).</summary>
    public static UpdaterOptions With(
        this UpdaterOptions options,
        IReadOnlyList<byte[]>? keys = null,
        string? version = null) => new()
        {
            Product = options.Product,
            Channel = options.Channel,
            CurrentVersion = version ?? options.CurrentVersion,
            Target = options.Target,
            TrustedKeys = keys ?? options.TrustedKeys,
            MaxDownloadBytes = options.MaxDownloadBytes,
            MaxUnpackedBytes = options.MaxUnpackedBytes,
            HttpTimeout = options.HttpTimeout,
            Transport = options.Transport,
        };
}
