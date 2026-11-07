using System.Diagnostics;
using System.Diagnostics.Metrics;
using System.Text;
using Shouldly;
using Xunit;

namespace Cruciverba.Xfill.Tests;

/// <summary>Shared dictionary: every 2..5-letter string over eight letters (a subset of length 5).</summary>
public sealed class DictionaryFixture : IDisposable
{
    public DictionaryFixture()
    {
        Csv = BuildCsv();
        Engine = XfillEngine.LoadWordlist(Csv);
    }

    public byte[] Csv { get; }

    public XfillEngine Engine { get; }

    public void Dispose() => Engine.Dispose();

    private static byte[] BuildCsv()
    {
        char[] alpha = ['A', 'E', 'I', 'O', 'R', 'S', 'T', 'N'];
        var words = alpha.Select(c => c.ToString()).ToList();
        var csv = new StringBuilder();
        for (int len = 2; len <= 5; len++)
        {
            words = words.SelectMany(w => alpha.Select(c => w + c)).ToList();
            for (int i = 0; i < words.Count; i++)
            {
                if (len == 5 && i % 3 != 0)
                {
                    continue;
                }
                csv.Append(words[i]).Append(",Voce ").Append(len).Append('-').Append(i).Append('\n');
                if (i % 4 == 0)
                {
                    csv.Append(words[i]).Append(",Altra voce ").Append(len).Append('-').Append(i).Append('\n');
                }
            }
        }
        return Encoding.UTF8.GetBytes(csv.ToString());
    }
}

public sealed class XfillEngineTests(DictionaryFixture fixture) : IClassFixture<DictionaryFixture>
{
    private readonly XfillEngine _engine = fixture.Engine;

    private static GenerateRequest Small(int rows, int cols) => new()
    {
        Rows = rows,
        Cols = cols,
        Candidates = 12,
        Pool = 3,
        NodeBudget = 4000,
    };

    [Fact]
    public void Version_is_the_native_engine_version()
    {
        XfillEngine.Version.ShouldMatch(@"^\d+\.\d+\.\d+");
    }

    [Fact]
    public void Dictionary_reports_words_and_memory()
    {
        _engine.WordCount.ShouldBeGreaterThan(1000);
        _engine.DictionaryBytes.ShouldBeGreaterThan(_engine.WordCount);
        _engine.MeasuredLoadBytes.ShouldBeGreaterThanOrEqualTo(0);
    }

    [Fact]
    public async Task Loads_from_streams()
    {
        using (var sync = XfillEngine.LoadWordlist(new MemoryStream(fixture.Csv)))
        {
            sync.WordCount.ShouldBe(_engine.WordCount);
        }
        using var async = await XfillEngine.LoadWordlistAsync(new MemoryStream(fixture.Csv), TestContext.Current.CancellationToken);
        async.WordCount.ShouldBe(_engine.WordCount);
        Should.Throw<ArgumentNullException>(() => XfillEngine.LoadWordlist((Stream)null!));
        await Should.ThrowAsync<ArgumentNullException>(() => XfillEngine.LoadWordlistAsync(null!));
    }

    [Fact]
    public void An_empty_csv_is_rejected()
    {
        var ex = Should.Throw<XfillException>(() => XfillEngine.LoadWordlist("# niente\n"u8));
        ex.Code.ShouldBe(-6);
        ex.Message.ShouldContain("CSV");
    }

    [Fact]
    public async Task Generates_a_rectangular_grid_that_validates()
    {
        var result = await _engine.GenerateAsync(Small(6, 9) with { Blocks = 12 }, TestContext.Current.CancellationToken);

        result.Status.ShouldBe(GenerateStatus.Ok, result.Error);
        (result.Rows, result.Cols).ShouldBe((6, 9));
        result.Grid.Count.ShouldBe(6);
        result.Grid.ShouldAllBe(row => row.Length == 9);
        result.Pattern.Count.ShouldBe(6);
        result.BlackCells.Count.ShouldBe(result.Blocks);
        result.Entries.ShouldNotBeEmpty();
        result.Entries.ShouldAllBe(e => e.Clue != null && e.Answer.Length == e.Length);
        result.Entries.ShouldContain(e => e.Direction == Direction.Down);
        result.EngineVersion.ShouldBe(XfillEngine.Version);
        result.Score.ShouldBeGreaterThan(0);
        result.Attempts.ShouldBeGreaterThanOrEqualTo(1);
        result.Filled.ShouldBeGreaterThanOrEqualTo(1);
        result.Patterns.ShouldBeGreaterThanOrEqualTo(result.Attempts);
        result.Nodes.ShouldBeGreaterThan(0UL);
        result.MinScore.ShouldBeGreaterThan(0);
        result.WeakWords.ShouldNotBeNull();
        result.DurationMs.ShouldBeGreaterThanOrEqualTo(0UL);

        var report = XfillEngine.Validate(ValidateInput.FromResult(result));
        report.Valid.ShouldBeTrue();
        report.Slots.ShouldBe(result.Entries.Count);
        report.Errors.ShouldBeEmpty();
    }

    [Fact]
    public async Task Same_seed_gives_the_same_grid_with_any_thread_count()
    {
        var ct = TestContext.Current.CancellationToken;
        var one = await _engine.GenerateAsync(Small(5, 5) with { Blocks = 4, Seed = 5, Threads = 1, Symmetry = Symmetry.Mirror }, ct);
        var four = await _engine.GenerateAsync(Small(5, 5) with { Blocks = 4, Seed = 5, Threads = 4, Symmetry = Symmetry.Mirror }, ct);
        one.Status.ShouldBe(GenerateStatus.Ok, one.Error);
        four.Grid.ShouldBe(one.Grid);
        four.Entries.ShouldBe(one.Entries);
        four.Seed.ShouldBe(5UL);
    }

    [Fact]
    public async Task Theme_words_are_counted()
    {
        var request = Small(7, 7) with
        {
            ThemeWords = ["arte", "rete", "sera", "tana", "nero", "oste", "rosa", "zzzz"],
            ThemeWeight = 2,
            ThemeSlots = true,
        };
        var result = await _engine.GenerateAsync(request, TestContext.Current.CancellationToken);
        result.ThemeRequested.ShouldBe(8);
        result.ThemeInDictionary.ShouldBe(7);
        result.ThemeCount.ShouldBeGreaterThanOrEqualTo(1);
        result.ThemeWords.ShouldBe(result.Entries.Where(e => e.IsTheme).Select(e => e.Answer));
    }

    [Fact]
    public async Task Impossible_requests_return_no_grid()
    {
        var result = await _engine.GenerateAsync(Small(9, 9) with { Blocks = 0, Candidates = 3 }, TestContext.Current.CancellationToken);
        result.Status.ShouldBe(GenerateStatus.NoGrid);
        result.Error.ShouldNotBeNullOrEmpty();
        result.Grid.ShouldBeEmpty();
    }

    [Fact]
    public async Task Invalid_requests_throw()
    {
        var ex = await Should.ThrowAsync<XfillException>(() => _engine.GenerateAsync(Small(1, 9)));
        ex.Code.ShouldBe(-3);
        ex.Message.ShouldContain("rows");
        await Should.ThrowAsync<XfillException>(() => _engine.GenerateAsync(Small(5, 5) with { Optimize = "fast" }));
        await Should.ThrowAsync<ArgumentNullException>(() => _engine.GenerateAsync(null!));
    }

    [Fact]
    public async Task Cancelling_the_token_stops_the_engine_quickly()
    {
        var request = Small(7, 7) with { Blocks = 12, NodeBudget = 0, TimePerGridS = 30, Candidates = 40, Pool = 40, Threads = 2 };
        using var cts = new CancellationTokenSource();
        var task = _engine.GenerateAsync(request, cts.Token);
        await Task.Delay(200, TestContext.Current.CancellationToken);
        var watch = Stopwatch.StartNew();
        await cts.CancelAsync();
        var ex = await Should.ThrowAsync<OperationCanceledException>(() => task);
        watch.Elapsed.ShouldBeLessThan(TimeSpan.FromSeconds(1));
        ex.CancellationToken.ShouldBe(cts.Token);
    }

    [Fact]
    public async Task A_cancelled_token_throws_at_once()
    {
        using var cts = new CancellationTokenSource();
        await cts.CancelAsync();
        await Should.ThrowAsync<OperationCanceledException>(() => _engine.GenerateAsync(Small(7, 7), cts.Token));
    }

    [Fact]
    public async Task One_dictionary_serves_concurrent_generations()
    {
        var ct = TestContext.Current.CancellationToken;
        var tasks = Enumerable.Range(0, 4)
            .Select(_ => _engine.GenerateAsync(Small(5, 5) with { Blocks = 2, Seed = 3, Threads = 1 }, ct))
            .ToArray();
        var results = await Task.WhenAll(tasks);
        results.ShouldAllBe(r => r.Status == GenerateStatus.Ok);
        results.Select(r => string.Join("|", r.Grid)).Distinct().Count().ShouldBe(1);
    }

    [Fact]
    public async Task A_disposed_engine_refuses_work()
    {
        var engine = XfillEngine.LoadWordlist(fixture.Csv);
        engine.Dispose();
        engine.IsDisposed.ShouldBeTrue();
        engine.Dispose();
        await Should.ThrowAsync<ObjectDisposedException>(() => engine.GenerateAsync(Small(5, 5)));
    }

    [Fact]
    public void Validate_reports_errors()
    {
        var entries = new List<Entry>
        {
            new() { Number = 1, Direction = Direction.Across, Row = 0, Col = 0, Length = 4, Answer = "CASA", Clue = "Abitazione" },
            new() { Number = 2, Direction = Direction.Across, Row = 2, Col = 0, Length = 4, Answer = "CASA", Clue = "La casa" },
        };
        var report = XfillEngine.Validate(new ValidateInput { Rows = 3, Cols = 4, Grid = ["CASA", "####", "CASA"], Entries = entries });
        report.Valid.ShouldBeFalse();
        report.Errors.Select(e => e.Code).ShouldBe(["clue_contains_answer", "repeated_word"], ignoreOrder: true);
        var repeated = report.Errors.Single(e => e.Code == "repeated_word");
        (repeated.Number, repeated.Direction, repeated.Row, repeated.Col).ShouldBe((2, Direction.Across, 2, 0));
        repeated.Message.ShouldContain("CASA");
        report.Warnings.ShouldBeEmpty();
        report.Slots.ShouldBe(2);

        var bad = Should.Throw<XfillException>(() => XfillEngine.Validate(new ValidateInput { Rows = -1, Cols = 1 }));
        bad.Code.ShouldBe(-2);
        bad.Message.ShouldContain("bad_json");
        Should.Throw<ArgumentNullException>(() => XfillEngine.Validate(null!));
        Should.Throw<ArgumentNullException>(() => ValidateInput.FromResult(null!));
    }

    [Fact]
    public async Task Generation_emits_a_span_and_metrics()
    {
        var spans = new List<Activity>();
        using var listener = new ActivityListener
        {
            ShouldListenTo = s => s.Name == XfillTelemetry.Name,
            Sample = (ref ActivityCreationOptions<ActivityContext> _) => ActivitySamplingResult.AllDataAndRecorded,
            ActivityStopped = a => { lock (spans) { spans.Add(a); } },
        };
        ActivitySource.AddActivityListener(listener);

        var measurements = new List<(string Name, double Value)>();
        using var meters = new MeterListener
        {
            InstrumentPublished = (instrument, l) =>
            {
                if (instrument.Meter.Name == XfillTelemetry.Name)
                {
                    l.EnableMeasurementEvents(instrument);
                }
            },
        };
        meters.SetMeasurementEventCallback<double>((i, v, _, _) => { lock (measurements) { measurements.Add((i.Name, v)); } });
        meters.SetMeasurementEventCallback<long>((i, v, _, _) => { lock (measurements) { measurements.Add((i.Name, v)); } });
        meters.Start();

        using var engine = XfillEngine.LoadWordlist(fixture.Csv);
        var result = await engine.GenerateAsync(Small(5, 7) with { Blocks = 6, Seed = 9 }, TestContext.Current.CancellationToken);
        meters.RecordObservableInstruments();

        Activity span;
        lock (spans)
        {
            span = spans.Last(a => a.OperationName == "xfill.generate" && Equals(a.GetTagItem("xfill.seed"), 9UL));
            spans.ShouldContain(a => a.OperationName == "xfill.load_wordlist");
        }
        span.GetTagItem("xfill.format").ShouldBe("7x5");
        span.GetTagItem("xfill.outcome").ShouldBe(result.Status.ToString());
        span.GetTagItem("xfill.blocks").ShouldBe(result.Blocks);
        span.GetTagItem("xfill.attempts").ShouldBe(result.Attempts);
        span.GetTagItem("xfill.theme_count").ShouldBe(0);
        span.GetTagItem("xfill.score").ShouldBe(result.Score);
        lock (measurements)
        {
            measurements.ShouldContain(m => m.Name == "xfill.generate.duration" && m.Value > 0);
            measurements.ShouldContain(m => m.Name == "xfill.generate.outcomes" && m.Value == 1);
            measurements.ShouldContain(m => m.Name == "xfill.dictionary.memory" && m.Value >= engine.DictionaryBytes);
        }
    }

    [Fact]
    public async Task Failed_generations_mark_the_span_as_error()
    {
        var spans = new List<Activity>();
        using var listener = new ActivityListener
        {
            ShouldListenTo = s => s.Name == XfillTelemetry.Name,
            Sample = (ref ActivityCreationOptions<ActivityContext> _) => ActivitySamplingResult.AllDataAndRecorded,
            ActivityStopped = a => { lock (spans) { spans.Add(a); } },
        };
        ActivitySource.AddActivityListener(listener);
        await Should.ThrowAsync<XfillException>(() => _engine.GenerateAsync(Small(5, 5) with { Blocks = 99 }));
        Should.Throw<XfillException>(() => XfillEngine.LoadWordlist(ReadOnlySpan<byte>.Empty));
        lock (spans)
        {
            spans.ShouldContain(a => a.OperationName == "xfill.generate" && a.Status == ActivityStatusCode.Error
                && Equals(a.GetTagItem("xfill.outcome"), "InvalidRequest"));
            spans.ShouldContain(a => a.OperationName == "xfill.load_wordlist" && a.Status == ActivityStatusCode.Error);
        }
    }
}
