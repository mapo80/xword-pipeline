using System.Buffers.Binary;
using System.Collections.Concurrent;
using System.ComponentModel;
using System.Diagnostics;
using System.Text;

namespace Cruciverba.Xfill;

/// <summary>A reply of the host: return code, main text (JSON) and error message.</summary>
internal readonly record struct HostReply(int Rc, string? Text, string? Error);

/// <summary>
/// One <c>xfill-host</c> child process (protocol in <c>fill-engine/host/xfill-host.c</c>). When it
/// ends — a native fault, a kill — every call in progress fails with
/// <see cref="XfillEngineCrashedException"/>.
/// </summary>
internal sealed class HostProcess : IDisposable
{
    internal const byte Hello = 0;
    internal const byte Open = 1;
    internal const byte Generate = 2;
    internal const byte Cancel = 3;
    internal const byte Validate = 4;
    internal const byte Build = 5;
    internal const byte Close = 6;
    internal const byte Fault = 0x7F;

    /// <summary>Environment variable that lets the host provoke faults (tests).</summary>
    internal const string FaultVariable = "XFILL_HOST_FAULT_INJECTION";

    private const int StderrKept = 4096;

    private readonly Process _process;
    private readonly Stream _input;
    private readonly Stream _output;
    private readonly Lock _writeLock = new();
    private readonly ConcurrentDictionary<uint, TaskCompletionSource<HostReply>> _pending = new();
    private readonly StringBuilder _stderr = new();
    private readonly Task _stderrPump;
    private uint _nextId;
    private volatile XfillEngineCrashedException? _failure;
    private int _disposed;

    private HostProcess(Process process)
    {
        _process = process;
        _input = process.StandardInput.BaseStream;
        _output = process.StandardOutput.BaseStream;
        _stderrPump = Task.Run(PumpStderrAsync);
    }

    /// <summary>Engine version sent by the host when it starts.</summary>
    public string Version { get; private set; } = "";

    public int ProcessId => _process.Id;

    /// <summary>False once the process has ended (or its pipes broke) or was disposed.</summary>
    public bool IsAlive => _failure is null && Volatile.Read(ref _disposed) == 0;

    /// <summary>Memory of the child process (private bytes where reported, else working set).</summary>
    public long MemoryBytes
    {
        get
        {
            _process.Refresh();
            long privateBytes = _process.PrivateMemorySize64;
            return privateBytes > 0 ? privateBytes : _process.WorkingSet64;
        }
    }

    /// <summary>Start the host on <paramref name="library"/> and wait for its hello.</summary>
    /// <exception cref="XfillException">The host can't be started.</exception>
    /// <exception cref="XfillEngineCrashedException">It ended before answering.</exception>
    public static HostProcess Start(string host, string library, IReadOnlyDictionary<string, string>? environment)
    {
        var start = new ProcessStartInfo(host)
        {
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        start.ArgumentList.Add(library);
        start.Environment.Remove(FaultVariable);
        foreach (var (key, value) in environment ?? new Dictionary<string, string>())
        {
            start.Environment[key] = value;
        }
        Process process;
        try
        {
            process = Process.Start(start)!;
        }
        catch (Win32Exception e)
        {
            throw new XfillException(XfillCodes.ErrArg, $"cannot start the engine host {host}: {e.Message}");
        }
        var child = new HostProcess(process);
        var hello = child.ReadFrame();
        if (hello is null)
        {
            var crash = child.Crashed();
            child.Dispose();
            throw crash;
        }
        child.Version = hello.Value.Reply.Text ?? "";
        _ = Task.Run(child.PumpResponses);
        return child;
    }

    /// <summary>Send a request; the task completes with the host's reply.</summary>
    public (uint Id, Task<HostReply> Reply) Begin(byte kind, ReadOnlySpan<byte> payload)
    {
        uint id = Interlocked.Increment(ref _nextId);
        var reply = new TaskCompletionSource<HostReply>(TaskCreationOptions.RunContinuationsAsynchronously);
        _pending[id] = reply;
        if (_failure is { } failure)
        {
            Fail(id, failure);
            return (id, reply.Task);
        }
        try
        {
            Write(kind, id, payload);
        }
        catch (Exception e) when (e is IOException or ObjectDisposedException)
        {
            // the pipe broke: the response pump reports the end of the process
            if (_failure is { } ended)
            {
                Fail(id, ended);
            }
        }
        return (id, reply.Task);
    }

    /// <summary>Send a request and wait for the reply.</summary>
    public HostReply Call(byte kind, ReadOnlySpan<byte> payload) => Begin(kind, payload).Reply.GetAwaiter().GetResult();

    /// <summary>Ask the host to stop generation <paramref name="id"/> (best effort).</summary>
    public void SendCancel(uint id)
    {
        try
        {
            Write(Cancel, id, []);
        }
        catch (Exception e) when (e is IOException or ObjectDisposedException)
        {
            // the host is gone: the call fails on its own
        }
    }

    /// <summary>Close the pipes (the host exits) and end the process.</summary>
    public void Dispose()
    {
        if (Interlocked.Exchange(ref _disposed, 1) != 0)
        {
            return;
        }
        try
        {
            _input.Dispose();
        }
        catch (IOException)
        {
            // already broken
        }
        if (!_process.WaitForExit(2000))
        {
            _process.Kill(entireProcessTree: true);
            _process.WaitForExit(2000);
        }
        _process.Dispose();
    }

    private void Write(byte kind, uint id, ReadOnlySpan<byte> payload)
    {
        Span<byte> head = stackalloc byte[9];
        head[0] = kind;
        BinaryPrimitives.WriteUInt32LittleEndian(head[1..], id);
        BinaryPrimitives.WriteUInt32LittleEndian(head[5..], (uint)payload.Length);
        lock (_writeLock)
        {
            _input.Write(head);
            _input.Write(payload);
            _input.Flush();
        }
    }

    /// <summary>Read one response frame (id, reply); null at the end of the stream.</summary>
    private (uint Id, HostReply Reply)? ReadFrame()
    {
        Span<byte> head = stackalloc byte[13];
        if (!ReadExactly(head))
        {
            return null;
        }
        uint id = BinaryPrimitives.ReadUInt32LittleEndian(head[1..]);
        int rc = BinaryPrimitives.ReadInt32LittleEndian(head[5..]);
        string? text = ReadText(BinaryPrimitives.ReadUInt32LittleEndian(head[9..]), out bool ok);
        Span<byte> len = stackalloc byte[4];
        if (!ok || !ReadExactly(len))
        {
            return null;
        }
        string? error = ReadText(BinaryPrimitives.ReadUInt32LittleEndian(len), out ok);
        return ok ? (id, new HostReply(rc, text, error)) : null;
    }

    private string? ReadText(uint length, out bool ok)
    {
        if (length == 0)
        {
            ok = true;
            return null;
        }
        byte[] bytes = new byte[length];
        ok = ReadExactly(bytes);
        return ok ? Encoding.UTF8.GetString(bytes) : null;
    }

    private bool ReadExactly(Span<byte> buffer)
    {
        try
        {
            _output.ReadExactly(buffer);
            return true;
        }
        catch (Exception e) when (e is EndOfStreamException or IOException or ObjectDisposedException)
        {
            return false;
        }
    }

    private void PumpResponses()
    {
        while (ReadFrame() is { } frame)
        {
            if (_pending.TryRemove(frame.Id, out var call))
            {
                call.TrySetResult(frame.Reply);
            }
        }
        var failure = Crashed();
        _failure = failure;
        foreach (uint id in _pending.Keys)
        {
            Fail(id, failure);
        }
    }

    private void Fail(uint id, Exception failure)
    {
        if (_pending.TryRemove(id, out var call))
        {
            call.TrySetException(failure);
        }
    }

    /// <summary>The exception for a host whose output ended.</summary>
    private XfillEngineCrashedException Crashed()
    {
        int? exitCode = null;
        try
        {
            if (_process.WaitForExit(2000))
            {
                exitCode = _process.ExitCode;
                _stderrPump.Wait(1000);
            }
        }
        catch (InvalidOperationException)
        {
            // disposed meanwhile
        }
        string diagnostics;
        lock (_stderr)
        {
            diagnostics = _stderr.ToString().Trim();
        }
        return new XfillEngineCrashedException(exitCode, diagnostics);
    }

    private async Task PumpStderrAsync()
    {
        try
        {
            var reader = _process.StandardError;
            char[] buffer = new char[1024];
            int n;
            while ((n = await reader.ReadAsync(buffer).ConfigureAwait(false)) > 0)
            {
                lock (_stderr)
                {
                    _stderr.Append(buffer, 0, n);
                    if (_stderr.Length > StderrKept)
                    {
                        _stderr.Remove(0, _stderr.Length - StderrKept);
                    }
                }
            }
        }
        catch (Exception e) when (e is IOException or ObjectDisposedException or InvalidOperationException)
        {
            // the process is gone
        }
    }
}
