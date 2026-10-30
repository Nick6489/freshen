using System;
using System.Collections.Generic;
using System.Linq;

namespace Freshen;

/// <summary>Where to look for signed releases. Every source shares the same signature checks.</summary>
public sealed class ReleaseSource
{
    private ReleaseSource(int kind, string? a, string? b, IReadOnlyList<ManifestLocation>? locations)
    {
        Kind = kind;
        A = a;
        B = b;
        Locations = locations;
    }

    internal int Kind { get; }

    internal string? A { get; }

    internal string? B { get; }

    internal IReadOnlyList<ManifestLocation>? Locations { get; }

    /// <summary>A JSON array of manifest locations (at most 100, 1 MiB). The index only locates manifests; each one is authenticated separately.</summary>
    public static ReleaseSource Index(Uri document) =>
        new(Interop.NativeMethods.SourceIndex, Absolute(document, nameof(document)), null, null);

    /// <summary>A single signed manifest and its detached signature.</summary>
    public static ReleaseSource Manifest(Uri document, Uri signature) =>
        new(Interop.NativeMethods.SourceManifest, Absolute(document, nameof(document)), Absolute(signature, nameof(signature)), null);

    /// <summary>Manifest locations supplied by the host's own discovery (at most 100).</summary>
    public static ReleaseSource Manifests(IEnumerable<ManifestLocation> locations)
    {
        ArgumentNullException.ThrowIfNull(locations);
        var list = locations.ToList();
        if (list.Count > 100)
        {
            throw new ArgumentException("At most 100 manifest locations are supported.", nameof(locations));
        }

        return new(Interop.NativeMethods.SourceManifests, null, null, list);
    }

    /// <summary>The first 100 releases of a GitHub repository. Release metadata itself is not trusted.</summary>
    public static ReleaseSource GitHub(string owner, string repository)
    {
        ArgumentException.ThrowIfNullOrEmpty(owner);
        ArgumentException.ThrowIfNullOrEmpty(repository);
        return new(Interop.NativeMethods.SourceGitHub, owner, repository, null);
    }

    private static string Absolute(Uri uri, string name)
    {
        ArgumentNullException.ThrowIfNull(uri, name);
        if (!uri.IsAbsoluteUri)
        {
            throw new ArgumentException("The URI must be absolute.", name);
        }

        return uri.AbsoluteUri;
    }
}

/// <summary>A signed manifest and its detached signature.</summary>
/// <param name="Document">URL of the manifest.</param>
/// <param name="Signature">URL of the manifest's signature.</param>
public sealed record ManifestLocation(Uri Document, Uri Signature);

/// <summary>Authenticated notes for one release.</summary>
/// <param name="Version">Semantic version.</param>
/// <param name="Notes">Release notes; possibly empty. The host owns formatting, localization and the empty-notes fallback.</param>
public sealed record ReleaseNote(string Version, string Notes);

/// <summary>Configuration for an <see cref="Updater"/>.</summary>
public sealed class UpdaterOptions
{
    /// <summary>Signed product name that releases must match.</summary>
    public required string Product { get; init; }

    /// <summary>Signed channel that releases must match, such as "stable".</summary>
    public required string Channel { get; init; }

    /// <summary>Semantic version of the running application.</summary>
    public required string CurrentVersion { get; init; }

    /// <summary>Distribution target, for example "x86_64-pc-windows-msvc".</summary>
    public required string Target { get; init; }

    /// <summary>Pinned Ed25519 public keys, 32 bytes each. Ship old and new keys together to rotate.</summary>
    public required IReadOnlyList<byte[]> TrustedKeys { get; init; }

    /// <summary>Largest package download accepted. Defaults to 256 MiB.</summary>
    public ulong MaxDownloadBytes { get; init; } = 256UL * 1024 * 1024;

    /// <summary>Largest total unpacked size accepted. Defaults to 1 GiB.</summary>
    public ulong MaxUnpackedBytes { get; init; } = 1024UL * 1024 * 1024;

    /// <summary>Total timeout of the built-in HTTPS transport. Defaults to five minutes. Ignored when <see cref="Transport"/> is set.</summary>
    public TimeSpan HttpTimeout { get; init; } = TimeSpan.FromMinutes(5);

    /// <summary>Optional custom transport for authentication, offline storage or tests. Null uses the built-in HTTPS transport.</summary>
    public IUpdateTransport? Transport { get; init; }
}

/// <summary>The stage of an update operation.</summary>
public enum UpdateProgressKind
{
    /// <summary>An event this version of the bindings does not know. Ignore it.</summary>
    Unknown = 0,

    /// <summary>A check has started.</summary>
    Checking = 1,

    /// <summary>The package is downloading.</summary>
    Downloading = 2,

    /// <summary>The downloaded package is being verified.</summary>
    Verifying = 3,

    /// <summary>A file is being extracted.</summary>
    Extracting = 4,

    /// <summary>The update is fully prepared.</summary>
    Ready = 5,
}

/// <summary>A progress report. Reported on the worker thread; <see cref="Progress{T}"/> marshals to its creating context.</summary>
/// <param name="Kind">The stage.</param>
/// <param name="BytesReceived">Bytes downloaded so far (downloading only).</param>
/// <param name="TotalBytes">Total size when known (downloading only).</param>
/// <param name="Path">The file being extracted (extracting only).</param>
public readonly record struct UpdateProgress(UpdateProgressKind Kind, ulong BytesReceived, ulong? TotalBytes, string? Path);

/// <summary>Where an installation transaction stands.</summary>
public enum InstallState
{
    /// <summary>A state this version of the bindings does not know. Read the detail text.</summary>
    Unknown = 100,

    /// <summary>Staged and waiting; nothing has been replaced.</summary>
    Prepared = 1,

    /// <summary>Files are being replaced.</summary>
    Applying = 2,

    /// <summary>Installed; waiting for the new version to confirm startup.</summary>
    AwaitingConfirmation = 3,

    /// <summary>Installed and confirmed.</summary>
    Committed = 4,

    /// <summary>A failed update is being restored.</summary>
    RollingBack = 5,

    /// <summary>A failed update was restored; the old version is intact.</summary>
    RolledBack = 6,

    /// <summary>The update failed and recovery material was kept. See <see cref="FreshenHost.RecoverInstallation"/>.</summary>
    Failed = 7,
}

/// <summary>The last installation transaction under an installation root.</summary>
/// <param name="State">Where it stands.</param>
/// <param name="Version">The version being installed.</param>
/// <param name="Detail">Human-readable detail for diagnostics.</param>
public sealed record InstallationStatus(InstallState State, string Version, string Detail);

/// <summary>
/// The application's local ownership policy for an install. It is decided by the
/// application and never inferred from a download.
/// </summary>
public sealed class InstallPolicy
{
    private InstallPolicy()
    {
    }

    internal int Layout { get; private init; }

    internal string Root { get; private init; } = string.Empty;

    internal string Executable { get; private init; } = string.Empty;

    internal IReadOnlyList<string> OwnedFiles { get; private init; } = Array.Empty<string>();

    internal IReadOnlyList<string> ExpandableDirectories { get; private init; } = Array.Empty<string>();

    internal string? BundleName { get; private init; }

    internal string? BundleId { get; private init; }

    internal string? TeamId { get; private init; }

    /// <summary>How long the helper waits for this process to exit. Null keeps the default (120 seconds).</summary>
    public TimeSpan? ExitTimeout { get; set; }

    /// <summary>How long the helper waits for the new version to confirm startup. Null keeps the default (120 seconds).</summary>
    public TimeSpan? ConfirmationTimeout { get; set; }

    /// <summary>
    /// A portable Windows/Linux installation directory. Only <paramref name="ownedFiles"/>
    /// are replaced (plus files strictly under <paramref name="expandableDirectories"/>);
    /// the manifest cannot introduce anything else.
    /// </summary>
    /// <param name="root">The existing, writable installation directory.</param>
    /// <param name="executable">Relative path of the executable; must be one of the owned files.</param>
    /// <param name="ownedFiles">Relative paths of every program file the application owns.</param>
    /// <param name="expandableDirectories">Relative directories under which additional signed files may be installed.</param>
    public static InstallPolicy Portable(
        string root,
        string executable,
        IEnumerable<string> ownedFiles,
        IEnumerable<string>? expandableDirectories = null)
    {
        ArgumentException.ThrowIfNullOrEmpty(root);
        ArgumentException.ThrowIfNullOrEmpty(executable);
        ArgumentNullException.ThrowIfNull(ownedFiles);
        return new InstallPolicy
        {
            Layout = Interop.NativeMethods.LayoutPortable,
            Root = root,
            Executable = executable,
            OwnedFiles = ownedFiles.ToList(),
            ExpandableDirectories = expandableDirectories?.ToList() ?? new List<string>(),
        };
    }

    /// <summary>
    /// A macOS application bundle. Freshen does not relocate the bundle; it is replaced where it is.
    /// The signed archive must include the bundle prefix, for example <c>MyApp.app/Contents/MacOS/MyApp</c>.
    /// </summary>
    /// <param name="parentDirectory">The existing directory that contains the bundle.</param>
    /// <param name="bundleName">The bundle's directory name, for example "MyApp.app".</param>
    /// <param name="executable">Relative path of the executable inside the archive.</param>
    /// <param name="bundleId">The expected bundle identifier.</param>
    /// <param name="teamId">The expected Apple Team ID.</param>
    public static InstallPolicy MacBundle(
        string parentDirectory,
        string bundleName,
        string executable,
        string bundleId,
        string teamId)
    {
        ArgumentException.ThrowIfNullOrEmpty(parentDirectory);
        ArgumentException.ThrowIfNullOrEmpty(bundleName);
        ArgumentException.ThrowIfNullOrEmpty(executable);
        ArgumentException.ThrowIfNullOrEmpty(bundleId);
        ArgumentException.ThrowIfNullOrEmpty(teamId);
        return new InstallPolicy
        {
            Layout = Interop.NativeMethods.LayoutMacBundle,
            Root = parentDirectory,
            Executable = executable,
            BundleName = bundleName,
            BundleId = bundleId,
            TeamId = teamId,
        };
    }
}
