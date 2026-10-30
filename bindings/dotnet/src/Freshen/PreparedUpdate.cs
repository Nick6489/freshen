using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using Freshen.Interop;

namespace Freshen;

/// <summary>
/// A fully downloaded, verified and staged update. Holding one lets the user defer
/// installation. Disposing it without arming deletes its temporary files and leaves
/// the installation untouched.
/// </summary>
public sealed unsafe class PreparedUpdate : IDisposable
{
    private readonly PreparedHandle _handle;
    private int _consumed;

    private PreparedUpdate(PreparedHandle handle, string version, string notes, IReadOnlyList<ReleaseNote> history)
    {
        _handle = handle;
        Version = version;
        Notes = notes;
        History = history;
    }

    /// <summary>Version of the prepared release.</summary>
    public string Version { get; }

    /// <summary>Notes of the prepared release; possibly empty.</summary>
    public string Notes { get; }

    /// <summary>Authenticated release history retained by the update check, oldest first.</summary>
    public IReadOnlyList<ReleaseNote> History { get; }

    /// <summary>
    /// Copies the verified staging into the installation and starts a helper that waits for
    /// this process to exit. Returns once the helper holds the installation lock. Save your
    /// work and quit, or call <see cref="Handoff.Cancel"/> if shutdown is declined. Blocking.
    /// </summary>
    /// <remarks>This <see cref="PreparedUpdate"/> is consumed whether the call succeeds or fails.</remarks>
    /// <param name="policy">The application's local ownership policy.</param>
    /// <param name="helperExecutable">A self-contained helper executable: the <c>freshen-helper</c> binary shipped with the application, or an executable that calls <see cref="FreshenHost.RunHelper"/> first thing.</param>
    /// <param name="relaunchArguments">Arguments the restarted application receives.</param>
    /// <exception cref="InvalidOperationException">The update was already armed.</exception>
    /// <exception cref="FreshenException">Any failure, for example <see cref="FreshenErrorCode.Busy"/> when another update holds the lock.</exception>
    public Handoff Arm(InstallPolicy policy, string helperExecutable, IEnumerable<string>? relaunchArguments = null)
    {
        ArgumentNullException.ThrowIfNull(policy);
        ArgumentException.ThrowIfNullOrEmpty(helperExecutable);
        var arguments = relaunchArguments?.ToList() ?? new List<string>();

        using var strings = new NativeStrings();
        NativeInstallPolicy native = BuildPolicy(policy, strings);
        byte* helper = strings.Add(helperExecutable, nameof(helperExecutable));
        byte** relaunch = strings.AddArray(arguments, nameof(relaunchArguments));

        if (Interlocked.Exchange(ref _consumed, 1) != 0)
        {
            throw new InvalidOperationException("This update was already armed.");
        }

        using var scope = new HandleScope(_handle);
        nint handoff;
        int status = NativeMethods.freshen_arm(scope.Raw, &native, helper, relaunch, (nuint)arguments.Count, &handoff);
        ErrorMapper.ThrowIfError(status);
        return Handoff.FromNative(handoff);
    }

    /// <summary>Arms the update on a thread-pool thread.</summary>
    public Task<Handoff> ArmAsync(InstallPolicy policy, string helperExecutable, IEnumerable<string>? relaunchArguments = null) =>
        Task.Run(() => Arm(policy, helperExecutable, relaunchArguments));

    /// <summary>Releases the staged files. Safe after <see cref="Arm"/>.</summary>
    public void Dispose() => _handle.Dispose();

    private static NativeInstallPolicy BuildPolicy(InstallPolicy policy, NativeStrings strings)
    {
        var native = new NativeInstallPolicy
        {
            StructSize = (nuint)sizeof(NativeInstallPolicy),
            Layout = policy.Layout,
            Root = strings.Add(policy.Root, "root"),
            Executable = strings.Add(policy.Executable, "executable"),
            ExitTimeoutSecs = Seconds(policy.ExitTimeout, nameof(policy.ExitTimeout)),
            ConfirmationTimeoutSecs = Seconds(policy.ConfirmationTimeout, nameof(policy.ConfirmationTimeout)),
        };
        if (policy.Layout == NativeMethods.LayoutPortable)
        {
            native.OwnedFiles = strings.AddArray(policy.OwnedFiles, "ownedFiles");
            native.OwnedFileCount = (nuint)policy.OwnedFiles.Count;
            native.ExpandableDirectories = strings.AddArray(policy.ExpandableDirectories, "expandableDirectories");
            native.ExpandableDirectoryCount = (nuint)policy.ExpandableDirectories.Count;
        }
        else
        {
            native.BundleName = strings.Add(policy.BundleName!, "bundleName");
            native.BundleId = strings.Add(policy.BundleId!, "bundleId");
            native.TeamId = strings.Add(policy.TeamId!, "teamId");
        }

        return native;
    }

    private static ulong Seconds(TimeSpan? value, string name)
    {
        if (value is null)
        {
            return 0;
        }

        if (value <= TimeSpan.Zero || value > TimeSpan.FromHours(1))
        {
            throw new ArgumentOutOfRangeException(name, value, "Timeouts must be between one second and one hour.");
        }

        return (ulong)Math.Ceiling(value.Value.TotalSeconds);
    }

    /// <summary>Takes ownership of a native prepared update and reads its data once, up front.</summary>
    internal static PreparedUpdate FromNative(nint raw)
    {
        var handle = new PreparedHandle(raw);
        try
        {
            string version = NativeMethods.TakeString(NativeMethods.freshen_prepared_version(raw)) ?? string.Empty;
            string notes = NativeMethods.TakeString(NativeMethods.freshen_prepared_notes(raw)) ?? string.Empty;
            int count = checked((int)NativeMethods.freshen_prepared_history_count(raw));
            var history = new List<ReleaseNote>(count);
            for (int i = 0; i < count; i++)
            {
                history.Add(new ReleaseNote(
                    NativeMethods.TakeString(NativeMethods.freshen_prepared_history_version(raw, (nuint)i)) ?? string.Empty,
                    NativeMethods.TakeString(NativeMethods.freshen_prepared_history_notes(raw, (nuint)i)) ?? string.Empty));
            }

            return new PreparedUpdate(handle, version, notes, history);
        }
        catch
        {
            handle.Dispose();
            throw;
        }
    }
}

/// <summary>
/// An armed update: a helper holds the installation lock and waits for this process to
/// exit. <b>Disposing a handoff does not cancel the installation.</b>
/// </summary>
public sealed class Handoff : IDisposable
{
    private readonly HandoffHandle _handle;
    private int _cancelled;

    private Handoff(HandoffHandle handle, int helperProcessId)
    {
        _handle = handle;
        HelperProcessId = helperProcessId;
    }

    /// <summary>Process id of the waiting helper.</summary>
    public int HelperProcessId { get; }

    /// <summary>Cancels the requested installation because shutdown was declined. Blocking; a handoff can be cancelled once.</summary>
    /// <exception cref="InvalidOperationException">The handoff was already cancelled.</exception>
    public void Cancel()
    {
        if (Interlocked.Exchange(ref _cancelled, 1) != 0)
        {
            throw new InvalidOperationException("This handoff was already cancelled.");
        }

        using var scope = new HandleScope(_handle);
        ErrorMapper.ThrowIfError(NativeMethods.freshen_handoff_cancel(scope.Raw));
    }

    /// <summary>Releases the handle without cancelling the installation.</summary>
    public void Dispose() => _handle.Dispose();

    internal static Handoff FromNative(nint raw)
    {
        var handle = new HandoffHandle(raw);
        return new Handoff(handle, checked((int)NativeMethods.freshen_handoff_helper_pid(raw)));
    }
}
