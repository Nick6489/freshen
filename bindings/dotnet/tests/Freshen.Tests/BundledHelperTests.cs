using System;
using System.IO;
using System.Runtime.InteropServices;
using Xunit;

namespace Freshen.Tests;

[Collection("rust")]
public sealed class BundledHelperTests
{
    private readonly RustArtifacts _rust;

    public BundledHelperTests(RustArtifacts rust)
    {
        _rust = rust;
    }

    private static string Rid() =>
        (OperatingSystem.IsWindows() ? "win" : OperatingSystem.IsMacOS() ? "osx" : "linux") + "-" +
        RuntimeInformation.ProcessArchitecture.ToString().ToLowerInvariant();

    private static string HelperName => OperatingSystem.IsWindows() ? "freshen-helper.exe" : "freshen-helper";

    private string Plant(string directory)
    {
        Directory.CreateDirectory(directory);
        string path = Path.Combine(directory, HelperName);
        File.WriteAllText(path, "not a real helper");
        if (!OperatingSystem.IsWindows())
        {
            // What NuGet delivers: readable, but not executable.
            File.SetUnixFileMode(path, UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.GroupRead | UnixFileMode.OtherRead);
        }

        return path;
    }

    [Fact]
    public void FindsAHelperBesideTheApplicationAndRestoresItsExecuteBit()
    {
        string directory = _rust.NewScratchDirectory("helper-beside");
        string planted = Plant(directory);

        string? found = FreshenHost.FindBundledHelper(directory);

        Assert.Equal(planted, found);
        if (!OperatingSystem.IsWindows())
        {
            Assert.True((File.GetUnixFileMode(planted) & UnixFileMode.UserExecute) != 0);
        }
    }

    [Fact]
    public void FindsAHelperUnderTheRuntimesLayoutOfAPortableBuild()
    {
        string directory = _rust.NewScratchDirectory("helper-runtimes");
        string planted = Plant(Path.Combine(directory, "runtimes", Rid(), "native"));

        Assert.Equal(planted, FreshenHost.FindBundledHelper(directory));
    }

    [Fact]
    public void ReturnsNullWhenNoHelperIsPackaged()
    {
        Assert.Null(FreshenHost.FindBundledHelper(_rust.NewScratchDirectory("helper-none")));
    }

    [Fact]
    public void AnExecutableHelperIsLeftAlone()
    {
        if (OperatingSystem.IsWindows())
        {
            return;
        }

        string directory = _rust.NewScratchDirectory("helper-exec");
        string planted = Plant(directory);
        UnixFileMode already = UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute | UnixFileMode.GroupRead | UnixFileMode.GroupExecute;
        File.SetUnixFileMode(planted, already);

        Assert.Equal(planted, FreshenHost.FindBundledHelper(directory));
        Assert.Equal(already, File.GetUnixFileMode(planted));
    }
}
