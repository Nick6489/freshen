using System;
using System.IO;
using System.Threading;

namespace Freshen.TestSupport;

/// <summary>
/// Serves the signed fixtures written by the <c>make_fixtures</c> example under
/// <c>https://test.invalid/</c>, never touching the network. Knobs let tests misbehave on purpose.
/// </summary>
public sealed class FixtureTransport : IUpdateTransport
{
    private readonly string _directory;
    private int _calls;

    public FixtureTransport(string directory)
    {
        _directory = directory;
    }

    /// <summary>How many times the native library called this transport.</summary>
    public int Calls => Volatile.Read(ref _calls);

    /// <summary>Writes the package twice, ignoring the limit, to prove the sink enforces it.</summary>
    public bool OverrunPackage { get; set; }

    /// <summary>Invoked after the first chunk of every download (for example to cancel mid-download).</summary>
    public Action? AfterFirstChunk { get; set; }

    /// <summary>Invoked at the start of every download (for example to block).</summary>
    public Action? BeforeDownload { get; set; }

    public void Download(Uri url, DownloadSink sink, ulong limit)
    {
        Interlocked.Increment(ref _calls);
        BeforeDownload?.Invoke();
        if (url.Host != "test.invalid")
        {
            throw new IOException($"unexpected host {url.Host}");
        }

        string file = url.AbsolutePath.TrimStart('/') switch
        {
            "index" => "index.json",
            "m/1.5.0" => "v1.5.0.manifest",
            "s/1.5.0" => "v1.5.0.sig",
            "m/2.0.0" => "v2.0.0.manifest",
            "s/2.0.0" => "v2.0.0.sig",
            "bad-sig" => "tampered.sig",
            "package" => "package.zip",
            "huge" => "huge",
            _ => throw new FileNotFoundException($"no fixture for {url}"),
        };

        using FileStream stream = File.OpenRead(Path.Combine(_directory, file));
        if (file != "huge")
        {
            // "huge" deliberately declares no total.
            sink.SetTotal((ulong)stream.Length);
        }

        int passes = OverrunPackage && file == "package.zip" ? 2 : 1;
        byte[] buffer = new byte[4096];
        bool first = true;
        for (int pass = 0; pass < passes; pass++)
        {
            stream.Position = 0;
            int count;
            while ((count = stream.Read(buffer, 0, buffer.Length)) > 0)
            {
                sink.Write(buffer.AsSpan(0, count));
                if (first)
                {
                    first = false;
                    AfterFirstChunk?.Invoke();
                }
            }
        }
    }
}
