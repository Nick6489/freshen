using System;
using System.Collections.Generic;
using System.Linq;
using System.Runtime.InteropServices;
using Freshen.Interop;

namespace Freshen;

/// <summary>Process-level entry points: helper dispatch, startup confirmation and recovery.</summary>
public static unsafe class FreshenHost
{
    /// <summary>The C ABI version of the loaded native library.</summary>
    public static uint NativeAbiVersion => NativeMethods.freshen_abi_version();

    /// <summary>The version of the loaded native library.</summary>
    public static string NativeLibraryVersion =>
        Marshal.PtrToStringUTF8((nint)NativeMethods.freshen_library_version()) ?? string.Empty;

    /// <summary>
    /// Self-helper dispatch. When the application executable doubles as the update helper,
    /// call this first in <c>Main</c>, before any UI or single-instance lock, passing the
    /// command-line arguments (which exclude the program name in .NET).
    /// </summary>
    /// <returns><see langword="true"/> when this process was launched as the helper and has finished; return from <c>Main</c> without starting normally.</returns>
    public static bool RunHelper(IReadOnlyList<string> arguments)
    {
        ArgumentNullException.ThrowIfNull(arguments);
        using var strings = new NativeStrings();
        byte** argv = strings.AddArray(arguments, nameof(arguments));
        byte handled = 0;
        ErrorMapper.ThrowIfError(NativeMethods.freshen_run_helper((nuint)arguments.Count, argv, &handled));
        return handled != 0;
    }

    /// <summary>
    /// Call from the restarted application once its essential initialization has succeeded.
    /// A harmless no-op for an ordinary launch.
    /// </summary>
    /// <param name="installationRoot">The installation directory passed in the <see cref="InstallPolicy"/>.</param>
    /// <returns><see langword="true"/> only when this launch was an update awaiting confirmation.</returns>
    public static bool ConfirmStartup(string installationRoot)
    {
        ArgumentException.ThrowIfNullOrEmpty(installationRoot);
        using var strings = new NativeStrings();
        byte confirmed = 0;
        ErrorMapper.ThrowIfError(NativeMethods.freshen_confirm_startup(strings.Add(installationRoot, nameof(installationRoot)), &confirmed));
        return confirmed != 0;
    }

    /// <summary>Read-only status of the last installation transaction, or <see langword="null"/> if there is none. Safe to show on a later launch.</summary>
    public static InstallationStatus? GetInstallationStatus(string installationRoot) =>
        Status(installationRoot, recover: false);

    /// <summary>
    /// Resumes a pending rollback once every application process is stopped. Run it from a
    /// maintenance process when the target executable may be locked.
    /// </summary>
    public static InstallationStatus RecoverInstallation(string installationRoot) =>
        Status(installationRoot, recover: true)!;

    /// <summary>
    /// Removes a finished (committed or rolled back) or cancelled transaction and its
    /// backups. Backups are kept until you call this.
    /// </summary>
    /// <exception cref="FreshenException"><see cref="FreshenErrorCode.Busy"/> while a helper is still active; retry later.</exception>
    public static void DiscardFinishedInstallation(string installationRoot)
    {
        ArgumentException.ThrowIfNullOrEmpty(installationRoot);
        using var strings = new NativeStrings();
        ErrorMapper.ThrowIfError(NativeMethods.freshen_discard_finished_installation(strings.Add(installationRoot, nameof(installationRoot))));
    }

    private static InstallationStatus? Status(string installationRoot, bool recover)
    {
        ArgumentException.ThrowIfNullOrEmpty(installationRoot);
        using var strings = new NativeStrings();
        byte* root = strings.Add(installationRoot, nameof(installationRoot));
        int state = -1;
        byte* version = null;
        byte* detail = null;
        int status = recover
            ? NativeMethods.freshen_recover_installation(root, &state, &version, &detail)
            : NativeMethods.freshen_installation_status(root, &state, &version, &detail);
        // Take ownership of both strings before deciding whether to throw, so nothing leaks.
        string versionText = NativeMethods.TakeString(version) ?? string.Empty;
        string detailText = NativeMethods.TakeString(detail) ?? string.Empty;
        ErrorMapper.ThrowIfError(status);
        if (state == 0)
        {
            return null;
        }

        var mapped = Enum.IsDefined(typeof(InstallState), state) ? (InstallState)state : InstallState.Unknown;
        return new InstallationStatus(mapped, versionText, detailText);
    }
}
