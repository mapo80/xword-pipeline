using System.Diagnostics;
using System.Text;
using System.Text.Json;

namespace Cruciverba.Xfill;

/// <summary>Error reported by the native engine.</summary>
public class XfillException : Exception
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
/// The engine process ended during the call (a native fault, or the process was killed). The
/// application is unaffected; the next call starts a new engine process.
/// </summary>
public sealed class XfillEngineCrashedException : XfillException
{
    /// <summary>The <see cref="XfillException.Code"/> of this exception.</summary>
    public const int EngineCrashed = XfillCodes.EngineCrashed;

    /// <summary>Create the exception from the process exit code and its last error output.</summary>
    public XfillEngineCrashedException(int? exitCode, string diagnostics)
        : base(EngineCrashed, Describe(exitCode, diagnostics))
    {
        ExitCode = exitCode;
        Diagnostics = diagnostics;
    }

    /// <summary>Exit code of the engine process, when it could be read.</summary>
    public int? ExitCode { get; }

    /// <summary>Last lines the engine process wrote to its error output.</summary>
    public string Diagnostics { get; }

    private static string Describe(int? exitCode, string diagnostics)
    {
        string how = exitCode is { } c ? $"exit code {c}" : "pipe closed";
        return diagnostics.Length > 0
            ? $"the native engine process ended ({how}): {diagnostics}"
            : $"the native engine process ended ({how})";
    }
}

/// <summary>
/// The crossword engine: a child process (<c>xfill-host</c>) running the engine library, so that
/// a native fault never ends this process. Dictionaries are built once from a CSV
/// (<see cref="BuildDictionaryAsync(string, string, CancellationToken)"/>, e.g. after a data
/// import) into a file that <see cref="OpenDictionaryAsync"/> maps in about a millisecond, with no
/// parsing and no copy. If the process ends, the calls in progress fail with
/// <see cref="XfillEngineCrashedException"/> and the next call starts a new process, reopening
/// the dictionaries in use. One engine serves concurrent calls; dispose it to end the process
/// (calls in progress finish first).
/// </summary>
public sealed class XfillEngine : IDisposable
{
    private readonly XfillEngineOptions _options;
    private readonly string _host;
    private readonly string _library;
    private readonly Lock _gate = new();
    private readonly Lock _childGate = new();
    private HostProcess? _child;
    private int _epoch;
    private int _restarts;
    private int _active;
    private bool _disposeRequested;
    private bool _closed;

    private XfillEngine(XfillEngineOptions options, string host, string library, HostProcess child)
    {
        _options = options;
        _host = host;
        _library = library;
        _child = child;
        Version = child.Version;
        Slots = new SemaphoreSlim(Math.Max(1, options.MaxConcurrentGenerations));
    }

    /// <summary>Version of the native engine.</summary>
    public string Version { get; }

    internal SemaphoreSlim Slots { get; }

    internal bool IsDisposed
    {
        get
        {
            lock (_gate)
            {
                return _disposeRequested;
            }
        }
    }

    /// <summary>Id of the engine process (null when none is running).</summary>
    internal int? HostProcessId
    {
        get
        {
            lock (_childGate)
            {
                return _child is { IsAlive: true } c ? c.ProcessId : null;
            }
        }
    }

    /// <summary>Engine processes started again after one ended.</summary>
    internal int HostRestarts => Volatile.Read(ref _restarts);

    /// <summary>Start the engine process.</summary>
    /// <exception cref="XfillException">The host or the engine library is missing or can't start.</exception>
    public static XfillEngine Start(XfillEngineOptions? options = null)
    {
        options ??= XfillEngineOptions.Default;
        var (host, library) = NativeLocator.Find(options);
        return new XfillEngine(options, host, library, HostProcess.Start(host, library, options.HostEnvironment));
    }

    /// <summary>
    /// Build the dictionary file <paramref name="dictionaryPath"/> from a <c>WORD,clue</c> CSV
    /// file (UTF-8 or Latin-1). The file is written atomically: a reader never sees half of it.
    /// </summary>
    /// <exception cref="XfillException">No usable word (code −6), or a file can't be read or written (−7).</exception>
    public async Task<DictionaryStats> BuildDictionaryAsync(string csvPath, string dictionaryPath, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(csvPath);
        ArgumentNullException.ThrowIfNull(dictionaryPath);
        cancellationToken.ThrowIfCancellationRequested();
        using var activity = XfillTelemetry.Source.StartActivity("xfill.build_dictionary");
        string payload = $"{Path.GetFullPath(csvPath)}\0{Path.GetFullPath(dictionaryPath)}";
        var reply = await CallAsync(HostProcess.Build, Encoding.UTF8.GetBytes(payload)).ConfigureAwait(false);
        var stats = Stats(reply, activity);
        return new DictionaryStats { Words = stats.Words, Bytes = stats.Bytes };
    }

    /// <summary>Build the dictionary file <paramref name="dictionaryPath"/> from a CSV stream.</summary>
    public async Task<DictionaryStats> BuildDictionaryAsync(Stream csv, string dictionaryPath, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(csv);
        string temp = Path.Combine(Path.GetTempPath(), $"xfill-{Guid.NewGuid():N}.csv");
        try
        {
            var file = File.Create(temp);
            await using (file.ConfigureAwait(false))
            {
                await csv.CopyToAsync(file, cancellationToken).ConfigureAwait(false);
            }
            return await BuildDictionaryAsync(temp, dictionaryPath, cancellationToken).ConfigureAwait(false);
        }
        finally
        {
            File.Delete(temp);
        }
    }

    /// <summary>Open a dictionary file: mapped by the engine process, no parsing and no copy.</summary>
    /// <exception cref="XfillException">Missing (code −7), or corrupted / of another format version (−6): rebuild it.</exception>
    public async Task<XfillDictionary> OpenDictionaryAsync(string dictionaryPath, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(dictionaryPath);
        cancellationToken.ThrowIfCancellationRequested();
        using var activity = XfillTelemetry.Source.StartActivity("xfill.open_dictionary");
        string path = Path.GetFullPath(dictionaryPath);
        Enter();
        try
        {
            var (child, epoch) = Current();
            var reply = await child.Begin(HostProcess.Open, Encoding.UTF8.GetBytes(path)).Reply.ConfigureAwait(false);
            return new XfillDictionary(this, path, Stats(reply, activity), epoch);
        }
        finally
        {
            Exit();
        }
    }

    /// <summary>
    /// Check a finished grid: numbering, coordinates, crossings, answers against the grid,
    /// repeated words, clues containing the answer.
    /// </summary>
    public async Task<ValidationReport> ValidateAsync(ValidateInput grid)
    {
        ArgumentNullException.ThrowIfNull(grid);
        byte[] json = JsonSerializer.SerializeToUtf8Bytes(grid, XfillJsonContext.Default.ValidateInput);
        var reply = await CallAsync(HostProcess.Validate, json).ConfigureAwait(false);
        if (reply.Rc != XfillCodes.Ok || reply.Text is null)
        {
            throw new XfillException(reply.Rc, reply.Text ?? reply.Error ?? $"xfill_validate failed ({reply.Rc})");
        }
        return JsonSerializer.Deserialize(reply.Text, XfillJsonContext.Default.ValidationReport)!;
    }

    /// <summary>End the engine process; calls in progress finish first.</summary>
    public void Dispose()
    {
        bool close;
        lock (_gate)
        {
            if (_disposeRequested)
            {
                return;
            }
            _disposeRequested = true;
            close = _active == 0 && !_closed;
            _closed |= close;
        }
        if (close)
        {
            Close();
        }
    }

    /// <summary>Provoke a native fault in the engine process (tests; needs fault injection).</summary>
    internal async Task InjectFaultAsync(string kind)
    {
        var reply = await CallAsync(HostProcess.Fault, Encoding.UTF8.GetBytes(kind)).ConfigureAwait(false);
        throw new XfillException(reply.Rc, reply.Error ?? "fault not raised");
    }

    /// <summary>The running engine process (started again if it ended) and its generation number.</summary>
    internal (HostProcess Child, int Epoch) Current()
    {
        lock (_childGate)
        {
            if (_child is { IsAlive: true } alive)
            {
                return (alive, _epoch);
            }
            _child?.Dispose();
            _child = null;
            _child = HostProcess.Start(_host, _library, _options.HostEnvironment);
            _epoch++;
            _restarts++;
            return (_child, _epoch);
        }
    }

    internal void Enter()
    {
        lock (_gate)
        {
            ObjectDisposedException.ThrowIf(_disposeRequested, this);
            _active++;
        }
    }

    internal void Exit()
    {
        bool close;
        lock (_gate)
        {
            _active--;
            close = _disposeRequested && _active == 0 && !_closed;
            _closed |= close;
        }
        if (close)
        {
            Close();
        }
    }

    /// <summary>Send a request to the engine process and wait for its reply.</summary>
    private async Task<HostReply> CallAsync(byte kind, byte[] payload)
    {
        Enter();
        try
        {
            return await Current().Child.Begin(kind, payload).Reply.ConfigureAwait(false);
        }
        finally
        {
            Exit();
        }
    }

    internal static HostStats Stats(HostReply reply, Activity? activity)
    {
        if (reply.Rc != XfillCodes.Ok || reply.Text is null)
        {
            activity?.SetStatus(ActivityStatusCode.Error, reply.Error);
            throw new XfillException(reply.Rc, reply.Error ?? $"engine error ({reply.Rc})");
        }
        var stats = JsonSerializer.Deserialize(reply.Text, XfillJsonContext.Default.HostStats)!;
        activity?.SetTag("xfill.dictionary.words", stats.Words);
        activity?.SetTag("xfill.dictionary.bytes", stats.Bytes);
        return stats;
    }

    private void Close()
    {
        lock (_childGate)
        {
            _child?.Dispose();
            _child = null;
        }
        Slots.Dispose();
    }
}

/// <summary>
/// An open dictionary of an <see cref="XfillEngine"/>: immutable, serves concurrent
/// generations. Dispose it to close it (generations in progress finish first).
/// </summary>
[System.Diagnostics.CodeAnalysis.SuppressMessage(
    "Naming", "CA1711", Justification = "A crossword dictionary (words and clues), not a collection type.")]
public sealed class XfillDictionary : IDisposable
{
    private readonly XfillEngine _engine;
    private readonly Lock _gate = new();
    private uint _dict;
    private int _epoch;
    private bool _disposed;

    internal XfillDictionary(XfillEngine engine, string path, HostStats stats, int epoch)
    {
        _engine = engine;
        Path = path;
        WordCount = stats.Words;
        DictionaryBytes = stats.Bytes;
        MappedBytes = stats.Mapped;
        _dict = stats.Dict;
        _epoch = epoch;
        XfillTelemetry.AddDictionary(this);
    }

    /// <summary>The dictionary file.</summary>
    public string Path { get; }

    /// <summary>Words in the dictionary.</summary>
    public long WordCount { get; }

    /// <summary>Size of the dictionary (bytes).</summary>
    public long DictionaryBytes { get; }

    /// <summary>Part of it read in place from the file: shared, reclaimable pages (bytes).</summary>
    public long MappedBytes { get; }

    /// <summary>
    /// Generate a crossword. Cancelling the token stops the engine within milliseconds and throws
    /// <see cref="OperationCanceledException"/>. A request the engine can't satisfy returns a
    /// result with status <see cref="GenerateStatus.NoGrid"/>.
    /// </summary>
    /// <exception cref="XfillException">Invalid request or engine error.</exception>
    /// <exception cref="XfillEngineCrashedException">The engine process ended during the call.</exception>
    public Task<GenerateResult> GenerateAsync(GenerateRequest request, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(request);
        lock (_gate)
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
        }
        cancellationToken.ThrowIfCancellationRequested();
        _engine.Enter();
        return GenerateCoreAsync(request, cancellationToken);
    }

    /// <summary>Close the dictionary in the engine process (generations in progress finish first).</summary>
    public void Dispose()
    {
        uint dict;
        int epoch;
        lock (_gate)
        {
            if (_disposed)
            {
                return;
            }
            _disposed = true;
            (dict, epoch) = (_dict, _epoch);
        }
        XfillTelemetry.RemoveDictionary(this);
        try
        {
            _engine.Enter();
        }
        catch (ObjectDisposedException)
        {
            return; // the engine process is gone, and the dictionary with it
        }
        try
        {
            var (child, current) = _engine.Current();
            if (current == epoch)
            {
                Span<byte> id = stackalloc byte[4];
                System.Buffers.Binary.BinaryPrimitives.WriteUInt32LittleEndian(id, dict);
                _ = child.Begin(HostProcess.Close, id).Reply;
            }
        }
        catch (XfillException)
        {
            // a new engine process could not start: nothing to close
        }
        finally
        {
            _engine.Exit();
        }
    }

    private async Task<GenerateResult> GenerateCoreAsync(GenerateRequest request, CancellationToken cancellationToken)
    {
        try
        {
            await _engine.Slots.WaitAsync(cancellationToken).ConfigureAwait(false);
            try
            {
                return await RunAsync(request, cancellationToken).ConfigureAwait(false);
            }
            finally
            {
                _engine.Slots.Release();
            }
        }
        finally
        {
            _engine.Exit();
        }
    }

    private async Task<GenerateResult> RunAsync(GenerateRequest request, CancellationToken cancellationToken)
    {
        using var activity = XfillTelemetry.Source.StartActivity("xfill.generate");
        activity?.SetTag("xfill.rows", request.Rows);
        activity?.SetTag("xfill.cols", request.Cols);
        // product formats are written COLUMNS × ROWS
        activity?.SetTag("xfill.format", $"{request.Cols}x{request.Rows}");
        var started = Stopwatch.GetTimestamp();
        byte[] json = JsonSerializer.SerializeToUtf8Bytes(request, XfillJsonContext.Default.GenerateRequest);
        HostReply reply;
        try
        {
            var (child, dict) = Bind();
            byte[] payload = new byte[4 + json.Length];
            System.Buffers.Binary.BinaryPrimitives.WriteUInt32LittleEndian(payload, dict);
            json.CopyTo(payload, 4);
            var (id, pending) = child.Begin(HostProcess.Generate, payload);
            using (cancellationToken.Register(static state =>
            {
                var (c, i) = ((HostProcess, uint))state!;
                c.SendCancel(i);
            }, (child, id)))
            {
                reply = await pending.ConfigureAwait(false);
            }
        }
        catch (XfillEngineCrashedException e)
        {
            XfillTelemetry.Record(activity, null, "Crashed", Stopwatch.GetElapsedTime(started).TotalSeconds);
            activity?.SetStatus(ActivityStatusCode.Error, e.Message);
            throw;
        }
        double seconds = Stopwatch.GetElapsedTime(started).TotalSeconds;
        GenerateResult? parsed = reply.Text is null
            ? null
            : JsonSerializer.Deserialize(reply.Text, XfillJsonContext.Default.GenerateResult);
        string outcome = parsed?.Status.ToString() ?? "Error";
        XfillTelemetry.Record(activity, parsed, outcome, seconds);
        switch (reply.Rc)
        {
            case XfillCodes.Ok:
                return parsed!;
            case XfillCodes.Cancelled:
                throw new OperationCanceledException("Generation cancelled.", cancellationToken);
            default:
                activity?.SetStatus(ActivityStatusCode.Error, reply.Error);
                throw new XfillException(reply.Rc, reply.Error ?? parsed?.Error ?? $"xfill_generate failed ({reply.Rc})");
        }
    }

    /// <summary>
    /// The engine process and this dictionary's id in it: reopened (about a millisecond) when the
    /// process was started again after it ended.
    /// </summary>
    private (HostProcess Child, uint Dict) Bind()
    {
        lock (_gate)
        {
            var (child, epoch) = _engine.Current();
            if (epoch == _epoch)
            {
                return (child, _dict);
            }
            var stats = XfillEngine.Stats(child.Call(HostProcess.Open, Encoding.UTF8.GetBytes(Path)), null);
            (_dict, _epoch) = (stats.Dict, epoch);
            return (child, _dict);
        }
    }
}
