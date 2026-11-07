using System.Runtime.InteropServices;

namespace Cruciverba.Xfill;

/// <summary>P/Invoke declarations of the C interface (include/xfill.h).</summary>
internal static unsafe partial class NativeMethods
{
    internal const string Library = "xfill";

    internal const int Ok = 0;
    internal const int ErrArg = -1;
    internal const int ErrJson = -2;
    internal const int ErrRequest = -3;
    internal const int ErrPanic = -4;
    internal const int Cancelled = -5;
    internal const int ErrData = -6;

    [LibraryImport(Library, EntryPoint = "xfill_version")]
    internal static partial nint Version();

    [LibraryImport(Library, EntryPoint = "xfill_wordlist_load_csv")]
    internal static partial int WordlistLoadCsv(byte* csv, nuint len, out WordlistHandle handle, out nint err);

    [LibraryImport(Library, EntryPoint = "xfill_wordlist_free")]
    internal static partial void WordlistFree(nint wordlist);

    [LibraryImport(Library, EntryPoint = "xfill_wordlist_stats")]
    internal static partial int WordlistStats(WordlistHandle wordlist, out nint statsJson);

    [LibraryImport(Library, EntryPoint = "xfill_generate")]
    internal static partial int Generate(
        WordlistHandle wordlist, byte* requestJson, int* cancel, out nint resultJson, out nint err);

    [LibraryImport(Library, EntryPoint = "xfill_validate")]
    internal static partial int Validate(byte* gridJson, out nint reportJson);

    [LibraryImport(Library, EntryPoint = "xfill_string_free")]
    internal static partial void StringFree(nint s);

    /// <summary>Read and free a string returned by the library (null → null).</summary>
    internal static string? TakeString(nint s)
    {
        if (s == 0)
        {
            return null;
        }
        try
        {
            return Marshal.PtrToStringUTF8(s);
        }
        finally
        {
            StringFree(s);
        }
    }
}

/// <summary>Owns a native dictionary; released with <c>xfill_wordlist_free</c>.</summary>
internal sealed class WordlistHandle : SafeHandle
{
    public WordlistHandle()
        : base(0, ownsHandle: true)
    {
    }

    public override bool IsInvalid => handle == 0;

    protected override bool ReleaseHandle()
    {
        NativeMethods.WordlistFree(handle);
        return true;
    }
}
