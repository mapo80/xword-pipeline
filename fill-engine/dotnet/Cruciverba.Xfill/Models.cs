using System.Text.Json.Serialization;

namespace Cruciverba.Xfill;

/// <summary>Block symmetry of the generated pattern.</summary>
[JsonConverter(typeof(JsonStringEnumConverter<Symmetry>))]
public enum Symmetry
{
    /// <summary>180° rotation.</summary>
    [JsonStringEnumMemberName("rot")] Rotational,
    /// <summary>Left–right mirror.</summary>
    [JsonStringEnumMemberName("mirror")] Mirror,
    /// <summary>No symmetry.</summary>
    [JsonStringEnumMemberName("none")] None,
}

/// <summary>Outcome of a generation.</summary>
[JsonConverter(typeof(JsonStringEnumConverter<GenerateStatus>))]
public enum GenerateStatus
{
    /// <summary>A grid was generated.</summary>
    [JsonStringEnumMemberName("ok")] Ok,
    /// <summary>No pattern could be generated or filled with these parameters.</summary>
    [JsonStringEnumMemberName("no_grid")] NoGrid,
    /// <summary>Stopped by cancellation.</summary>
    [JsonStringEnumMemberName("cancelled")] Cancelled,
    /// <summary>The request is invalid.</summary>
    [JsonStringEnumMemberName("invalid_request")] InvalidRequest,
}

/// <summary>Entry direction.</summary>
[JsonConverter(typeof(JsonStringEnumConverter<Direction>))]
public enum Direction
{
    /// <summary>Horizontal.</summary>
    [JsonStringEnumMemberName("across")] Across,
    /// <summary>Vertical.</summary>
    [JsonStringEnumMemberName("down")] Down,
}

/// <summary>
/// What to generate. <see cref="Rows"/> and <see cref="Cols"/> are explicit (a product format
/// written COLUMNS × ROWS, e.g. 22 × 12, is <c>Rows = 12, Cols = 22</c>). Unset (null) fields take
/// the engine defaults.
/// </summary>
public sealed record GenerateRequest
{
    /// <summary>Grid rows (height).</summary>
    public required int Rows { get; init; }
    /// <summary>Grid columns (width).</summary>
    public required int Cols { get; init; }
    /// <summary>Black squares (default ~15% of the cells).</summary>
    public int? Blocks { get; init; }
    /// <summary>Block symmetry (default rotational).</summary>
    public Symmetry? Symmetry { get; init; }
    /// <summary>Seed: same seed and request give the same grid.</summary>
    public ulong? Seed { get; init; }
    /// <summary>Worker threads (0 = one per CPU).</summary>
    public int? Threads { get; init; }
    /// <summary>Patterns to try at most.</summary>
    public int? Candidates { get; init; }
    /// <summary>Filled grids to compare before choosing.</summary>
    public int? Pool { get; init; }
    /// <summary>Search nodes per pattern (0 = time only, not reproducible).</summary>
    public ulong? NodeBudget { get; init; }
    /// <summary>Seconds per pattern (budget without nodes, otherwise a ceiling).</summary>
    public double? TimePerGridS { get; init; }
    /// <summary>Theme words, placed first and counted in the result.</summary>
    public IReadOnlyList<string>? ThemeWords { get; init; }
    /// <summary>Weight of a theme word against fill quality.</summary>
    public double? ThemeWeight { get; init; }
    /// <summary>Candidate ordering: none, weak, strong, max, reliability, theme.</summary>
    public string? Optimize { get; init; }
    /// <summary>Longest entry.</summary>
    public int? MaxLen { get; init; }
    /// <summary>Allow black squares to touch.</summary>
    public bool? AllowAdjacent { get; init; }
    /// <summary>At most this percentage of 2-letter entries.</summary>
    public int? MaxTwoLetterPct { get; init; }
    /// <summary>Entry lengths that must appear.</summary>
    public IReadOnlyList<int>? RequireLengths { get; init; }
    /// <summary>Discard fills with a lower mean score.</summary>
    public double? MinMean { get; init; }
    /// <summary>Discard fills with more weak entries.</summary>
    public int? MaxWeak { get; init; }
    /// <summary>Pick clues from the dictionary (default true).</summary>
    public bool? Clues { get; init; }
    /// <summary>Further solver options in command-line form.</summary>
    public IReadOnlyList<string>? SolverArgs { get; init; }
}

/// <summary>One numbered entry.</summary>
public sealed record Entry
{
    /// <summary>Clue number.</summary>
    public int Number { get; init; }
    /// <summary>Direction.</summary>
    public Direction Direction { get; init; }
    /// <summary>Start row (0-based).</summary>
    public int Row { get; init; }
    /// <summary>Start column (0-based).</summary>
    public int Col { get; init; }
    /// <summary>Length in cells.</summary>
    public int Length { get; init; }
    /// <summary>Answer (A–Z).</summary>
    public string Answer { get; init; } = "";
    /// <summary>Clue, if any.</summary>
    public string? Clue { get; init; }
    /// <summary>Word score (0–100).</summary>
    public int Score { get; init; }
    /// <summary>The answer is a theme word.</summary>
    public bool IsTheme { get; init; }
}

/// <summary>Result of a generation (grid fields empty unless <see cref="Status"/> is Ok).</summary>
public sealed record GenerateResult
{
    /// <summary>Outcome.</summary>
    public GenerateStatus Status { get; init; }
    /// <summary>Reason when no grid was produced.</summary>
    public string? Error { get; init; }
    /// <summary>Version of the native engine.</summary>
    public string EngineVersion { get; init; } = "";
    /// <summary>Seed used.</summary>
    public ulong Seed { get; init; }
    /// <summary>Rows.</summary>
    public int Rows { get; init; }
    /// <summary>Columns.</summary>
    public int Cols { get; init; }
    /// <summary>Number of black squares.</summary>
    public int Blocks { get; init; }
    /// <summary>Black squares as [row, col].</summary>
    public IReadOnlyList<int[]> BlackCells { get; init; } = [];
    /// <summary>Pattern rows (# black, . white).</summary>
    public IReadOnlyList<string> Pattern { get; init; } = [];
    /// <summary>Solution rows (letters, # black).</summary>
    public IReadOnlyList<string> Grid { get; init; } = [];
    /// <summary>Entries by number, across before down.</summary>
    public IReadOnlyList<Entry> Entries { get; init; } = [];
    /// <summary>Mean word score.</summary>
    public double Score { get; init; }
    /// <summary>Lowest word score.</summary>
    public int MinScore { get; init; }
    /// <summary>Answers under the weak bar.</summary>
    public IReadOnlyList<string> WeakWords { get; init; } = [];
    /// <summary>Theme answers in the grid.</summary>
    public int ThemeCount { get; init; }
    /// <summary>The theme answers.</summary>
    public IReadOnlyList<string> ThemeWords { get; init; } = [];
    /// <summary>Theme words requested.</summary>
    public int ThemeRequested { get; init; }
    /// <summary>Theme words found in the dictionary.</summary>
    public int ThemeInDictionary { get; init; }
    /// <summary>Patterns generated.</summary>
    public int Patterns { get; init; }
    /// <summary>Patterns filled before choosing.</summary>
    public int Attempts { get; init; }
    /// <summary>Patterns filled successfully.</summary>
    public int Filled { get; init; }
    /// <summary>Search nodes.</summary>
    public ulong Nodes { get; init; }
    /// <summary>Engine time in milliseconds.</summary>
    public ulong DurationMs { get; init; }
}

/// <summary>A finished grid to validate (a <see cref="GenerateResult"/> converts with <see cref="FromResult"/>).</summary>
public sealed record ValidateInput
{
    /// <summary>Rows.</summary>
    public int Rows { get; init; }
    /// <summary>Columns.</summary>
    public int Cols { get; init; }
    /// <summary>Solution rows (letters, # black).</summary>
    public IReadOnlyList<string> Grid { get; init; } = [];
    /// <summary>Entries.</summary>
    public IReadOnlyList<Entry> Entries { get; init; } = [];

    /// <summary>The grid of a generation result.</summary>
    public static ValidateInput FromResult(GenerateResult result)
    {
        ArgumentNullException.ThrowIfNull(result);
        return new ValidateInput { Rows = result.Rows, Cols = result.Cols, Grid = result.Grid, Entries = result.Entries };
    }
}

/// <summary>One problem found by validation.</summary>
public sealed record ValidationIssue
{
    /// <summary>Code (bad_number, answer_mismatch, crossing_conflict, repeated_word…).</summary>
    public string Code { get; init; } = "";
    /// <summary>Description.</summary>
    public string Message { get; init; } = "";
    /// <summary>Entry number, if any.</summary>
    public int? Number { get; init; }
    /// <summary>Entry direction, if any.</summary>
    public Direction? Direction { get; init; }
    /// <summary>Row, if any.</summary>
    public int? Row { get; init; }
    /// <summary>Column, if any.</summary>
    public int? Col { get; init; }
}

/// <summary>Result of validation.</summary>
public sealed record ValidationReport
{
    /// <summary>No errors.</summary>
    public bool Valid { get; init; }
    /// <summary>Errors.</summary>
    public IReadOnlyList<ValidationIssue> Errors { get; init; } = [];
    /// <summary>Warnings.</summary>
    public IReadOnlyList<ValidationIssue> Warnings { get; init; } = [];
    /// <summary>Entries expected from the pattern.</summary>
    public int Slots { get; init; }
}

internal sealed record WordlistStats
{
    public long Words { get; init; }
    public long Bytes { get; init; }
}

[JsonSourceGenerationOptions(
    PropertyNamingPolicy = JsonKnownNamingPolicy.SnakeCaseLower,
    DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull)]
[JsonSerializable(typeof(GenerateRequest))]
[JsonSerializable(typeof(GenerateResult))]
[JsonSerializable(typeof(ValidateInput))]
[JsonSerializable(typeof(ValidationReport))]
[JsonSerializable(typeof(WordlistStats))]
internal sealed partial class XfillJsonContext : JsonSerializerContext
{
}
