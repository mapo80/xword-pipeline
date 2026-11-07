using System.Collections.Concurrent;
using System.Diagnostics;
using System.Diagnostics.Metrics;

namespace Cruciverba.Xfill;

/// <summary>
/// Traces and metrics of the SDK, both named <see cref="Name"/>: register them with
/// <c>AddSource("Cruciverba.Xfill")</c> and <c>AddMeter("Cruciverba.Xfill")</c>.
/// </summary>
public static class XfillTelemetry
{
    /// <summary>Name of the activity source and of the meter.</summary>
    public const string Name = "Cruciverba.Xfill";

    internal static readonly ActivitySource Source = new(Name, typeof(XfillTelemetry).Assembly.GetName().Version?.ToString());

    private static readonly Meter Meter = new(Name, typeof(XfillTelemetry).Assembly.GetName().Version?.ToString());

    private static readonly Histogram<double> Duration = Meter.CreateHistogram<double>(
        "xfill.generate.duration", unit: "s", description: "Duration of a generation.");

    private static readonly Counter<long> Outcomes = Meter.CreateCounter<long>(
        "xfill.generate.outcomes", description: "Generations by outcome.");

    private static readonly ConcurrentDictionary<XfillEngine, long> Dictionaries = new();

    private static readonly ObservableGauge<long> DictionaryMemory = Meter.CreateObservableGauge(
        "xfill.dictionary.memory",
        () => Dictionaries.Values.Sum(),
        unit: "By",
        description: "Native memory of the loaded dictionaries (engine estimate).");

    internal static void AddDictionary(XfillEngine engine) => Dictionaries[engine] = engine.DictionaryBytes;

    internal static void RemoveDictionary(XfillEngine engine) => Dictionaries.TryRemove(engine, out _);

    internal static void Record(Activity? activity, GenerateResult? result, string outcome, double seconds)
    {
        var tag = new KeyValuePair<string, object?>("xfill.outcome", outcome);
        Duration.Record(seconds, tag);
        Outcomes.Add(1, tag);
        if (activity is null)
        {
            return;
        }
        activity.SetTag("xfill.outcome", outcome);
        if (result is not null)
        {
            activity.SetTag("xfill.seed", result.Seed);
            activity.SetTag("xfill.blocks", result.Blocks);
            activity.SetTag("xfill.attempts", result.Attempts);
            activity.SetTag("xfill.score", result.Score);
            activity.SetTag("xfill.theme_count", result.ThemeCount);
            activity.SetTag("xfill.engine_version", result.EngineVersion);
        }
    }
}
