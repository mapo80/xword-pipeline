using System.Diagnostics;
using Shouldly;
using Xunit;

namespace Cruciverba.Xfill.Tests;

/// <summary>The engine process: start, packaging (execute bit) and calls on a process that ended.</summary>
public sealed class HostProcessTests(DictionaryFixture fixture) : IClassFixture<DictionaryFixture>
{
    private static (string Host, string Library) Native() => NativeLocator.Find(XfillEngineOptions.Default);

    [Fact]
    public void The_execute_bit_lost_by_the_package_is_restored()
    {
        if (OperatingSystem.IsWindows())
        {
            return; // no execute bit
        }
        var (host, library) = Native();
        string dir = Directory.CreateTempSubdirectory("xfill-pkg-").FullName;
        try
        {
            string copy = Path.Combine(dir, NativeLocator.HostName);
            File.Copy(host, copy);
            File.SetUnixFileMode(copy, UnixFileMode.UserRead | UnixFileMode.UserWrite);
            using var engine = XfillEngine.Start(new() { HostPath = copy, LibraryPath = library });
            engine.Version.ShouldBe(fixture.Engine.Version);
            (File.GetUnixFileMode(copy) & UnixFileMode.UserExecute).ShouldNotBe(UnixFileMode.None);
        }
        finally
        {
            Directory.Delete(dir, recursive: true);
        }
    }

    [Fact]
    public void A_host_that_cannot_run_is_reported()
    {
        string fake = Path.Combine(Path.GetTempPath(), $"xfill-fake-host-{Guid.NewGuid():N}");
        File.WriteAllBytes(fake, [0, 1, 2, 3]);
        try
        {
            if (!OperatingSystem.IsWindows())
            {
                File.SetUnixFileMode(fake, UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
            }
            var ex = Should.Throw<XfillException>(() => XfillEngine.Start(new() { HostPath = fake, LibraryPath = Native().Library }));
            ex.Message.ShouldContain("cannot start the engine host");
        }
        finally
        {
            File.Delete(fake);
        }
    }

    [Fact]
    public async Task Calls_on_an_ended_process_fail_at_once()
    {
        var (host, library) = Native();
        using var child = HostProcess.Start(host, library, null);
        child.Version.ShouldBe(fixture.Engine.Version);
        using (var process = Process.GetProcessById(child.ProcessId))
        {
            process.Kill();
            await process.WaitForExitAsync(TestContext.Current.CancellationToken);
        }
        // the response pump notices the end
        var first = child.Begin(HostProcess.Validate, "{}"u8).Reply;
        await Should.ThrowAsync<XfillEngineCrashedException>(() => first);
        child.IsAlive.ShouldBeFalse();
        var later = child.Begin(HostProcess.Validate, "{}"u8).Reply;
        later.IsFaulted.ShouldBeTrue();
        await Should.ThrowAsync<XfillEngineCrashedException>(() => later);
        child.SendCancel(1); // no effect, no error
        child.Dispose();
        child.Dispose();
    }

    [Fact]
    public void Unknown_requests_are_answered_with_an_error()
    {
        var (host, library) = Native();
        using var child = HostProcess.Start(host, library, null);
        var reply = child.Call(42, []);
        reply.Rc.ShouldBe(-1);
        reply.Error.ShouldBe("unknown request");
        // a generation on a dictionary that isn't open; closing one
        child.Call(HostProcess.Generate, [9, 0, 0, 0, (byte)'{', (byte)'}']).Error.ShouldBe("no such dictionary");
        child.Call(HostProcess.Close, [9, 0, 0, 0]).Error.ShouldBe("no such dictionary");
        child.Call(HostProcess.Build, "only-one-path"u8).Rc.ShouldBe(-1);
        child.MemoryBytes.ShouldBeGreaterThan(0);
    }

    [Fact]
    public void The_crash_message_names_the_exit_code_and_the_output()
    {
        new XfillEngineCrashedException(134, "memory allocation failed").Message
            .ShouldBe("the native engine process ended (exit code 134): memory allocation failed");
        new XfillEngineCrashedException(null, "").Message.ShouldBe("the native engine process ended (pipe closed)");
    }
}
