using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;

namespace Cruciverba.Xfill;

/// <summary>Error reported by the native engine.</summary>
public sealed class XfillException : Exception
{
    /// <summary>Create an exception with the engine's return code.</summary>
    public XfillException(int code, string message)
        : base(message)
    {
        Code = code;
    }

    /// <summary>Return code of the C interface (negative).</summary>
    public int Code { get; }
}

/// <summary>
/// The crossword engine with a loaded dictionary. The dictionary is immutable: one engine
/// serves concurrent <see cref="GenerateAsync"/> calls. Dispose it to free the native memory
/// (calls in progress keep it alive until they return).
/// </summary>
public sealed class XfillEngine : IDisposable
{
    private readonly WordlistHandle _handle;

    private XfillEngine(WordlistHandle handle, long words, long bytes, long measuredBytes)
    {
        _handle = handle;
        WordCount = words;
        DictionaryBytes = bytes;
        MeasuredLoadBytes = measuredBytes;
        XfillTelemetry.AddDictionary(this);
    }

    /// <summary>Version of the native engine.</summary>
    public static string Version => Marshal.PtrToStringUTF8(NativeMethods.Version()) ?? "";

    /// <summary>Words in the dictionary.</summary>
    public long WordCount { get; }

    /// <summary>Heap memory of the dictionary as computed by the engine (bytes).</summary>
    public long DictionaryBytes { get; }

    /// <summary>Growth of the process private memory while loading (bytes, may be 0).</summary>
    public long MeasuredLoadBytes { get; }

    internal bool IsDisposed => _handle.IsClosed;

    /// <summary>Load a <c>WORD,clue</c> CSV (UTF-8 or Latin-1).</summary>
    /// <exception cref="XfillException">No usable word in the CSV.</exception>
    public static unsafe XfillEngine LoadWordlist(ReadOnlySpan<byte> csv)
    {
        using var activity = XfillTelemetry.Source.StartActivity("xfill.load_wordlist");
        long before = PrivateBytes();
        int rc;
        WordlistHandle handle;
        nint err;
        fixed (byte* p = csv)
        {
            rc = NativeMethods.WordlistLoadCsv(p, (nuint)csv.Length, out handle, out err);
        }
        string? message = NativeMethods.TakeString(err);
        if (rc != NativeMethods.Ok)
        {
            handle.Dispose();
            activity?.SetStatus(ActivityStatusCode.Error, message);
            throw new XfillException(rc, message ?? $"xfill_wordlist_load_csv failed ({rc})");
        }
        long measured = Math.Max(0, PrivateBytes() - before);
        var stats = ReadStats(handle);
        activity?.SetTag("xfill.dictionary.words", stats.Words);
        activity?.SetTag("xfill.dictionary.bytes", stats.Bytes);
        return new XfillEngine(handle, stats.Words, stats.Bytes, measured);
    }

    /// <summary>Load a <c>WORD,clue</c> CSV from a stream.</summary>
    public static XfillEngine LoadWordlist(Stream csv)
    {
        ArgumentNullException.ThrowIfNull(csv);
        using var buffer = new MemoryStream();
        csv.CopyTo(buffer);
        return LoadWordlist(buffer.GetBuffer().AsSpan(0, (int)buffer.Length));
    }

    /// <summary>Load a <c>WORD,clue</c> CSV from a stream, reading it asynchronously.</summary>
    public static async Task<XfillEngine> LoadWordlistAsync(Stream csv, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(csv);
        using var buffer = new MemoryStream();
        await csv.CopyToAsync(buffer, cancellationToken).ConfigureAwait(false);
        return LoadWordlist(buffer.GetBuffer().AsSpan(0, (int)buffer.Length));
    }

    /// <summary>
    /// Generate a crossword on a worker thread. Cancelling the token stops the engine within
    /// milliseconds and throws <see cref="OperationCanceledException"/>. A request the engine
    /// can't satisfy returns a result with status <see cref="GenerateStatus.NoGrid"/>.
    /// </summary>
    /// <exception cref="XfillException">Invalid request or engine error.</exception>
    public Task<GenerateResult> GenerateAsync(GenerateRequest request, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(request);
        ObjectDisposedException.ThrowIf(_handle.IsClosed, this);
        cancellationToken.ThrowIfCancellationRequested();
        return Task.Factory.StartNew(
            () => Generate(request, cancellationToken),
            CancellationToken.None,
            TaskCreationOptions.LongRunning,
            TaskScheduler.Default);
    }

    private unsafe GenerateResult Generate(GenerateRequest request, CancellationToken cancellationToken)
    {
        using var activity = XfillTelemetry.Source.StartActivity("xfill.generate");
        activity?.SetTag("xfill.rows", request.Rows);
        activity?.SetTag("xfill.cols", request.Cols);
        // product formats are written COLUMNS × ROWS
        activity?.SetTag("xfill.format", $"{request.Cols}x{request.Rows}");
        var started = Stopwatch.GetTimestamp();
        byte[] json = Utf8Z(JsonSerializer.Serialize(request, XfillJsonContext.Default.GenerateRequest));
        int* flag = (int*)NativeMemory.AllocZeroed(sizeof(int));
        int rc;
        nint result;
        nint err;
        try
        {
            using (cancellationToken.Register(static state => Volatile.Write(ref *(int*)(nint)state!, 1), (nint)flag))
            {
                fixed (byte* p = json)
                {
                    rc = NativeMethods.Generate(_handle, p, flag, out result, out err);
                }
            }
        }
        finally
        {
            // the registration is disposed (and any running callback finished) before the free
            NativeMemory.Free(flag);
        }
        string? message = NativeMethods.TakeString(err);
        string? resultJson = NativeMethods.TakeString(result);
        double seconds = Stopwatch.GetElapsedTime(started).TotalSeconds;
        GenerateResult? parsed = resultJson is null
            ? null
            : JsonSerializer.Deserialize(resultJson, XfillJsonContext.Default.GenerateResult);
        string outcome = parsed?.Status.ToString() ?? "Error";
        XfillTelemetry.Record(activity, parsed, outcome, seconds);
        switch (rc)
        {
            case NativeMethods.Ok:
                return parsed!;
            case NativeMethods.Cancelled:
                throw new OperationCanceledException("Generation cancelled.", cancellationToken);
            default:
                activity?.SetStatus(ActivityStatusCode.Error, message);
                throw new XfillException(rc, message ?? parsed?.Error ?? $"xfill_generate failed ({rc})");
        }
    }

    /// <summary>
    /// Check a finished grid: numbering, coordinates, crossings, answers against the grid,
    /// repeated words, clues containing the answer.
    /// </summary>
    public static unsafe ValidationReport Validate(ValidateInput grid)
    {
        ArgumentNullException.ThrowIfNull(grid);
        byte[] json = Utf8Z(JsonSerializer.Serialize(grid, XfillJsonContext.Default.ValidateInput));
        int rc;
        nint report;
        fixed (byte* p = json)
        {
            rc = NativeMethods.Validate(p, out report);
        }
        string? text = NativeMethods.TakeString(report);
        if (rc != NativeMethods.Ok || text is null)
        {
            throw new XfillException(rc, text ?? $"xfill_validate failed ({rc})");
        }
        return JsonSerializer.Deserialize(text, XfillJsonContext.Default.ValidationReport)!;
    }

    /// <summary>Free the native dictionary.</summary>
    public void Dispose()
    {
        _handle.Dispose();
        XfillTelemetry.RemoveDictionary(this);
    }

    private static WordlistStats ReadStats(WordlistHandle handle)
    {
        int rc = NativeMethods.WordlistStats(handle, out nint json);
        string? text = NativeMethods.TakeString(json);
        if (rc != NativeMethods.Ok || text is null)
        {
            throw new XfillException(rc, $"xfill_wordlist_stats failed ({rc})");
        }
        return JsonSerializer.Deserialize(text, XfillJsonContext.Default.WordlistStats)!;
    }

    private static byte[] Utf8Z(string s)
    {
        byte[] bytes = new byte[Encoding.UTF8.GetByteCount(s) + 1];
        Encoding.UTF8.GetBytes(s, bytes);
        return bytes;
    }

    private static long PrivateBytes()
    {
        using var process = Process.GetCurrentProcess();
        return process.PrivateMemorySize64;
    }
}
