# fill-engine

Motore di riempimento (Rust), usabile come libreria (API Rust, interfaccia C,
SDK .NET) e da quattro comandi.

```
crates/
  xfill-core/   libreria: bitset, wordlist (+ rebus), clues (CSV), grid,
                gen_it (schemi), solver, insight (sezioni), words (pattern,
                anagrammi), llm (definizioni), library (selezione + JSON),
                api (generate/validate, JSON), pdf, bench
  xfill-cli/    binari: xfill, library, bench, parole
  xfill-ffi/    interfaccia C (libreria xfill: xfill.dll / libxfill.so /
                libxfill.dylib), header include/xfill.h generato da cbindgen
  xfill-perf/   benchmark e profilazione (quick, gen, heap, load, xfd), non pubblicato
host/           xfill-host.c: il processo che esegue la libreria (isolamento dei guasti)
dotnet/         SDK .NET Cruciverba.Xfill, test, esempio (vedi dotnet/README.md)
bench/temi/     liste a tema di prova per `bench --theme`
docs/           piano degli algoritmi e registro dei lavori
```

Dipendenze: `serde`/`serde_json` (contratto JSON dell'API); il resto è
scritto da zero.

Piano, scelte e risultati dei benchmark: [docs/piano-algoritmi.md](docs/piano-algoritmi.md).
Memoria, CPU e velocità (misure, strumenti, procedura per ripeterle):
[docs/prestazioni.md](docs/prestazioni.md).

## Algoritmi

**Riempimento** (ricerca su vincoli):

- domini a bitset per (lunghezza, posizione, simbolo); una parola piazzata esce
  dai domini degli altri slot della stessa lunghezza (niente doppioni);
- **arco-consistenza** sugli incroci, con cache incrementale delle lettere
  ammesse (contatori di supporto);
- **backjumping guidato dai conflitti**; apprendimento opzionale tra i riavvii;
- scelta dello slot (MRV, per casella vuota, vicinanza alla parte riempita);
- ordinamento dei candidati per modalità (`weak`, `strong`, `max`,
  `reliability`, `theme`, `none`), candidati caricati a lotti (mai troncati);
- riavvii con budget crescente; un tentativo esaurito dimostra che lo schema è
  impossibile; ricerca esaustiva su richiesta;
- soglie di qualità crescenti e **riparazione locale** delle parole deboli;
- N migliori soluzioni con branch and bound;
- vincoli: niente sottostringhe ripetute, pangramma, rebus;
- ricerca parallela (portfolio di semi o divisione del primo slot).

**Schemi**: ricerca in profondità sulle orbite di simmetria, completa (trova
uno schema se esiste), con vincoli su lunghezze, parole a tutta riga, rettangoli
bianchi, reticolo.

### Opzioni del solver (`xfill`, `library`, `bench`)

| Opzione | Default | Descrizione |
|---|---|---|
| `--propagation forward\|arc` | `arc` | Propagazione: solo incroci diretti, o arco-consistenza |
| `--backjump on\|off` | `on` | Backjumping guidato dai conflitti |
| `--learn on\|off` | `off` | Parole dimostrate impossibili escluse nei riavvii successivi |
| `--slot-order mrv\|per-cell` | `mrv` | Meno candidati, o meno candidati per casella vuota |
| `--linked off\|near\|on` | `off` | Prossimo slot entro 2 incroci (`near`) o incrociante (`on`) la parte riempita |
| `--optimize none\|weak\|strong\|max\|reliability\|theme` | `weak` | Ordine dei candidati |
| `--theme-list FILE` | — | Parole a tema (una per riga) per `--optimize theme` |
| `--lookahead K` | `0` | Riordina i primi K candidati per opzioni reali lasciate agli incroci |
| `--max-cands M`, `--cand-ratio P` | `400`, `1` | Lotti di candidati di min(M, max(20, N/P)) |
| `--avoid-substrings K` | `0` | Vieta sottostringhe di K lettere comuni a due parole |
| `--repair-max-weak N` | `4` | Riparazione delle parole deboli (0 = spenta) |
| `--pangram K`, `--pangram-alphabet ABC…` | `0`, 21 lettere italiane | Ogni lettera almeno K volte |
| `--exhaustive` | off | Un solo tentativo senza budget: riempie o dimostra l'impossibilità |
| `--parallel portfolio\|split` | `portfolio` | Come più thread lavorano sullo stesso schema |
| `--eco` | off | Thread a bassa priorità |
| `--node-limit N` | `0` | Budget di lavoro (nodi + candidati provati) invece del solo tempo: risultato riproducibile |
| `--theme-weight W` | `0` | Peso di una parola a tema nel confronto tra riempimenti |
| `--theme-slots on\|off` | `off` | Con `theme`: prima gli slot che possono ancora ospitare una parola a tema (il più vincolato tra loro) |

## `library` — genera schemi e li riempie

Genera molti schemi con le caselle nere richieste, scarta quelli impossibili
per il dizionario, li riempie in parallelo e tiene i migliori. Scrive un JSON
con schema, soluzione, parole numerate e definizioni; opzionalmente i PDF.

| Opzione | Default | Descrizione |
|---|---|---|
| `--wordlist PATH` | `../../dataset/Db.csv` | CSV `PAROLA,definizione` |
| `--size RxC` | `13x13` | Righe × colonne (oppure `N` per un quadrato) |
| `--blocks N` | ~15% delle caselle | Numero di caselle nere (±1 con la simmetria) |
| `--symmetry none\|rot\|mirror` | `rot` | Simmetria delle nere |
| `--gen search\|random` | `search` | Generatore di schemi |
| `--max-len N` | lato maggiore | Lunghezza massima di una parola |
| `--max-two-letter PCT` | — | Al massimo PCT% di parole da 2 lettere |
| `--min-len-count L:N,…` | — | Almeno N parole di lunghezza L |
| `--require-lengths L,…` | — | Lunghezze che devono comparire (es. per parole a tema) |
| `--max-full-words N` | — | Al massimo N parole a tutta riga/colonna |
| `--max-white-rect A` | — | Nessun rettangolo bianco più grande di A caselle |
| `--lattice` | off | Reticolo di nere nelle caselle (dispari, dispari) |
| `--no-adjacent` | off | Vieta caselle nere che si toccano |
| `--rebus RE,…` | — | Stringhe che il solver può mettere in una sola casella |
| `--candidates N`, `--time SECS` | `200`, `2.0` | Schemi da provare e tempo per schema |
| `--keep-mean F`, `--weak-bar N`, `--max-weak N` | `0`, `58`, — | Filtri di qualità |
| `--per-shape N`, `--top N` | `4`, `10` | Selezione dei migliori |
| `--seed N`, `--workers N` | `1`, n. CPU | Seme e thread |
| `--llm-clues off\|missing\|all` | `off` | Definizioni da un LLM per le parole senza (o per tutte) |
| `--llm-config FILE`, `--llm-model M` | `../../.alibaba`, `qwen3.8-flash` | File con URL (riga 1) e chiave (riga 2); modello |
| `--out PATH`, `--pdf DIR` | `../out/libraries/grid-library.json`, — | JSON e PDF |

## Libreria: dizionari, `api::generate` e interfaccia C

```rust
use xfill_core::api::{cancel_flag, generate, Dictionary, GenerateRequest};
// una volta per versione dei dati: CSV → file del dizionario
Dictionary::from_csv_bytes(&std::fs::read("Db.csv")?)?.save("dict.xfd".as_ref())?;
// a ogni avvio: il file si mappa e si usa così com'è (~1 ms, nessuna copia)
let dict = Dictionary::open("dict.xfd".as_ref())?;
let req: GenerateRequest = serde_json::from_str(r#"{"rows": 12, "cols": 22, "theme_words": ["ROMA"]}"#)?;
let res = generate(&dict, &req, &cancel_flag());
```

- **File del dizionario (`.xfd`)**: le strutture del dizionario (parole,
  punteggi, maschere, definizioni compresse) scritte come stanno in memoria,
  con intestazione, versione del formato e checksum
  ([`src/xfd.rs`](crates/xfill-core/src/xfd.rs)). `Dictionary::open` mappa il
  file: niente parsing, niente copie, nessun picco; le pagine sono del file
  (condivise, recuperabili dal sistema). Un file danneggiato o di un altro
  formato è rifiutato con un errore (si ricostruisce dal CSV).
- `rows` e `cols` sono sempre espliciti (un formato «22 × 12», COLONNE ×
  RIGHE, è `rows: 12, cols: 22`).
- Con `node_budget` (default 20.000 unità di lavoro per schema: nodi +
  candidati provati) il risultato dipende solo da seme e richiesta, con
  qualunque numero di thread; `time_per_grid_s` resta un tetto di sicurezza.
- `CancelFlag` (`Arc<AtomicBool>`) ferma la generazione in pochi ms.
- `threads: 0` (default) = un thread per griglia del pool (2, o 4 con parole a
  tema), al massimo uno per CPU: altri thread riempirebbero schemi poi
  scartati.
- Memoria: Db.csv (35.942 parole) = file di 2,7 MiB, aperto in 0,7 ms; l'export
  della knowledge base (200.193 parole) = 15 MiB, aperto in 3,9 ms. Ogni
  thread di riempimento 0,7–1,2 MiB. Per lunghezza si tengono al massimo
  65.535 parole, le migliori.
- `validate` controlla numerazione, coordinate, incroci, risposte rispetto
  alla griglia, parole ripetute, definizioni che contengono la risposta.

L'interfaccia C (`crates/xfill-ffi/include/xfill.h`), JSON UTF-8 in/out:
`xfill_version`, `xfill_dict_build` (CSV in memoria → file), `xfill_dict_open`
(file → handle), `xfill_wordlist_free`, `xfill_wordlist_stats`,
`xfill_generate`, `xfill_validate`, `xfill_string_free`. Codici: 0 ok,
−1 argomento, −2 JSON, −3 richiesta non valida, −4 panic interno,
−5 annullata, −6 dati non validi (CSV senza parole, file danneggiato o di un
altro formato), −7 file non leggibile o non scrivibile. Header:
`cbindgen --config cbindgen.toml --output include/xfill.h` (in
`crates/xfill-ffi`; la CI verifica che sia aggiornato).

## `xfill-host` — il motore in un processo a parte

[`host/xfill-host.c`](host/xfill-host.c) (C11, nessuna dipendenza) carica la
libreria compilata (`dlopen` / `LoadLibraryExW`) e ne inoltra l'interfaccia C
su stdin/stdout (messaggi binari: `BUILD`, `OPEN`, `CLOSE`, `GENERATE`,
`CANCEL`, `VALIDATE`). Un guasto nativo (abort, accesso non valido alla
memoria, stack overflow, memoria esaurita) chiude solo questo processo; su
Linux si offre come prima vittima dell'OOM killer (`oom_score_adj` 1000). Lo
usa l'SDK .NET ([dotnet/README.md](dotnet/README.md)).

```bash
host/build.sh                 # accanto alla libreria in target/release (Linux, macOS, MinGW)
cl /nologo /O2 /W4 /WX /std:c11 host\xfill-host.c /Fe:target\release\xfill-host.exe   # MSVC
```

## `xfill` — riempie uno schema dato

```bash
./target/release/xfill schema.txt --time 10 --boxed
./target/release/xfill schema.txt --lock "0,0,A,CASA"     # parola imposta
./target/release/xfill schema.txt --best 5                # le 5 migliori soluzioni
./target/release/xfill schema.txt --suggest 0,0,A         # candidati per uno slot
./target/release/xfill schema.txt --analyze               # sezioni e lettere forzate
```

Schema: una riga per riga della griglia; `#` nera, `.` vuota, `A`–`Z` lettera
già inserita, `[RE]` casella rebus. Altre opzioni: `--first`, `--workers N`
(default automatico), `--seed`, `--tiers`, `--min-score`, `--rebus RE,…`,
`--suggest-check SECS`, `--theme-list FILE`.

## `parole` — strumenti sul dizionario

```bash
./target/release/parole pattern "?A?A" --clues          # ? * @ (vocale) # (consonante) [..] [^..]
./target/release/parole anagram ROMANO --words 2         # anche con ? come jolly
```

## `bench` — confronto tra varianti

```bash
./target/release/bench --time 5 --mode first
./target/release/bench --time 3 --mode quality --optimize max --threads 8
```

Suite fissa di 30 schemi italiani; accetta tutte le opzioni del solver.

```bash
./target/release/bench --theme bench/temi/geografia.txt --runs 3 \
    --formats 13x13,15x15,12x18,12x22 --request '{"theme_weight": 1}'
```

Con `--theme` misura la densità del tema: generazioni complete per formato
e seme, parole a tema per griglia, parole, media, deboli, tempo.

## Test

```bash
cargo test                       # unitari + integrazione dei binari
cargo llvm-cov --summary-only    # copertura
```
