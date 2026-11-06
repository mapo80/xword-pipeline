# fill-engine

Motore di riempimento (Rust, nessuna dipendenza esterna) e tre comandi.

```
crates/
  xfill-core/   libreria: bitset, wordlist, clues (CSV), grid, gen_it (schemi),
                solver, library (selezione + JSON), pdf, bench
  xfill-cli/    binari: xfill, library, bench
docs/           piano degli algoritmi e registro dei lavori
```

Piano, scelte e risultati dei benchmark: [docs/piano-algoritmi.md](docs/piano-algoritmi.md).

## Algoritmi

**Riempimento** (ricerca su vincoli):

- domini a bitset per (lunghezza, posizione, lettera); una parola piazzata esce
  dai domini degli altri slot della stessa lunghezza (niente doppioni);
- **arco-consistenza** sugli incroci: le lettere ammesse per casella si
  propagano fino a punto fisso, con cache incrementale (contatori di supporto);
- **backjumping guidato dai conflitti**: in un vicolo cieco si torna
  direttamente all'ultima scelta coinvolta;
- scelta dello slot con meno candidati (MRV); candidati per fascia di punteggio;
- riavvii con budget crescente; un tentativo esaurito senza tagli dimostra che
  lo schema è impossibile e chiude la ricerca;
- soglie di qualità crescenti (prima un riempimento qualsiasi, poi con parole
  più comuni) e **riparazione locale** delle parole deboli (zona attorno alla
  parola, prima tutte insieme, poi una alla volta);
- ricerca parallela (portfolio di semi) in `xfill`.

**Schemi**: ricerca in profondità sulle orbite di simmetria con memo degli stati
visitati e potatura esatta (connettività, blocchi 2×2, caselle isolate, limite
inferiore sulle nere necessarie); completa: trova uno schema se esiste.

### Opzioni del solver (`xfill`, `library`, `bench`)

| Opzione | Default | Descrizione |
|---|---|---|
| `--propagation forward\|arc` | `arc` | Propagazione: solo incroci diretti, o arco-consistenza |
| `--backjump on\|off` | `on` | Backjumping guidato dai conflitti |
| `--slot-order mrv\|per-cell` | `mrv` | Meno candidati, o meno candidati per casella vuota |
| `--linked on\|off` | `off` | Preferisci slot che incrociano la parte già riempita |
| `--lookahead K` | `0` | Riordina i primi K candidati per opzioni reali lasciate agli incroci |
| `--avoid-substrings K` | `0` | Vieta sottostringhe di K lettere comuni a due parole |
| `--repair-max-weak N` | `4` | Riparazione delle parole deboli (0 = spenta) |

## `library` — genera schemi e li riempie

Genera molti schemi con le caselle nere richieste, scarta quelli impossibili
per il dizionario, li riempie in parallelo e tiene i migliori (meno parole
"deboli", poi punteggio medio più alto). Scrive un JSON con schema, soluzione,
parole numerate e definizioni; opzionalmente i PDF.

| Opzione | Default | Descrizione |
|---|---|---|
| `--wordlist PATH` | `../../dataset/Db.csv` | CSV `PAROLA,definizione` |
| `--size RxC` | `13x13` | Righe × colonne (oppure `N` per un quadrato) |
| `--blocks N` | ~15% delle caselle | Numero di caselle nere (±1 con la simmetria) |
| `--symmetry` | `rot` | `none`, `rot` (rotazione 180°), `mirror` (speculare sinistra-destra) |
| `--gen search\|random` | `search` | Generatore di schemi: ricerca completa, o cammino casuale |
| `--max-len N` | lato maggiore | Lunghezza massima di una parola (mai oltre la più lunga del dizionario) |
| `--max-two-letter PCT` | — | Al massimo PCT% di parole da 2 lettere |
| `--no-adjacent` | off | Vieta caselle nere che si toccano |
| `--candidates N` | `200` | Schemi da generare e provare |
| `--time SECS` | `2.0` | Tempo massimo di riempimento per schema |
| `--keep-mean F` | `0` | Tieni solo riempimenti con punteggio medio ≥ F |
| `--weak-bar N` / `--max-weak N` | `58` / nessuno | Parole sotto N sono "deboli"; tieni solo riempimenti con ≤ N deboli |
| `--per-shape N` | `4` | Massimo riempimenti diversi dello stesso schema |
| `--top N` | `10` | Quanti cruciverba tenere |
| `--seed N` | `1` | Seme casuale |
| `--workers N` | n. CPU | Thread di riempimento (uno schema per thread) |
| `--out PATH` | `../out/libraries/grid-library.json` | File JSON di uscita (parole con definizione) |
| `--pdf DIR` | — | Un PDF A4 per cruciverba: schema numerato e definizioni su una pagina, soluzione sulla seguente |

## `xfill` — riempie uno schema dato

```bash
./target/release/xfill schema.txt --time 10 --boxed
./target/release/xfill schema.txt --lock "0,0,A,CASA"   # parola imposta
./target/release/xfill schema.txt --analyze             # cosa è possibile, senza cercare
```

Formato dello schema: una riga per riga della griglia; `#` casella nera, `.`
casella vuota, `A`–`Z` lettera già inserita. Altre opzioni: `--time`, `--seed`,
`--first` (primo riempimento, il thread vincente ferma gli altri),
`--workers N` (default: n. CPU), `--tiers 50,58,63`, `--min-score`.

`--analyze` stampa se lo schema è fattibile, gli slot impossibili, le lettere
forzate dalla propagazione e quante lettere restano possibili per casella.

## `bench` — confronto tra varianti

```bash
./target/release/bench --time 5 --mode first               # tempo al primo riempimento
./target/release/bench --time 3 --mode quality --backjump off
```

Suite fissa di 30 schemi italiani (9×9 … 13×21); riporta per famiglia gli
schemi riempiti, la mediana dei tempi, i nodi, il punteggio medio e le parole
deboli. Accetta tutte le opzioni del solver.

## Test

```bash
cargo test                       # unitari + integrazione dei binari
cargo llvm-cov --summary-only    # copertura
```
