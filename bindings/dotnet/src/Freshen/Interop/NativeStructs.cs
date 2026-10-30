using System.Runtime.InteropServices;

namespace Freshen.Interop;

// These mirror the #[repr(C)] structs in ffi/src and include/freshen.h field for
// field. Sequential layout applies the same natural alignment a C compiler would.
// Every struct starts with struct_size so the library can grow them compatibly.

[StructLayout(LayoutKind.Sequential)]
internal unsafe struct NativeTransport
{
    public nuint StructSize;
    public nint User;
    public delegate* unmanaged[Cdecl]<nint, byte*, nint, ulong, int> Download;
}

[StructLayout(LayoutKind.Sequential)]
internal unsafe struct NativeUpdaterConfig
{
    public nuint StructSize;
    public byte* Product;
    public byte* Channel;
    public byte* CurrentVersion;
    public byte* Target;
    public byte* TrustedKeys;
    public nuint TrustedKeyCount;
    public ulong MaxDownload;
    public ulong MaxUnpacked;
    public ulong HttpTimeoutSecs;
    public NativeTransport* Transport;
}

[StructLayout(LayoutKind.Sequential)]
internal unsafe struct NativeLocation
{
    public byte* Document;
    public byte* Signature;
}

[StructLayout(LayoutKind.Sequential)]
internal unsafe struct NativeSource
{
    public nuint StructSize;
    public int Kind;
    public byte* A;
    public byte* B;
    public NativeLocation* Locations;
    public nuint LocationCount;
}

[StructLayout(LayoutKind.Sequential)]
internal unsafe struct NativeInstallPolicy
{
    public nuint StructSize;
    public int Layout;
    public byte* Root;
    public byte* Executable;
    public byte** OwnedFiles;
    public nuint OwnedFileCount;
    public byte** ExpandableDirectories;
    public nuint ExpandableDirectoryCount;
    public byte* BundleName;
    public byte* BundleId;
    public byte* TeamId;
    public ulong ExitTimeoutSecs;
    public ulong ConfirmationTimeoutSecs;
}
