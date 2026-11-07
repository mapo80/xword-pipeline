# Cruciverba.Xfill

SDK .NET 10 del motore di cruciverba `xfill`: genera, numera e definisce
cruciverba (anche rettangolari e a tema), verifica griglie finite, si annulla
con un `CancellationToken`, espone tracce e metriche OpenTelemetry.

Il pacchetto contiene la libreria nativa per `win-x64`, `win-arm64`,
`linux-x64`, `linux-arm64`, `osx-x64`, `osx-arm64` (`runtimes/<rid>/native/`).

## Uso

```csharp
using Cruciverba.Xfill;

using var engine = XfillEngine.LoadWordlist(File.ReadAllBytes("Db.csv")); // PAROLA,definizione
var result = await engine.GenerateAsync(new GenerateRequest
{
    Rows = 12,          // il formato 22 × 12 si scrive COLONNE × RIGHE
    Cols = 22,
    Seed = 42,          // stesso seme + stessa richiesta ⇒ stessa griglia
    ThemeWords = ["ROMA", "MILANO", "TEVERE"],
}, cancellationToken);

if (result.Status == GenerateStatus.Ok)
{
    Console.WriteLine($"{result.ThemeCount} parole a tema: {string.Join(", ", result.ThemeWords)}");
    var report = XfillEngine.Validate(ValidateInput.FromResult(result));
}
```

- Un `XfillEngine` è immutabile e serve più generazioni in parallelo;
  `Dispose` libera il dizionario nativo (le chiamate in corso lo tengono vivo).
- `GenerateAsync` gira su un thread dedicato; annullare il token ferma il
  motore in pochi millisecondi (`OperationCanceledException`).
- Richiesta non valida → `XfillException` (codice −3); nessuna griglia
  possibile → risultato con `Status = NoGrid` e `Error`.
- Memoria del dizionario: `DictionaryBytes` (stima del motore) e
  `MeasuredLoadBytes` (crescita della memoria del processo al caricamento).

## Telemetria

`ActivitySource` e `Meter` si chiamano `Cruciverba.Xfill`
(`AddSource("Cruciverba.Xfill")`, `AddMeter("Cruciverba.Xfill")`).

| Nome | Tipo | Contenuto |
|---|---|---|
| `xfill.generate` | span | `xfill.rows`, `xfill.cols`, `xfill.format` (COLONNE×RIGHE), `xfill.seed`, `xfill.blocks`, `xfill.attempts`, `xfill.outcome`, `xfill.score`, `xfill.theme_count`, `xfill.engine_version` |
| `xfill.load_wordlist` | span | `xfill.dictionary.words`, `xfill.dictionary.bytes` |
| `xfill.generate.duration` | istogramma (s) | per `xfill.outcome` |
| `xfill.generate.outcomes` | contatore | per `xfill.outcome` |
| `xfill.dictionary.memory` | gauge (byte) | memoria dei dizionari caricati |

## Sviluppo

```bash
cd fill-engine
cargo build --release -p xfill-ffi        # libreria nativa in target/release
cd dotnet
dotnet test --coverlet --coverlet-include "[Cruciverba.Xfill]*" \
  --coverlet-output-format cobertura --coverlet-threshold 90 --coverlet-threshold-stat Total
./pack-local.sh 0.1.0                     # pacchetto con la sola libreria di questa macchina
dotnet run --project examples/Cruciverba.Xfill.Example -c Release -- \
  --wordlist ../../../dataset/Db.csv --theme ../bench/temi/geografia.txt
```

`dotnet test` usa Microsoft.Testing.Platform (vedi `global.json`); la libreria
nativa si prende da `fill-engine/target/release` (proprietà
`XfillCargoTarget`). La CI (`.github/workflows/nuget.yml`) compila le sei
librerie, esegue i test su Windows, Linux e macOS, impacchetta e pubblica su
GitHub Packages ai tag `xfill-v<versione>`.
