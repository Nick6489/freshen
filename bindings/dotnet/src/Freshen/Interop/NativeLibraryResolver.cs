using System;
using System.Reflection;
using System.Runtime.InteropServices;

namespace Freshen.Interop;

/// <summary>
/// Lets the host point at the native library explicitly. Without the override the
/// runtime's normal probing applies, which finds the library beside the application
/// or under <c>runtimes/{rid}/native</c> in a NuGet package.
/// </summary>
internal static class NativeLibraryResolver
{
    /// <summary>Environment variable holding the full path of the freshen native library.</summary>
    internal const string OverrideVariable = "FRESHEN_FFI_LIBRARY";

    internal static void Install()
    {
        NativeLibrary.SetDllImportResolver(typeof(NativeLibraryResolver).Assembly, Resolve);
    }

    private static IntPtr Resolve(string name, Assembly assembly, DllImportSearchPath? searchPath)
    {
        if (name != NativeMethods.Library)
        {
            return IntPtr.Zero;
        }

        string? path = Environment.GetEnvironmentVariable(OverrideVariable);
        return string.IsNullOrEmpty(path) ? IntPtr.Zero : NativeLibrary.Load(path);
    }
}
