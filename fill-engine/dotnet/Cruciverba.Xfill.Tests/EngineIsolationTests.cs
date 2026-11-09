using System.Diagnostics;
using Shouldly;
using Xunit;

namespace Cruciverba.Xfill.Tests;

/// <summary>
/// The engine library runs in a child process (xfill-host): a native fault there (abort, invalid
/// memory access, stack overflow) ends that process, never this one, and the next call starts a
/// new engine process, reopening the dictionaries in use (a mapped file: about a millisecond).
/// </summary>
public sealed class EngineIsolationTests(DictionaryFixture fixture) : IClassFixture<DictionaryFixture>
{
    private static readonly XfillEngineOptions Faults = new()
    {
        HostEnvironment = new Dictionary<string, string> { ["XFILL_HOST_FAULT_INJECTION"] = "1" },
    };

    private static GenerateRequest Small => new()
    {
        Rows = 5,
        Cols = 5,
        Blocks = 2,
        Seed = 3,
        Threads = 1,
        Candidates = 12,
        Pool = 3,
        NodeBudget = 4000,
    };

    [Theory]
    [InlineData("abort")]
    [InlineData("segv")]
    [InlineData("overflow")]
    public async Task A_native_crash_does_not_take_this_process_down(string fault)
    {
        var ct = TestContext.Current.CancellationToken;
        using var engine = XfillEngine.Start(Faults);
        using var dict = await engine.OpenDictionaryAsync(fixture.DictionaryPath, ct);
        var before = await dict.GenerateAsync(Small, ct);
        int firstPid = engine.HostProcessId!.Value;

        var crash = await Should.ThrowAsync<XfillEngineCrashedException>(() => engine.InjectFaultAsync(fault));
        crash.Code.ShouldBe(XfillEngineCrashedException.EngineCrashed);
        crash.ExitCode.ShouldNotBe(0);

        // still alive here; the next call starts a new engine process and reopens the dictionary
        var watch = Stopwatch.StartNew();
        var after = await dict.GenerateAsync(Small, ct);
        after.Grid.ShouldBe(before.Grid);
        engine.HostRestarts.ShouldBe(1);
        engine.HostProcessId.ShouldNotBe(firstPid);
        watch.Elapsed.ShouldBeLessThan(TimeSpan.FromSeconds(5));
    }

    [Fact]
    public async Task Fault_injection_is_off_by_default()
    {
        using var engine = XfillEngine.Start();
        var ex = await Should.ThrowAsync<XfillException>(() => engine.InjectFaultAsync("abort"));
        ex.ShouldNotBeOfType<XfillEngineCrashedException>();
        ex.Message.ShouldContain("off");
        engine.HostRestarts.ShouldBe(0);
    }

    [Fact]
    public async Task A_killed_engine_fails_the_calls_in_progress_and_comes_back()
    {
        var ct = TestContext.Current.CancellationToken;
        using var engine = XfillEngine.Start();
        using var dict = await engine.OpenDictionaryAsync(fixture.DictionaryPath, ct);
        var slow = Small with { Rows = 7, Cols = 7, Blocks = 12, NodeBudget = 0, TimePerGridS = 30, Candidates = 40, Pool = 40, Threads = 2 };
        var running = dict.GenerateAsync(slow, ct);
        await Task.Delay(200, ct);
        using (var host = Process.GetProcessById(engine.HostProcessId!.Value))
        {
            host.Kill();
        }
        var crash = await Should.ThrowAsync<XfillEngineCrashedException>(() => running);
        crash.Message.ShouldContain("engine process ended");
        (await dict.GenerateAsync(Small, ct)).Status.ShouldBe(GenerateStatus.Ok);
        engine.HostRestarts.ShouldBe(1);
        // a dictionary opened before the restart and closed after: nothing breaks
        var other = await engine.OpenDictionaryAsync(fixture.DictionaryPath, ct);
        other.Dispose();
    }

    [Fact]
    public async Task The_engine_process_ends_with_the_engine()
    {
        var engine = XfillEngine.Start();
        using var host = Process.GetProcessById(engine.HostProcessId!.Value);
        engine.Dispose();
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(5));
        await host.WaitForExitAsync(timeout.Token);
        host.HasExited.ShouldBeTrue();
        engine.HostProcessId.ShouldBeNull();
    }

    [Fact]
    public async Task A_dispose_waits_for_the_calls_in_progress()
    {
        var ct = TestContext.Current.CancellationToken;
        var engine = XfillEngine.Start();
        var dict = await engine.OpenDictionaryAsync(fixture.DictionaryPath, ct);
        var running = dict.GenerateAsync(Small, ct);
        dict.Dispose();
        engine.Dispose();
        (await running).Status.ShouldBe(GenerateStatus.Ok);
        engine.HostProcessId.ShouldBeNull();
    }

    [Fact]
    public async Task Generations_beyond_the_limit_wait_their_turn()
    {
        var ct = TestContext.Current.CancellationToken;
        using var engine = XfillEngine.Start(new XfillEngineOptions { MaxConcurrentGenerations = 1 });
        using var dict = await engine.OpenDictionaryAsync(fixture.DictionaryPath, ct);
        var slow = Small with { Rows = 7, Cols = 7, Blocks = 12, NodeBudget = 0, TimePerGridS = 30, Candidates = 40, Pool = 40, Threads = 1 };
        using var cts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        var first = dict.GenerateAsync(slow, cts.Token);
        var second = dict.GenerateAsync(Small, ct);
        await Task.Delay(300, ct);
        second.IsCompleted.ShouldBeFalse(); // waiting for the slot
        await cts.CancelAsync();
        await Should.ThrowAsync<OperationCanceledException>(() => first);
        (await second).Status.ShouldBe(GenerateStatus.Ok);
        // a waiting call can be cancelled too
        var third = dict.GenerateAsync(slow, ct);
        using var quick = new CancellationTokenSource();
        var waiting = dict.GenerateAsync(Small, quick.Token);
        await quick.CancelAsync();
        await Should.ThrowAsync<OperationCanceledException>(() => waiting);
        _ = third.ContinueWith(static _ => { }, TaskScheduler.Default);
    }

    [Fact]
    public void A_missing_host_or_library_is_reported()
    {
        string missing = Path.Combine(Path.GetTempPath(), "no-such-xfill-file");
        Should.Throw<XfillException>(() => XfillEngine.Start(new() { HostPath = missing }))
            .Message.ShouldContain("no-such-xfill-file");
        Should.Throw<XfillException>(() => XfillEngine.Start(new() { LibraryPath = missing }))
            .Message.ShouldContain("no-such-xfill-file");
        // a file that isn't the engine library: the host can't load it and says why
        string notALibrary = Path.Combine(Path.GetTempPath(), $"xfill-test-{Guid.NewGuid():N}.txt");
        File.WriteAllText(notALibrary, "not a library");
        try
        {
            var crash = Should.Throw<XfillEngineCrashedException>(() => XfillEngine.Start(new() { LibraryPath = notALibrary }));
            crash.Diagnostics.ShouldContain("cannot load the engine library");
        }
        finally
        {
            File.Delete(notALibrary);
        }
    }
}
