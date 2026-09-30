using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Runtime.CompilerServices;
using Freshen.Interop;

namespace Freshen;

/// <summary>
/// Supplies downloads: custom authentication, offline storage, or test fixtures.
/// Implementations run on the thread performing the operation, so blocking is fine
/// and asynchronous work should be awaited synchronously.
/// </summary>
/// <remarks>
/// Every byte must go through the <see cref="DownloadSink"/>. The size limit,
/// cancellation and progress reporting are enforced by the sink inside the native
/// library, so an implementation cannot skip them. URLs that are not HTTPS, or that
/// embed credentials, are rejected before the implementation is called.
/// </remarks>
public interface IUpdateTransport
{
    /// <summary>Fetches <paramref name="url"/> and writes every byte to <paramref name="sink"/>.</summary>
    /// <param name="url">The HTTPS URL to fetch.</param>
    /// <param name="sink">Receives the bytes. Valid only for the duration of this call.</param>
    /// <param name="limit">Informational maximum size; the sink enforces it.</param>
    void Download(Uri url, DownloadSink sink, ulong limit);
}

/// <summary>Receives the bytes of one download. Valid only during <see cref="IUpdateTransport.Download"/>.</summary>
public sealed unsafe class DownloadSink
{
    private nint _handle;

    internal DownloadSink(nint handle)
    {
        _handle = handle;
    }

    internal void Invalidate() => _handle = 0;

    /// <summary>Whether the operation has been cancelled. Poll this while blocked on the network.</summary>
    public bool IsCancelled => NativeMethods.freshen_sink_is_cancelled(Handle) != 0;

    /// <summary>Reports the total size when it is known so progress can show a percentage. A total above the limit fails immediately.</summary>
    /// <exception cref="FreshenException">The total exceeds the size limit.</exception>
    public void SetTotal(ulong totalBytes)
    {
        int status = NativeMethods.freshen_sink_set_total(Handle, totalBytes);
        Check(status);
    }

    /// <summary>Appends downloaded bytes.</summary>
    /// <exception cref="OperationCanceledException">The operation was cancelled.</exception>
    /// <exception cref="FreshenException">The size limit was exceeded or writing failed.</exception>
    public void Write(ReadOnlySpan<byte> data)
    {
        fixed (byte* pointer = data)
        {
            int status = NativeMethods.freshen_sink_write(Handle, pointer, (nuint)data.Length);
            Check(status);
        }
    }

    /// <summary>Copies a stream to the sink until it ends.</summary>
    public void CopyFrom(Stream source)
    {
        ArgumentNullException.ThrowIfNull(source);
        byte[] buffer = new byte[64 * 1024];
        int count;
        while ((count = source.Read(buffer, 0, buffer.Length)) > 0)
        {
            Write(buffer.AsSpan(0, count));
        }
    }

    private nint Handle => _handle != 0
        ? _handle
        : throw new ObjectDisposedException(nameof(DownloadSink), "A DownloadSink is only valid during the Download call it was passed to.");

    private static void Check(int status) => ErrorMapper.ThrowIfError(status);
}

/// <summary>Managed state the native updater carries for its custom transport.</summary>
internal sealed class TransportState
{
    private Exception? _lastError;

    public TransportState(IUpdateTransport transport)
    {
        Transport = transport;
    }

    public IUpdateTransport Transport { get; }

    public void Remember(Exception error) => _lastError = error;

    public Exception? TakeError()
    {
        Exception? error = _lastError;
        _lastError = null;
        return error;
    }
}

internal static unsafe class TransportCallbacks
{
    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    internal static int Download(nint user, byte* url, nint sink, ulong limit)
    {
        // Nothing may unwind into native code: every failure becomes a status code.
        TransportState? state = null;
        var wrapper = new DownloadSink(sink);
        try
        {
            state = (TransportState)GCHandle.FromIntPtr(user).Target!;
            var uri = new Uri(Marshal.PtrToStringUTF8((nint)url)!, UriKind.Absolute);
            state.Transport.Download(uri, wrapper, limit);
            return NativeMethods.Ok;
        }
        catch (FreshenException error)
        {
            state?.Remember(error);
            return (int)error.Code;
        }
        catch (OperationCanceledException error)
        {
            state?.Remember(error);
            return NativeMethods.ErrCancelled;
        }
        catch (Exception error)
        {
            state?.Remember(error);
            return NativeMethods.ErrIo;
        }
        finally
        {
            wrapper.Invalidate();
        }
    }
}

internal sealed class ProgressState
{
    private readonly IProgress<UpdateProgress> _progress;

    public ProgressState(IProgress<UpdateProgress> progress)
    {
        _progress = progress;
    }

    public unsafe void Report(int kind, ulong received, ulong total, byte* path)
    {
        var mapped = kind switch
        {
            NativeMethods.EventChecking => UpdateProgressKind.Checking,
            NativeMethods.EventDownloading => UpdateProgressKind.Downloading,
            NativeMethods.EventVerifying => UpdateProgressKind.Verifying,
            NativeMethods.EventExtracting => UpdateProgressKind.Extracting,
            NativeMethods.EventReady => UpdateProgressKind.Ready,
            _ => UpdateProgressKind.Unknown,
        };
        _progress.Report(new UpdateProgress(
            mapped,
            received,
            total == NativeMethods.TotalUnknown ? null : total,
            path == null ? null : Marshal.PtrToStringUTF8((nint)path)));
    }
}

internal static unsafe class ProgressCallbacks
{
    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    internal static void OnEvent(nint user, int kind, ulong received, ulong total, byte* path)
    {
        try
        {
            ((ProgressState)GCHandle.FromIntPtr(user).Target!).Report(kind, received, total, path);
        }
        catch
        {
            // A misbehaving progress handler must not take down the update or unwind into native code.
        }
    }
}
