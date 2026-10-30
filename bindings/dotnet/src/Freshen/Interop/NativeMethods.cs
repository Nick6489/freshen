using System;
using System.Runtime.InteropServices;

namespace Freshen.Interop;

/// <summary>The C ABI of <c>libfreshen_ffi</c>, exactly as declared in <c>freshen.h</c>.</summary>
internal static unsafe partial class NativeMethods
{
    internal const string Library = "freshen_ffi";

    /// <summary>The ABI version these bindings were written against.</summary>
    internal const uint ExpectedAbiVersion = 1;

    // Status codes (FRESHEN_ERR_*).
    internal const int Ok = 0;
    internal const int ErrCancelled = 1;
    internal const int ErrIo = 10;
    internal const int ErrInvalidArgument = 101;

    // Source kinds (FRESHEN_SOURCE_*), layouts (FRESHEN_LAYOUT_*).
    internal const int SourceIndex = 1;
    internal const int SourceManifest = 2;
    internal const int SourceGitHub = 3;
    internal const int SourceManifests = 4;
    internal const int LayoutPortable = 1;
    internal const int LayoutMacBundle = 2;

    // Progress event kinds (FRESHEN_EVENT_*) and the "size unknown" marker.
    internal const int EventChecking = 1;
    internal const int EventDownloading = 2;
    internal const int EventVerifying = 3;
    internal const int EventExtracting = 4;
    internal const int EventReady = 5;
    internal const ulong TotalUnknown = ulong.MaxValue;

    static NativeMethods()
    {
        NativeLibraryResolver.Install();
        uint actual = freshen_abi_version();
        if (actual != ExpectedAbiVersion)
        {
            throw new InvalidOperationException(
                $"The loaded freshen native library implements C ABI version {actual}, but these " +
                $"bindings require version {ExpectedAbiVersion}. Ship matching versions together.");
        }
    }

    [LibraryImport(Library)] internal static partial uint freshen_abi_version();
    [LibraryImport(Library)] internal static partial byte* freshen_library_version();
    [LibraryImport(Library)] internal static partial byte* freshen_last_error_message();
    [LibraryImport(Library)] internal static partial void freshen_string_free(byte* text);

    [LibraryImport(Library)] internal static partial nint freshen_cancel_new();
    [LibraryImport(Library)] internal static partial void freshen_cancel_cancel(nint cancel);
    [LibraryImport(Library)] internal static partial void freshen_cancel_free(nint cancel);

    [LibraryImport(Library)] internal static partial int freshen_updater_new(NativeUpdaterConfig* config, nint* updater);
    [LibraryImport(Library)] internal static partial void freshen_updater_free(nint updater);

    [LibraryImport(Library)]
    internal static partial int freshen_check(
        nint updater,
        NativeSource* source,
        nint cancel,
        delegate* unmanaged[Cdecl]<nint, int, ulong, ulong, byte*, void> onEvent,
        nint user,
        nint* candidate);

    [LibraryImport(Library)]
    internal static partial int freshen_prepare(
        nint updater,
        nint candidate,
        byte* temporaryParent,
        nint cancel,
        delegate* unmanaged[Cdecl]<nint, int, ulong, ulong, byte*, void> onEvent,
        nint user,
        nint* prepared);

    [LibraryImport(Library)] internal static partial byte* freshen_candidate_version(nint candidate);
    [LibraryImport(Library)] internal static partial byte* freshen_candidate_notes(nint candidate);
    [LibraryImport(Library)] internal static partial ulong freshen_candidate_download_size(nint candidate);
    [LibraryImport(Library)] internal static partial nuint freshen_candidate_history_count(nint candidate);
    [LibraryImport(Library)] internal static partial byte* freshen_candidate_history_version(nint candidate, nuint index);
    [LibraryImport(Library)] internal static partial byte* freshen_candidate_history_notes(nint candidate, nuint index);
    [LibraryImport(Library)] internal static partial void freshen_candidate_free(nint candidate);

    [LibraryImport(Library)] internal static partial byte* freshen_prepared_version(nint prepared);
    [LibraryImport(Library)] internal static partial byte* freshen_prepared_notes(nint prepared);
    [LibraryImport(Library)] internal static partial nuint freshen_prepared_history_count(nint prepared);
    [LibraryImport(Library)] internal static partial byte* freshen_prepared_history_version(nint prepared, nuint index);
    [LibraryImport(Library)] internal static partial byte* freshen_prepared_history_notes(nint prepared, nuint index);
    [LibraryImport(Library)] internal static partial void freshen_prepared_free(nint prepared);

    [LibraryImport(Library)]
    internal static partial int freshen_arm(
        nint prepared,
        NativeInstallPolicy* policy,
        byte* helperExecutable,
        byte** relaunchArgs,
        nuint relaunchArgCount,
        nint* handoff);

    [LibraryImport(Library)] internal static partial uint freshen_handoff_helper_pid(nint handoff);
    [LibraryImport(Library)] internal static partial int freshen_handoff_cancel(nint handoff);
    [LibraryImport(Library)] internal static partial void freshen_handoff_free(nint handoff);

    [LibraryImport(Library)] internal static partial int freshen_installation_status(byte* root, int* state, byte** version, byte** detail);
    [LibraryImport(Library)] internal static partial int freshen_recover_installation(byte* root, int* state, byte** version, byte** detail);
    [LibraryImport(Library)] internal static partial int freshen_discard_finished_installation(byte* root);
    [LibraryImport(Library)] internal static partial int freshen_run_helper(nuint argc, byte** argv, byte* handled);
    [LibraryImport(Library)] internal static partial int freshen_confirm_startup(byte* root, byte* confirmed);

    [LibraryImport(Library)] internal static partial int freshen_sink_write(nint sink, byte* data, nuint length);
    [LibraryImport(Library)] internal static partial int freshen_sink_set_total(nint sink, ulong total);
    [LibraryImport(Library)] internal static partial byte freshen_sink_is_cancelled(nint sink);

    /// <summary>Message for the last failure on the calling thread. Call it on the same thread, right after the failing call.</summary>
    internal static string LastErrorMessage()
    {
        byte* message = freshen_last_error_message();
        return message == null ? string.Empty : Marshal.PtrToStringUTF8((nint)message) ?? string.Empty;
    }

    /// <summary>Copies a library-owned string and frees it. Null stays null.</summary>
    internal static string? TakeString(byte* text)
    {
        if (text == null)
        {
            return null;
        }

        try
        {
            return Marshal.PtrToStringUTF8((nint)text);
        }
        finally
        {
            freshen_string_free(text);
        }
    }
}
