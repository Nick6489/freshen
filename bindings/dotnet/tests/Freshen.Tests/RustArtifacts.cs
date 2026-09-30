using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using Xunit;

namespace Freshen.Tests;

/// <summary>
/// Builds the native library, the helper and the example programs with cargo, generates
/// the signed fixtures, and points the bindings at the library. Shared by every test.
/// </summary>
public sealed class RustArtifacts : IDisposable
{
    private readonly string _scratch;

    public RustArtifacts()
    {
        RepoRoot = FindRepoRoot();
        Cargo("build -p freshen-ffi --lib --examples");
        Cargo("build -p freshen --bins");

        string debug = Path.Combine(RepoRoot, "target", "debug");
        LibraryPath = Path.Combine(debug, RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? "freshen_ffi.dll"
            : RuntimeInformation.IsOSPlatform(OSPlatform.OSX) ? "libfreshen_ffi.dylib"
            : "libfreshen_ffi.so");
        string exe = RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? ".exe" : string.Empty;
        HelperPath = Path.Combine(debug, "freshen-helper" + exe);
        ConfirmAppPath = Path.Combine(debug, "examples", "confirm_app" + exe);
        foreach (string path in new[] { LibraryPath, HelperPath, ConfirmAppPath })
        {
            Assert.True(File.Exists(path), $"cargo did not produce {path}");
        }

        // Must be set before the first native call in this process.
        Environment.SetEnvironmentVariable("FRESHEN_FFI_LIBRARY", LibraryPath);

        _scratch = Directory.CreateTempSubdirectory("freshen-dotnet-").FullName;
        FixturesDirectory = Path.Combine(_scratch, "fixtures");
        Run(Path.Combine(debug, "examples", "make_fixtures" + exe), Quote(FixturesDirectory) + " " + Quote(ConfirmAppPath));
    }

    public string RepoRoot { get; }

    /// <summary>The name the packaged application is installed under: <c>application</c>, or
    /// <c>application.exe</c> on Windows, which cannot launch a file without an extension.</summary>
    public string AppName => File.ReadAllText(Path.Combine(FixturesDirectory, "appname"));

    public string LibraryPath { get; }

    public string HelperPath { get; }

    public string ConfirmAppPath { get; }

    /// <summary>Signed releases 1.5.0 and 2.0.0 whose package installs the confirm_app example.</summary>
    public string FixturesDirectory { get; }

    /// <summary>Path of the built end-to-end parent program.</summary>
    public static string E2EAssemblyPath
    {
        get
        {
            // Built by a ProjectReference; it sits beside this assembly's project output.
            string here = AppContext.BaseDirectory;
            string configuration = new DirectoryInfo(here).Parent!.Name;
            string framework = new DirectoryInfo(here).Name;
            string projects = new DirectoryInfo(here).Parent!.Parent!.Parent!.Parent!.FullName;
            return Path.Combine(projects, "Freshen.E2E", "bin", configuration, framework, "Freshen.E2E.dll");
        }
    }

    public string NewScratchDirectory(string name)
    {
        string path = Path.Combine(_scratch, name + "-" + Guid.NewGuid().ToString("N")[..8]);
        Directory.CreateDirectory(path);
        return path;
    }

    public void Dispose()
    {
        try
        {
            Directory.Delete(_scratch, recursive: true);
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException)
        {
            // A helper process may briefly hold a file (Windows refuses to delete an open one);
            // the OS temp cleaner will get it.
        }
    }

    private static string Quote(string value) => "\"" + value + "\"";

    private static string FindRepoRoot()
    {
        for (var directory = new DirectoryInfo(AppContext.BaseDirectory); directory is not null; directory = directory.Parent)
        {
            string manifest = Path.Combine(directory.FullName, "Cargo.toml");
            if (File.Exists(manifest) && File.ReadAllText(manifest).Contains("[workspace]", StringComparison.Ordinal))
            {
                return directory.FullName;
            }
        }

        throw new InvalidOperationException("Could not find the freshen Cargo workspace above " + AppContext.BaseDirectory);
    }

    private void Cargo(string arguments) => Run(Environment.GetEnvironmentVariable("CARGO") ?? "cargo", arguments);

    private void Run(string file, string arguments)
    {
        var info = new ProcessStartInfo(file, arguments)
        {
            WorkingDirectory = RepoRoot,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
        };
        using Process process = Process.Start(info)!;
        string output = process.StandardOutput.ReadToEnd() + process.StandardError.ReadToEnd();
        process.WaitForExit();
        Assert.True(process.ExitCode == 0, $"{file} {arguments} failed ({process.ExitCode}):\n{output}");
    }
}

[CollectionDefinition("rust")]
public sealed class RustCollection : ICollectionFixture<RustArtifacts>
{
}
