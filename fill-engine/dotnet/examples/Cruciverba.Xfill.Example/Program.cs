// Generates a 22 × 12 crossword (12 rows, 22 columns) with a theme list and prints it.
// Usage: dotnet run -- [--wordlist Db.csv] [--theme words.txt] [--seed N]
// Without --wordlist it builds a small synthetic dictionary (smoke test of the package).
using System.Text;
using Cruciverba.Xfill;

string? Arg(string name) => args.SkipWhile(a => a != name).Skip(1).FirstOrDefault();

byte[] csv = Arg("--wordlist") is { } path ? File.ReadAllBytes(path) : SyntheticCsv();
string[] theme = Arg("--theme") is { } themePath
    ? File.ReadAllLines(themePath).Select(l => l.Trim()).Where(l => l.Length > 0 && !l.StartsWith('#')).ToArray()
    : ["ARTE", "RETE", "SERA", "TANA", "NERO", "OSTE", "ROSA", "ORATE", "TENORI", "SIRENA"];
ulong seed = ulong.TryParse(Arg("--seed"), out var s) ? s : 1;

using var engine = XfillEngine.LoadWordlist(csv);
Console.WriteLine($"xfill {XfillEngine.Version} on {System.Runtime.InteropServices.RuntimeInformation.RuntimeIdentifier}");
Console.WriteLine($"dictionary: {engine.WordCount} words, {engine.DictionaryBytes / 1024 / 1024.0:F1} MB (engine), {engine.MeasuredLoadBytes / 1024 / 1024.0:F1} MB (process)");

var request = new GenerateRequest
{
    Rows = 12,
    Cols = 22,
    Seed = seed,
    ThemeWords = theme,
    // the synthetic dictionary has words of at most 6 letters
    Blocks = Arg("--wordlist") is null ? 60 : null,
};
var result = await engine.GenerateAsync(request);
Console.WriteLine($"status: {result.Status} {result.Error}");
if (result.Status != GenerateStatus.Ok)
{
    return 1;
}
foreach (var row in result.Grid)
{
    Console.WriteLine(string.Join(' ', row.ToCharArray()));
}
Console.WriteLine($"{result.Cols} × {result.Rows}: {result.Blocks} blocks, {result.Entries.Count} words, score {result.Score:F1}, " +
    $"{result.WeakWords.Count} weak, {result.Attempts} attempts, {result.DurationMs} ms");
Console.WriteLine($"theme words: {result.ThemeCount} ({string.Join(", ", result.ThemeWords)})");
var report = XfillEngine.Validate(ValidateInput.FromResult(result));
Console.WriteLine($"validation: {(report.Valid ? "ok" : string.Join("; ", report.Errors.Select(e => $"{e.Code} {e.Message}")))}");
if (!report.Valid)
{
    return 2;
}
if (result.ThemeCount == 0 || result.ThemeWords.Count != result.ThemeCount)
{
    Console.WriteLine("no theme word counted");
    return 3;
}
return 0;

static byte[] SyntheticCsv()
{
    char[] alpha = ['A', 'E', 'I', 'O', 'R', 'S', 'T', 'N'];
    var words = alpha.Select(c => c.ToString()).ToList();
    var csv = new StringBuilder();
    for (int len = 2; len <= 6; len++)
    {
        words = words.SelectMany(w => alpha.Select(c => w + c)).ToList();
        for (int i = 0; i < words.Count; i += len < 5 ? 1 : 5)
        {
            // some words only have a clue that spells them: the engine must not publish it
            string clue = len >= 3 && i % 7 == 0 ? $"La parola {words[i]}" : $"Voce {len}-{i}";
            csv.Append(words[i]).Append(',').Append(clue).Append('\n');
        }
    }
    return Encoding.UTF8.GetBytes(csv.ToString());
}
