using System;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace Freshen.Interop;

/// <summary>Base for owned native handles. The runtime keeps a handle alive while a call is using it.</summary>
internal abstract class NativeHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    protected NativeHandle(nint raw)
        : base(ownsHandle: true)
    {
        SetHandle(raw);
    }
}

internal sealed class UpdaterHandle : NativeHandle
{
    private GCHandle _transportState;

    public UpdaterHandle(nint raw, GCHandle transportState)
        : base(raw)
    {
        _transportState = transportState;
    }

    protected override bool ReleaseHandle()
    {
        NativeMethods.freshen_updater_free(handle);
        // The native updater may call the transport until it is freed, so the
        // managed state it points at can only be released afterwards.
        if (_transportState.IsAllocated)
        {
            _transportState.Free();
        }

        return true;
    }
}

internal sealed class CancelHandle : NativeHandle
{
    public CancelHandle(nint raw)
        : base(raw)
    {
    }

    protected override bool ReleaseHandle()
    {
        NativeMethods.freshen_cancel_free(handle);
        return true;
    }
}

internal sealed class CandidateHandle : NativeHandle
{
    public CandidateHandle(nint raw)
        : base(raw)
    {
    }

    protected override bool ReleaseHandle()
    {
        NativeMethods.freshen_candidate_free(handle);
        return true;
    }
}

internal sealed class PreparedHandle : NativeHandle
{
    public PreparedHandle(nint raw)
        : base(raw)
    {
    }

    protected override bool ReleaseHandle()
    {
        // Deletes the staged files unless the update was armed.
        NativeMethods.freshen_prepared_free(handle);
        return true;
    }
}

internal sealed class HandoffHandle : NativeHandle
{
    public HandoffHandle(nint raw)
        : base(raw)
    {
    }

    protected override bool ReleaseHandle()
    {
        // Freeing does not cancel a requested installation.
        NativeMethods.freshen_handoff_free(handle);
        return true;
    }
}

/// <summary>Pins a <see cref="SafeHandle"/> for the duration of a native call.</summary>
internal readonly struct HandleScope : IDisposable
{
    private readonly SafeHandle _handle;

    public HandleScope(SafeHandle handle)
    {
        bool success = false;
        handle.DangerousAddRef(ref success);
        _handle = handle;
        Raw = handle.DangerousGetHandle();
    }

    public nint Raw { get; }

    public void Dispose() => _handle.DangerousRelease();
}
