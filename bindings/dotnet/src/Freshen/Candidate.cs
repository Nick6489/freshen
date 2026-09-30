using System;
using System.Collections.Generic;
using Freshen.Interop;

namespace Freshen;

/// <summary>A verified newer release that has not been downloaded yet.</summary>
public sealed unsafe class Candidate : IDisposable
{
    private Candidate(CandidateHandle handle, string version, string notes, ulong downloadSize, IReadOnlyList<ReleaseNote> history)
    {
        Handle = handle;
        Version = version;
        Notes = notes;
        DownloadSize = downloadSize;
        History = history;
    }

    /// <summary>Version of the selected (newest) release.</summary>
    public string Version { get; }

    /// <summary>Notes of the selected release; possibly empty.</summary>
    public string Notes { get; }

    /// <summary>Size in bytes of the package that <see cref="Updater.Prepare"/> will download.</summary>
    public ulong DownloadSize { get; }

    /// <summary>
    /// Authenticated releases newer than the installed version, oldest first, through
    /// the selected one. This is the available history, not a guarantee of completeness.
    /// </summary>
    public IReadOnlyList<ReleaseNote> History { get; }

    internal CandidateHandle Handle { get; }

    /// <summary>Releases the native candidate.</summary>
    public void Dispose() => Handle.Dispose();

    /// <summary>Takes ownership of a native candidate and reads its data once, up front.</summary>
    internal static Candidate FromNative(nint raw)
    {
        var handle = new CandidateHandle(raw);
        try
        {
            string version = NativeMethods.TakeString(NativeMethods.freshen_candidate_version(raw)) ?? string.Empty;
            string notes = NativeMethods.TakeString(NativeMethods.freshen_candidate_notes(raw)) ?? string.Empty;
            ulong size = NativeMethods.freshen_candidate_download_size(raw);
            int count = checked((int)NativeMethods.freshen_candidate_history_count(raw));
            var history = new List<ReleaseNote>(count);
            for (int i = 0; i < count; i++)
            {
                history.Add(new ReleaseNote(
                    NativeMethods.TakeString(NativeMethods.freshen_candidate_history_version(raw, (nuint)i)) ?? string.Empty,
                    NativeMethods.TakeString(NativeMethods.freshen_candidate_history_notes(raw, (nuint)i)) ?? string.Empty));
            }

            return new Candidate(handle, version, notes, size, history);
        }
        catch
        {
            handle.Dispose();
            throw;
        }
    }
}
