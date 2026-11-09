namespace Cruciverba.Xfill;

/// <summary>Where to find the native engine.</summary>
public sealed record XfillEngineOptions
{
    /// <summary>
    /// Path of <c>xfill-host</c>, the process that runs the engine library. Default: the
    /// <c>XFILL_HOST_PATH</c> environment variable, else the one shipped with the package
    /// (<c>runtimes/&lt;rid&gt;/native/</c>).
    /// </summary>
    public string? HostPath { get; init; }

    /// <summary>
    /// Path of the engine library the host loads. Default: next to the host, else the one
    /// shipped with the package.
    /// </summary>
    public string? LibraryPath { get; init; }

    /// <summary>
    /// Generations running at once in the engine process; more wait their turn. Default: one per
    /// CPU. Each running generation uses about 1 MB.
    /// </summary>
    public int MaxConcurrentGenerations { get; init; } = Environment.ProcessorCount;

    /// <summary>Extra environment of the engine process (tests: fault injection).</summary>
    internal IReadOnlyDictionary<string, string>? HostEnvironment { get; init; }

    internal static readonly XfillEngineOptions Default = new();
}
