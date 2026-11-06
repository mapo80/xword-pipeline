# fill-engine

Motore di riempimento (Rust, nessuna dipendenza esterna) e due comandi.

```
crates/
  xfill-core/   libreria: bitset, wordlist, clues (CSV), grid, gen_it (schemi),
                solver, library (selezione + JSON)
  xfill-cli/    binari: xfill, library
docs/           design del solver originale (in inglese)
```

Il solver è una ricerca CSP: domini a bitset per (lunghezza, posizione,
lettera), scelta dinamica dello slot più vincolato, candidati in ordine di
punteggio, riavvii casuali con budget crescente, soglie di qualità crescenti
(prima una soluzione qualsiasi, poi soluzioni con parole più comuni). Vedi
[docs/rust-fill-engine-design.md](docs/rust-fill-engine-design.md).

## `library` — genera schemi e li riempie

Genera molti schemi casuali con le caselle nere richieste, li riempie in
parallelo e tiene i migliori (meno parole "deboli", poi punteggio medio più
alto). Scrive un JSON con schema, soluzione e parole numerate.

| Opzione | Default | Descrizione |
|---|---|---|
| `--wordlist PATH` | `../../dataset/Db.csv` | CSV `PAROLA,definizione` (o dizionario `PAROLA;score`) |
| `--size RxC` | `13x13` | Righe × colonne (oppure `N` per un quadrato) |
| `--blocks N` | ~15% delle caselle | Numero di caselle nere (±1 con la simmetria) |
| `--symmetry` | `rot` | `none`, `rot` (rotazione 180°), `mirror` (speculare sinistra-destra) |
| `--max-len N` | lato maggiore | Lunghezza massima di una parola |
| `--no-adjacent` | off | Vieta caselle nere che si toccano |
| `--candidates N` | `200` | Schemi da generare e provare |
| `--time SECS` | `2.0` | Tempo massimo di riempimento per schema |
| `--keep-mean F` | `0` | Tieni solo riempimenti con punteggio medio ≥ F |
| `--weak-bar N` / `--max-weak N` | `58` / nessuno | Parole sotto N sono "deboli"; tieni solo riempimenti con ≤ N deboli |
| `--per-shape N` | `4` | Massimo riempimenti diversi dello stesso schema |
| `--top N` | `10` | Quanti cruciverba tenere |
| `--seed N` | `1` | Seme casuale |
| `--workers N` | n. CPU | Thread di riempimento |
| `--out PATH` | `../out/libraries/grid-library.json` | File JSON di uscita (parole con definizione) |
| `--pdf DIR` | — | Scrive un PDF A4 per ogni cruciverba tenuto: schema numerato e definizioni su una pagina (carattere e caselle si adattano), soluzione sulla pagina seguente |

## `xfill` — riempie uno schema dato

```bash
./target/release/xfill schema.txt --time 10 --boxed
./target/release/xfill schema.txt --lock "0,0,A,CASA"   # parola imposta
```

Formato dello schema: una riga per riga della griglia; `#` casella nera, `.`
casella vuota, `A`–`Z` lettera già inserita.

## Test

```bash
cargo test
```
