using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;
using System.Threading.Tasks;
using Freshen.Interop;

namespace Freshen;

/// <summary>
/// Checks for and prepares authenticated updates. Operations block, so call them on a
/// worker thread or use the <c>Async</c> methods. An updater runs one operation at a
/// time; a second concurrent call throws <see cref="InvalidOperationException"/>.
/// </summary>
public sealed unsafe class Updater : IDisposable
{
    private readonly UpdaterHandle _handle;
    private readonly TransportState? _transportState;
    private int _busy;

    /// <summary>Creates an updater from the application's update policy.</summary>
    /// <exception cref="ArgumentException">An option is missing or malformed.</exception>
    /// <exception cref="FreshenException">The native library rejected the configuration.</exception>
    public Updater(UpdaterOptions options)
    {
        ArgumentNullException.ThrowIfNull(options);
        if (options.TrustedKeys is null || options.TrustedKeys.Count == 0)
        {
            throw new ArgumentException("At least one trusted public key is required.", nameof(options));
        }

        byte[] keys = new byte[options.TrustedKeys.Count * 32];
        for (int i = 0; i < options.TrustedKeys.Count; i++)
        {
            byte[] key = options.TrustedKeys[i];
            if (key is null || key.Length != 32)
            {
                throw new ArgumentException("Every trusted key must be a 32-byte Ed25519 public key.", nameof(options));
            }

            key.CopyTo(keys, i * 32);
        }

        if (options.HttpTimeout < TimeSpan.Zero)
        {
            throw new ArgumentException("HttpTimeout must not be negative.", nameof(options));
        }

        using var strings = new NativeStrings();
        var config = new NativeUpdaterConfig
        {
            StructSize = (nuint)sizeof(NativeUpdaterConfig),
            Product = strings.Add(options.Product, nameof(options.Product)),
            Channel = strings.Add(options.Channel, nameof(options.Channel)),
            CurrentVersion = strings.Add(options.CurrentVersion, nameof(options.CurrentVersion)),
            Target = strings.Add(options.Target, nameof(options.Target)),
            TrustedKeyCount = (nuint)options.TrustedKeys.Count,
            MaxDownload = options.MaxDownloadBytes,
            MaxUnpacked = options.MaxUnpackedBytes,
            HttpTimeoutSecs = (ulong)Math.Ceiling(options.HttpTimeout.TotalSeconds),
        };

        GCHandle stateHandle = default;
        NativeTransport transport = default;
        if (options.Transport is { } custom)
        {
            _transportState = new TransportState(custom);
            stateHandle = GCHandle.Alloc(_transportState);
            transport = new NativeTransport
            {
                StructSize = (nuint)sizeof(NativeTransport),
                User = GCHandle.ToIntPtr(stateHandle),
                Download = &TransportCallbacks.Download,
            };
            config.Transport = &transport;
        }

        nint raw;
        int status;
        fixed (byte* keyPointer = keys)
        {
            config.TrustedKeys = keyPointer;
            status = NativeMethods.freshen_updater_new(&config, &raw);
        }

        if (status != NativeMethods.Ok)
        {
            if (stateHandle.IsAllocated)
            {
                stateHandle.Free();
            }

            ErrorMapper.ThrowIfError(status);
        }

        _handle = new UpdaterHandle(raw, stateHandle);
    }

    /// <summary>Checks for a newer authenticated release. Blocking.</summary>
    /// <returns>The newest applicable release, or <see langword="null"/> when the application is up to date.</returns>
    /// <exception cref="FreshenException">Any failure. A failed fetch or bad signature is an error, never "up to date".</exception>
    /// <exception cref="OperationCanceledException">The token was cancelled.</exception>
    public Candidate? Check(
        ReleaseSource source,
        IProgress<UpdateProgress>? progress = null,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(source);
        cancellationToken.ThrowIfCancellationRequested();
        using var operation = new Operation(this);
        using var cancel = new CancelBinding(cancellationToken);
        using var strings = new NativeStrings();
        NativeSource native = NativeSourceBuilder.Build(source, strings);
        using var progressState = new ProgressBinding(progress);

        nint candidate;
        int status = NativeMethods.freshen_check(
            operation.Raw,
            &native,
            cancel.Raw,
            progressState.Callback,
            progressState.User,
            &candidate);
        ErrorMapper.ThrowIfError(status, cancellationToken, _transportState?.TakeError());
        return candidate == 0 ? null : Candidate.FromNative(candidate);
    }

    /// <summary>Checks for a newer release on a thread-pool thread.</summary>
    public Task<Candidate?> CheckAsync(
        ReleaseSource source,
        IProgress<UpdateProgress>? progress = null,
        CancellationToken cancellationToken = default) =>
        Task.Run(() => Check(source, progress, cancellationToken), cancellationToken);

    /// <summary>
    /// Downloads, verifies and extracts a candidate into a private directory beneath
    /// <paramref name="temporaryParent"/>. The candidate stays valid. Blocking.
    /// </summary>
    /// <exception cref="FreshenException">Any failure, including a package larger than the configured limit.</exception>
    /// <exception cref="OperationCanceledException">The token was cancelled.</exception>
    public PreparedUpdate Prepare(
        Candidate candidate,
        string temporaryParent,
        IProgress<UpdateProgress>? progress = null,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(candidate);
        ArgumentException.ThrowIfNullOrEmpty(temporaryParent);
        cancellationToken.ThrowIfCancellationRequested();
        using var operation = new Operation(this);
        using var candidateScope = new HandleScope(candidate.Handle);
        using var cancel = new CancelBinding(cancellationToken);
        using var strings = new NativeStrings();
        byte* parent = strings.Add(temporaryParent, nameof(temporaryParent));
        using var progressState = new ProgressBinding(progress);

        nint prepared;
        int status = NativeMethods.freshen_prepare(
            operation.Raw,
            candidateScope.Raw,
            parent,
            cancel.Raw,
            progressState.Callback,
            progressState.User,
            &prepared);
        ErrorMapper.ThrowIfError(status, cancellationToken, _transportState?.TakeError());
        return PreparedUpdate.FromNative(prepared);
    }

    /// <summary>Prepares a candidate on a thread-pool thread.</summary>
    public Task<PreparedUpdate> PrepareAsync(
        Candidate candidate,
        string temporaryParent,
        IProgress<UpdateProgress>? progress = null,
        CancellationToken cancellationToken = default) =>
        Task.Run(() => Prepare(candidate, temporaryParent, progress, cancellationToken), cancellationToken);

    /// <summary>
    /// Releases the native updater. If an operation is running, the release completes
    /// when it finishes.
    /// </summary>
    public void Dispose() => _handle.Dispose();

    /// <summary>Guards one operation: refuses overlap and pins the native handle for the call.</summary>
    private readonly struct Operation : IDisposable
    {
        private readonly Updater _owner;
        private readonly HandleScope _scope;

        public Operation(Updater owner)
        {
            if (Interlocked.CompareExchange(ref owner._busy, 1, 0) != 0)
            {
                throw new InvalidOperationException("An update operation is already running on this Updater.");
            }

            _owner = owner;
            try
            {
                _scope = new HandleScope(owner._handle);
            }
            catch
            {
                Volatile.Write(ref owner._busy, 0);
                throw;
            }
        }

        public nint Raw => _scope.Raw;

        public void Dispose()
        {
            _scope.Dispose();
            Volatile.Write(ref _owner._busy, 0);
        }
    }
}

/// <summary>Allocates the native progress state for one operation and frees it afterwards.</summary>
internal sealed unsafe class ProgressBinding : IDisposable
{
    private GCHandle _state;

    public ProgressBinding(IProgress<UpdateProgress>? progress)
    {
        if (progress is null)
        {
            return;
        }

        _state = GCHandle.Alloc(new ProgressState(progress));
        Callback = &ProgressCallbacks.OnEvent;
        User = GCHandle.ToIntPtr(_state);
    }

    public delegate* unmanaged[Cdecl]<nint, int, ulong, ulong, byte*, void> Callback { get; }

    public nint User { get; }

    public void Dispose()
    {
        if (_state.IsAllocated)
        {
            _state.Free();
        }
    }
}

internal static unsafe class NativeSourceBuilder
{
    public static NativeSource Build(ReleaseSource source, NativeStrings strings)
    {
        var native = new NativeSource
        {
            StructSize = (nuint)sizeof(NativeSource),
            Kind = source.Kind,
        };
        if (source.A is not null)
        {
            native.A = strings.Add(source.A, "source");
        }

        if (source.B is not null)
        {
            native.B = strings.Add(source.B, "source");
        }

        if (source.Locations is { Count: > 0 } locations)
        {
            NativeLocation* array = strings.Allocate<NativeLocation>(locations.Count);
            for (int i = 0; i < locations.Count; i++)
            {
                array[i] = new NativeLocation
                {
                    Document = strings.Add(locations[i].Document.AbsoluteUri, "locations"),
                    Signature = strings.Add(locations[i].Signature.AbsoluteUri, "locations"),
                };
            }

            native.Locations = array;
            native.LocationCount = (nuint)locations.Count;
        }

        return native;
    }
}
