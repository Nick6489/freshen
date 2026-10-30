using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Runtime.Versioning;

namespace Freshen;

/// <summary>Finds the <c>freshen-helper</c> executable that the NuGet package ships beside the native library.</summary>
internal static class BundledHelper
{
    private static readonly object CopyGate = new();
    private static string? _privateCopy;
    private static string? _privateCopyOf;

    public static string? Find(string searchDirectory)
    {
        string name = OperatingSystem.IsWindows() ? "freshen-helper.exe" : "freshen-helper";
        // Beside the application (a RID-specific publish flattens native assets there), or
        // under runtimes/{rid}/native (a portable build keeps the package layout).
        string[] directories =
        {
            searchDirectory,
            Path.Combine(searchDirectory, "runtimes", CurrentRuntimeIdentifier(), "native"),
        };
        foreach (string directory in directories)
        {
            string candidate = Path.Combine(directory, name);
            if (File.Exists(candidate))
            {
                return OperatingSystem.IsWindows() ? candidate : EnsureExecutable(candidate);
            }
        }

        return null;
    }

    /// <summary>The portable runtime identifier for this process, such as <c>linux-x64</c> or <c>osx-arm64</c>.</summary>
    internal static string CurrentRuntimeIdentifier()
    {
        string os = OperatingSystem.IsWindows() ? "win" : OperatingSystem.IsMacOS() ? "osx" : "linux";
        string architecture = RuntimeInformation.ProcessArchitecture switch
        {
            Architecture.X64 => "x64",
            Architecture.Arm64 => "arm64",
            Architecture.X86 => "x86",
            Architecture.Arm => "arm",
            var other => other.ToString().ToLowerInvariant(),
        };
        return $"{os}-{architecture}";
    }

    /// <summary>
    /// NuGet does not preserve the execute bit, so a packaged helper arrives without it and
    /// could not be run. Set it in place, or, when the install location is not ours to modify,
    /// run a private executable copy from a fresh directory only this user can enter.
    /// </summary>
    [UnsupportedOSPlatform("windows")]
    private static string EnsureExecutable(string path)
    {
        UnixFileMode mode = File.GetUnixFileMode(path);
        if ((mode & UnixFileMode.UserExecute) != 0)
        {
            return path;
        }

        try
        {
            File.SetUnixFileMode(path, mode | UnixFileMode.UserExecute);
            return path;
        }
        catch (Exception error) when (error is UnauthorizedAccessException or IOException)
        {
            lock (CopyGate)
            {
                if (_privateCopy is not null && _privateCopyOf == path && File.Exists(_privateCopy))
                {
                    return _privateCopy;
                }

                // CreateTempSubdirectory makes the directory private to this user (mode 0700).
                string directory = Directory.CreateTempSubdirectory("freshen-helper-").FullName;
                string copy = Path.Combine(directory, Path.GetFileName(path));
                File.Copy(path, copy);
                File.SetUnixFileMode(copy, UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
                _privateCopy = copy;
                _privateCopyOf = path;
                return copy;
            }
        }
    }
}
