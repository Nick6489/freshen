using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

namespace Freshen.Interop;

/// <summary>Owns UTF-8 strings and arrays handed to native code for the duration of one call.</summary>
internal sealed unsafe class NativeStrings : IDisposable
{
    private readonly List<nint> _allocations = new();

    /// <summary>Allocates a NUL-terminated UTF-8 copy. A string containing NUL is rejected because it would silently truncate.</summary>
    public byte* Add(string value, string name)
    {
        ArgumentNullException.ThrowIfNull(value, name);
        if (value.Contains('\0'))
        {
            throw new ArgumentException("The value must not contain a NUL character.", name);
        }

        nint pointer = Marshal.StringToCoTaskMemUTF8(value);
        _allocations.Add(pointer);
        return (byte*)pointer;
    }

    /// <summary>Allocates a native array of NUL-terminated UTF-8 strings.</summary>
    public byte** AddArray(IReadOnlyList<string> values, string name)
    {
        if (values.Count == 0)
        {
            return null;
        }

        byte** array = (byte**)Allocate<nint>(values.Count);
        for (int i = 0; i < values.Count; i++)
        {
            array[i] = Add(values[i], name);
        }

        return array;
    }

    /// <summary>Allocates uninitialized native memory for <paramref name="count"/> elements.</summary>
    public T* Allocate<T>(int count)
        where T : unmanaged
    {
        nint pointer = Marshal.AllocCoTaskMem(checked(count * sizeof(T)));
        _allocations.Add(pointer);
        return (T*)pointer;
    }

    public void Dispose()
    {
        foreach (nint pointer in _allocations)
        {
            Marshal.FreeCoTaskMem(pointer);
        }

        _allocations.Clear();
    }
}
