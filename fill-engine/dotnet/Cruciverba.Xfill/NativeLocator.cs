using System.Runtime.InteropServices;

namespace Cruciverba.Xfill;

/// <summary>Finds <c>xfill-host</c> and the engine library shipped with the package.</summary>
internal static class NativeLocator
{
    /// <summary>Environment variable with the host path (overrides the search).</summary>
    internal const string HostVariable = "XFILL_HOST_PATH";

    internal static string HostName => OperatingSystem.IsWindows() ? "xfill-host.exe" : "xfill-host";

    internal static string LibraryName =>
        OperatingSystem.IsWindows() ? "xfill.dll" : OperatingSystem.IsMacOS() ? "libxfill.dylib" : "libxfill.so";

    /// <summary>The host executable and the library it loads.</summary>
    /// <exception cref="XfillException">One of them is missing.</exception>
    public static (string Host, string Library) Find(XfillEngineOptions options)
    {
        string host = Executable(Locate(options.HostPath ?? Environment.GetEnvironmentVariable(HostVariable), HostName, "engine host"));
        string? beside = Path.GetDirectoryName(host) is { } dir ? Path.Combine(dir, LibraryName) : null;
        string library = options.LibraryPath is null && beside is not null && File.Exists(beside)
            ? beside
            : Locate(options.LibraryPath, LibraryName, "engine library");
        return (host, library);
    }

    private static string Locate(string? chosen, string name, string what)
    {
        if (chosen is not null)
        {
            return File.Exists(chosen)
                ? Path.GetFullPath(chosen)
                : throw new XfillException(XfillCodes.ErrArg, $"{what} not found: {chosen}");
        }
        var candidates = Candidates(name).ToList();
        return candidates.FirstOrDefault(File.Exists)
            ?? throw new XfillException(XfillCodes.ErrArg, $"{what} {name} not found in: {string.Join(", ", candidates)}");
    }

    private static IEnumerable<string> Candidates(string name)
    {
        string root = AppContext.BaseDirectory;
        yield return Path.Combine(root, name);
        foreach (string rid in new[] { RuntimeInformation.RuntimeIdentifier, PortableRid() }.Distinct())
        {
            yield return Path.Combine(root, "runtimes", rid, "native", name);
        }
    }

    private static string PortableRid()
    {
        string os = OperatingSystem.IsWindows() ? "win" : OperatingSystem.IsMacOS() ? "osx" : "linux";
        string arch = RuntimeInformation.ProcessArchitecture == Architecture.Arm64 ? "arm64" : "x64";
        return $"{os}-{arch}";
    }

    /// <summary>
    /// Archives (NuGet) don't keep the Unix execute bit: set it, or run a private copy when the
    /// package folder is read-only.
    /// </summary>
    private static string Executable(string path)
    {
        if (OperatingSystem.IsWindows())
        {
            return path;
        }
        const UnixFileMode exec = UnixFileMode.UserExecute | UnixFileMode.GroupExecute | UnixFileMode.OtherExecute;
        var mode = File.GetUnixFileMode(path);
        if ((mode & UnixFileMode.UserExecute) != 0)
        {
            return path;
        }
        try
        {
            File.SetUnixFileMode(path, mode | exec);
            return path;
        }
        catch (Exception e) when (e is UnauthorizedAccessException or IOException)
        {
            string copy = Path.Combine(Path.GetTempPath(), $"xfill-host-{Environment.ProcessId}");
            File.Copy(path, copy, overwrite: true);
            File.SetUnixFileMode(copy, UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
            return copy;
        }
    }
}
