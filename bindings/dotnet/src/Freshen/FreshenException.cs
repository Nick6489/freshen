using System;
using System.Threading;
using Freshen.Interop;

namespace Freshen;

/// <summary>Classifies a failure reported by the native freshen library.</summary>
public enum FreshenErrorCode
{
    /// <summary>The operation was cancelled. Surfaces as <see cref="OperationCanceledException"/> from update operations.</summary>
    Cancelled = 1,

    /// <summary>A manifest signature did not verify against the pinned public keys.</summary>
    Signature = 2,

    /// <summary>The update, policy or configuration is invalid.</summary>
    Invalid = 3,

    /// <summary>A package contains an unsafe path.</summary>
    UnsafePath = 4,

    /// <summary>A download or extraction exceeded its configured limit.</summary>
    SizeLimit = 5,

    /// <summary>A downloaded file did not match its signed hash.</summary>
    HashMismatch = 6,

    /// <summary>Another update owns the installation lock. Retry later.</summary>
    Busy = 7,

    /// <summary>The helper process failed.</summary>
    Helper = 8,

    /// <summary>The installation requires recovery before another update.</summary>
    Recovery = 9,

    /// <summary>A filesystem or transport failure, including failures raised by a custom <see cref="IUpdateTransport"/>.</summary>
    Io = 10,

    /// <summary>A manifest or index was not valid JSON.</summary>
    Json = 11,

    /// <summary>The built-in HTTPS transport failed.</summary>
    Http = 12,

    /// <summary>A package archive was malformed.</summary>
    Zip = 13,

    /// <summary>An error the bindings do not know how to classify. Read the message.</summary>
    Unknown = 100,

    /// <summary>The native library rejected an argument.</summary>
    InvalidArgument = 101,

    /// <summary>A panic was caught inside the native library. Discard the objects involved.</summary>
    Panic = 102,
}

/// <summary>An update operation failed.</summary>
public sealed class FreshenException : Exception
{
    /// <summary>Creates an exception for a native failure.</summary>
    public FreshenException(FreshenErrorCode code, string message, Exception? innerException = null)
        : base(message, innerException)
    {
        Code = code;
    }

    /// <summary>The kind of failure.</summary>
    public FreshenErrorCode Code { get; }
}

internal static class ErrorMapper
{
    /// <summary>
    /// Turns a nonzero status into an exception. Must run on the thread that made the
    /// failing call, before any other freshen call, because the message is thread-local.
    /// </summary>
    internal static void ThrowIfError(int status, CancellationToken cancellationToken = default, Exception? cause = null)
    {
        if (status == NativeMethods.Ok)
        {
            return;
        }

        string message = NativeMethods.LastErrorMessage();
        if (status == NativeMethods.ErrCancelled)
        {
            throw new OperationCanceledException(message, cause, cancellationToken);
        }

        throw new FreshenException((FreshenErrorCode)status, message, cause);
    }
}
