using System;
using System.Threading;

namespace Freshen.Interop;

/// <summary>Connects a <see cref="CancellationToken"/> to a native cancellation handle for one operation.</summary>
internal sealed class CancelBinding : IDisposable
{
    private readonly CancelHandle? _handle;
    private CancellationTokenRegistration _registration;

    public CancelBinding(CancellationToken token)
    {
        if (!token.CanBeCanceled)
        {
            return;
        }

        _handle = new CancelHandle(NativeMethods.freshen_cancel_new());
        // Registering an already-cancelled token runs the callback inline, which is
        // what we want: the native call then fails fast with "cancelled".
        _registration = token.Register(static state => ((CancelBinding)state!).Cancel(), this);
    }

    /// <summary>The raw native handle, or zero when the token cannot be cancelled.</summary>
    public nint Raw => _handle is { IsClosed: false } handle ? handle.DangerousGetHandle() : 0;

    private void Cancel()
    {
        if (_handle is null)
        {
            return;
        }

        try
        {
            using var scope = new HandleScope(_handle);
            NativeMethods.freshen_cancel_cancel(scope.Raw);
        }
        catch (ObjectDisposedException)
        {
            // The operation finished and the binding was disposed first.
        }
    }

    public void Dispose()
    {
        // Disposing the registration waits for a running callback, so the handle
        // is never freed underneath it.
        _registration.Dispose();
        _handle?.Dispose();
    }
}
