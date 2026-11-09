# Cruciverba.Xfill

SDK .NET 10 del motore di cruciverba `xfill`: genera, numera e definisce
cruciverba (anche rettangolari e a tema), verifica griglie finite, si annulla
con un `CancellationToken`, espone tracce e metriche OpenTelemetry.

Il motore nativo gira in un **processo a parte** (`xfill-host`, che carica la
libreria `libxfill`): un guasto nativo — abort, accesso non valido alla
memoria, stack overflow, memoria esaurita — chiude quel processo, **mai
l'applicazione**. Il pacchetto contiene libreria e host per `win-x64`,
`win-arm64`, `linux-x64`, `linux-arm64`, `osx-x64`, `osx-arm64`
(`runtimes/<rid>/native/`).

## Uso

```csharp
using Cruciverba.Xfill;

using var engine = XfillEngine.Start();

// una volta per versione dei dati (per esempio dopo un import): CSV → file del dizionario
await engine.BuildDictionaryAsync("Db.csv", "dict-v3.xfd");          // PAROLA,definizione

// a ogni avvio: il file si apre in circa un millisecondo (mappato, nessun parsing)
using var dictionary = await engine.OpenDictionaryAsync("dict-v3.xfd");
var result = await dictionary.GenerateAsync(new GenerateRequest
{
    Rows = 12,          // il formato 22 × 12 si scrive COLONNE × RIGHE
    Cols = 22,
    Seed = 42,          // stesso seme + stessa richiesta ⇒ stessa griglia
    ThemeWords = ["ROMA", "MILANO", "TEVERE"],
}, cancellationToken);

if (result.Status == GenerateStatus.Ok)
{
    var report = await engine.ValidateAsync(ValidateInput.FromResult(result));
}
```

- **Un `XfillEngine` = un processo motore.** Serve più dizionari e più
  generazioni in parallelo (al massimo `MaxConcurrentGenerations`, default
  uno per CPU; le altre aspettano). `Dispose` chiude il processo dopo le
  chiamate in corso.
- **Dizionari su file (`.xfd`)**: `BuildDictionaryAsync` (da file o stream
  CSV) scrive il file in modo atomico; `OpenDictionaryAsync` lo apre mappato:
  pagine del file, condivise e recuperabili dal sistema, nessuna copia. Un file
  danneggiato o di un'altra versione del formato → `XfillException` −6: si
  ricostruisce.
- **Crash**: se il processo motore termina, le chiamate in corso falliscono
  con `XfillEngineCrashedException` (codice di uscita e ultime righe di errore
  del processo); la chiamata successiva avvia un processo nuovo e riapre da
  sola i dizionari in uso. Le chiamate fallite non vengono ripetute.
- **Memoria**: su Linux il processo motore si offre come prima vittima
  dell'OOM killer (`oom_score_adj` 1000): se la memoria finisce, si ferma il
  motore, non l'applicazione.
- `GenerateAsync`: annullare il token ferma il motore in pochi millisecondi
  (`OperationCanceledException`). Richiesta non valida → `XfillException`
  (−3); nessuna griglia possibile → risultato con `Status = NoGrid`.
- `XfillEngineOptions.HostPath` / `LibraryPath` (o la variabile
  `XFILL_HOST_PATH`) per usare file diversi da quelli del pacchetto. Su Unix
  l'SDK ripristina il bit di esecuzione dell'host (gli archivi NuGet non lo
  conservano), o ne usa una copia privata se la cartella è in sola lettura.

## Telemetria

`ActivitySource` e `Meter` si chiamano `Cruciverba.Xfill`
(`AddSource("Cruciverba.Xfill")`, `AddMeter("Cruciverba.Xfill")`).

| Nome | Tipo | Contenuto |
|---|---|---|
| `xfill.generate` | span | `xfill.rows`, `xfill.cols`, `xfill.format` (COLONNE×RIGHE), `xfill.seed`, `xfill.blocks`, `xfill.attempts`, `xfill.outcome`, `xfill.score`, `xfill.theme_count`, `xfill.engine_version` |
| `xfill.build_dictionary`, `xfill.open_dictionary` | span | `xfill.dictionary.words`, `xfill.dictionary.bytes` |
| `xfill.generate.duration` | istogramma (s) | per `xfill.outcome` |
| `xfill.generate.outcomes` | contatore | per `xfill.outcome` |
| `xfill.dictionary.memory` | gauge (byte) | dimensione dei dizionari aperti (in gran parte mappati dal file) |

## Sviluppo

```bash
cd fill-engine
cargo build --release -p xfill-ffi        # libreria nativa in target/release
host/build.sh                             # xfill-host accanto alla libreria
cd dotnet
dotnet test --coverlet --coverlet-include "[Cruciverba.Xfill]*" \
  --coverlet-output-format cobertura --coverlet-threshold 90 --coverlet-threshold-stat Total
./pack-local.sh 0.2.0                     # pacchetto con libreria e host di questa macchina
dotnet run --project examples/Cruciverba.Xfill.Example -c Release -- \
  --wordlist ../../../dataset/Db.csv --theme ../bench/temi/geografia.txt
```

`dotnet test` usa Microsoft.Testing.Platform (vedi `global.json`); libreria e
host si prendono da `fill-engine/target/release` (proprietà
`XfillCargoTarget`). I test provocano guasti nativi veri nel processo motore
(abort, SIGSEGV, stack overflow: variabile `XFILL_HOST_FAULT_INJECTION`, mai
attiva fuori dai test) e verificano che il processo dei test sopravviva e che
la generazione successiva riparta. La CI (`.github/workflows/nuget.yml`)
compila librerie e host per i sei RID (host con MSVC `/W4 /WX` su Windows),
esegue i test su Windows, Linux e macOS, impacchetta e pubblica su GitHub
Packages ai tag `xfill-v<versione>`.
