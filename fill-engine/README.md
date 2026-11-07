# fill-engine

Motore di riempimento (Rust, nessuna dipendenza esterna) e quattro comandi.

```
crates/
  xfill-core/   libreria: bitset, wordlist (+ rebus), clues (CSV), grid,
                gen_it (schemi), solver, insight (sezioni), words (pattern,
                anagrammi), llm (definizioni), library (selezione + JSON),
                pdf, bench
  xfill-cli/    binari: xfill, library, bench, parole
docs/           piano degli algoritmi e registro dei lavori
```

Piano, scelte e risultati dei benchmark: [docs/piano-algoritmi.md](docs/piano-algoritmi.md).

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

## Test

```bash
cargo test                       # unitari + integrazione dei binari
cargo llvm-cov --summary-only    # copertura
```
