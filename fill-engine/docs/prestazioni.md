# Prestazioni: memoria, CPU, velocità

Analisi e ottimizzazione del motore per un ambiente con poca memoria: prima un
thread solo, poi il parallelo. Qui sono raccolti la procedura seguita, gli
strumenti, i risultati e i passi per ripetere le misure.

Punto di partenza: commit `45e8340` (versione 0.1.1). Unità: 1 MiB =
1.048.576 byte.

## Regole seguite

1. **Stessi risultati, bit per bit.** Con `node_budget` il motore è
   deterministico. Ogni ottimizzazione deve dare le stesse griglie, le stesse
   definizioni e lo stesso numero di nodi per ogni seme (impronte, vedi sotto).
   Le ottimizzazioni che cambiano il risultato restano fuori da questo lavoro.
2. **Misurare prima di cambiare**: profilo della CPU, profilo dell'heap,
   footprint del processo. Si tiene una modifica solo se migliora la misura.
3. **TDD** sui comportamenti nuovi (stop dei riempimenti in corso, thread
   automatici, limite di parole per lunghezza): test rosso, poi verde.
4. **Ciclo di prova breve**: una suite che gira in circa 2 s per ogni
   tentativo, e la suite completa solo per la validazione finale.

## Ambiente di misura

- Apple M2 Pro (6 core performance + 4 efficiency), 32 GB, macOS 26 (Darwin 25.3).
- Rust 1.99.1, profilo `release` del workspace (`opt-level 3`, LTO thin,
  `codegen-units 1`).
- Dizionario `dataset/Db.csv`: 90.319 righe, 35.942 risposte, 90.057
  definizioni.
- Le misure di riferimento sono state fatte su una copia del motore fuori dal
  repository (`git archive`), per non toccare il `target/` usato da altri
  lavori.

## Strumenti

| Strumento | Cosa misura | Installazione |
|---|---|---|
| `xfill-perf` (crate del workspace, non pubblicato) | `quick`: suite rapida del solver; `gen`: `api::generate` completo con impronte; `heap`: profilo dhat | incluso |
| [samply](https://github.com/mstange/samply) | profilo della CPU a campionamento, grafo delle chiamate (Firefox Profiler) | `cargo install samply --locked` |
| `sample` (macOS) | campionamento testuale di un processo in corso, con file e riga | di sistema |
| `xctrace` (Instruments) | Time Profiler / Allocations da riga di comando | Xcode |
| [dhat-rs](https://docs.rs/dhat) | heap: picco, totale e allocazioni **per sito di chiamata** | dipendenza di `xfill-perf` |
| `/usr/bin/time -l` | RSS massimo e **peak memory footprint** del processo | di sistema |
| `footprint`, `vmmap --summary` | footprint per categoria (heap vivo, heap liberato ma trattenuto, codice) | di sistema |
| allocatore contatore (`xfill_perf::Counting`) | heap vivo, picco, numero di allocazioni e byte allocati per fase | incluso |

Il profilo `profiling` del workspace (`release` + simboli di debug) serve a
samply, `sample` e dhat per ricavare funzioni e righe.

## Procedura per ripetere le misure

Tutti i comandi vanno lanciati da `fill-engine/`, con `DB=../../dataset/Db.csv`.

### 1. Compilazione

```bash
cargo build --release -p xfill-perf            # quick, gen, heap
cargo build --profile profiling -p xfill-perf  # stessi binari con i simboli
```

### 2. Riferimento (versione da confrontare)

Si compila la versione di partenza in una cartella a parte, con l'harness
copiato dentro:

```bash
git worktree add /tmp/xf-base 45e8340
cp -r crates/xfill-perf /tmp/xf-base/fill-engine/crates/
# in /tmp/xf-base/fill-engine/Cargo.toml aggiungere "crates/xfill-perf" ai members
(cd /tmp/xf-base/fill-engine && cargo build --release -p xfill-perf)
```

L'harness usa solo API già presenti in 45e8340 e compila così com'è. Le
impronte di riferimento si producono con i passi 3 e 4 su quella copia
(`quick-base.txt`, `full-base.txt`).

### 3. Suite rapida del solver (circa 2 s)

20 schemi fissi (4 per formato: 9×9, 13×13, 15×15, 12×18, 12×22, seme 7),
ciascuno riempito con 3.000 unità di lavoro, un thread:

```bash
./target/release/quick $DB 3000 4 1 > quick.txt
tail -1 quick.txt   # µs per nodo, allocazioni, picco di heap del solve
diff <(grep ^FP quick-base.txt) <(grep ^FP quick.txt) && echo IDENTICI
```

Le righe `FP` (nodi, riavvii, media, deboli, hash del riempimento) devono
coincidere con quelle del riferimento. Per i tempi si fanno 3 esecuzioni e si
prende il minimo: il rumore tra due esecuzioni è del 3–8%.

### 4. Generazione completa e impronte

```bash
PERF_FP=1 ./target/release/gen $DB 10 5 9x9,13x13,15x15,12x18,12x22 > full.txt
diff <(grep ^FP full-base.txt) <(grep ^FP full.txt) && echo IDENTICI
./target/release/gen $DB 1 3 9x9,13x13,15x15,12x18,12x22   # tempi, un thread
./target/release/gen $DB 0 3 9x9,13x13,15x15,12x18,12x22   # thread automatici
```

Argomenti: `gen <csv> <thread> <semi> <formati>`; `thread 0` = default
dell'API. Le righe `FP` contengono griglia, risposte e definizioni (hash),
nodi, schemi provati e punteggio.

### 5. CPU

```bash
samply record -- ./target/profiling/quick $DB 3000 4 1     # apre il profilo
samply record --save-only -o quick.json.gz -- ./target/profiling/quick $DB 3000 4 1
samply load quick.json.gz
```

Per un riepilogo testuale con file e riga, su un processo in corso:

```bash
./target/profiling/quick $DB 3000 4 1 & sample $! 2 -mayDie -file prof.txt
grep -A15 'Sort by top of stack' prof.txt
```

Per separare dal chiamante una funzione inlined, durante l'analisi le si
mette temporaneamente `#[inline(never)]`.

### 6. Heap per sito di chiamata (dhat)

```bash
./target/profiling/heap $DB 13x13 1 1                  # caricamento + generazione
HEAP_PHASE=gen ./target/profiling/heap $DB 15x15 1 1   # solo la generazione
```

Il file `dhat-heap.json` si apre in
[dh_view](https://nnethercote.github.io/dh_view/dh_view.html). Ordinare per
"At t-gmax" dà i siti che compongono il picco, per "Total (blocks)" quelli
che allocano più spesso.

### 7. Memoria del processo

```bash
/usr/bin/time -l ./target/release/gen $DB 1 3 13x13,15x15 2>&1 | grep -E 'GEN|peak memory footprint|maximum resident'
PERF_PAUSE=5 ./target/release/gen $DB 1 1 13x13 &   # stampa il pid e aspetta
footprint <pid>; vmmap --summary <pid>
```

Su Linux: `/usr/bin/time -v` (Maximum resident set size) e
`/proc/<pid>/smaps_rollup`.

### 8. Prima di chiudere

```bash
cargo test --release      # 25 test CLI, 172 core, 10 FFI
cargo clippy --all-targets --release
cargo fmt --all --check
```

## Diagnosi iniziale (45e8340)

- **CPU**: tra l'85 e il 90% del tempo è nella propagazione
  (`recurse → revise_from → remove_words`). Per ogni nodo si clonavano bitset
  grandi quanto il dizionario e si allocava a ogni salvataggio nel trail:
  **13,7 milioni di allocazioni e 4,1 GB allocati** per una 13×13 con un
  thread.
- **Parallelo**: con il default (un thread per CPU) la generazione era **4
  volte più lenta che con un thread** e consumava 30 volte la CPU. Quando le
  griglie del pool erano pronte, gli altri thread continuavano a riempire fino
  in fondo schemi poi scartati, perché lo stop veniva controllato solo tra uno
  schema e l'altro.
- **Memoria**: dizionario da 11,6 MiB in 431.931 allocazioni. Le definizioni
  stavano in una `HashMap<String, Vec<String>>` (6,4 MiB), le parole in un
  `Box<[u8]>` ciascuna. Il picco di caricamento era di 13,4 MiB, perché il CSV
  veniva riscritto come testo "PAROLA;punteggio" e riletto.

## Interventi (in ordine, ciascuno misurato)

Suite rapida: µs per nodo (minimo di 3 esecuzioni) e allocazioni.

| # | Intervento | µs/nodo | Allocazioni | Esito |
|---|---|---|---|---|
| 0 | Partenza | 277 | 12,09 M | — |
| 1 | Rimozione in una passata senza bitset temporanei; trail in arene contigue (istantanee senza allocazioni); coda di revisione riusata; lettere contigue per lunghezza (`flat`) | 152 | 0,57 M | tenuto |
| 2 | Contatori di supporto *ibridi*: se si rimuovono più parole di quante ne restino, i contatori si ricalcolano dalle rimaste invece di decrementarli | (in 1) | | tenuto |
| 3 | Per le lettere da togliere, OR delle maschere sull'insieme più piccolo (rimosse oppure tenute, complementato): ogni parola ha una sola lettera per posizione | 156 | | tenuto (neutro, base per 4) |
| 4 | Aggiornamento delle maschere senza diramazioni (`mask &= !((v == 0) << c)`) | 146 | | tenuto |
| — | Blocchi radi: controllo parola per parola (lettere per colonna) quando il blocco ha poche parole vive | 168 | | **scartato** (più lento) |
| — | Supporti residui (un testimone per lettera, nessun contatore) | 200 | | **scartato**: 51 M ricerche di testimoni, il 30% dei controlli |
| — | Niente istantanee dei contatori nel trail (ricalcolo all'undo) | stima ×2 sulla propagazione | | **scartato**: l'undo costerebbe quanto le rimozioni, cioè 354 M parole contro 208 M |
| 5 | Dizionario compatto (sotto) | 147 | 0,48 M | tenuto |
| 6 | Contatori `u16` con passo di 26 simboli (più i rebus) invece di 32 | 147 | | tenuto: trail dei contatori −60% |
| 7 | Candidati in una pila condivisa tra i livelli, insiemi di conflitto per profondità, ordinamento senza buffer (ordine identico allo stabile), distanze di `linked` solo se l'opzione è attiva | 147 | 0,33 M | tenuto |

A livello di generazione:

| # | Intervento | Effetto |
|---|---|---|
| A | `SolveConfig::halt`: `fill_patterns` ferma i riempimenti in corso quando il pool è completo, perché i loro risultati verrebbero comunque scartati (test `a_complete_pool_stops_the_fills_in_flight`: 4,0 s → < 1 s) | 13×13 con 10 thread: 17,2 s → 1,3 s |
| B | Thread automatici (`threads: 0`) = min(CPU, pool) invece di uno per CPU (test `automatic_threads_are_one_per_grid_of_the_pool`) | stessa griglia, CPU e memoria per thread solo dove servono |
| C | Dizionario compatto: definizioni in un'arena (risposte ordinate e concatenate, testi concatenati per risposta, offset `u32`); parole solo in `flat`; maschere compat in un unico blocco con una sequenza di zeri condivisa per le coppie (posizione, lettera) assenti (4.596 presenti su 9.568) | 11,6 → **4,05 MiB**; 431.931 → **747** allocazioni |
| D | Caricamento in due passate con le dimensioni esatte calcolate prima (nessuna crescita per raddoppio), indice di 12 byte per riga, lettere delle risposte sullo stack | picco di caricamento 13,4 → **4,5 MiB** |
| E | Al massimo 65.535 parole per lunghezza, le migliori (test `a_length_keeps_at_most_max_words_the_best_ones`); Db.csv ne ha al massimo 6.468 | permette i contatori `u16` |

## Risultati

Impronte identiche alla partenza su tutte le misure: 20 schemi della suite
rapida e 25 generazioni (5 formati × 5 semi) con 1 thread. Griglie,
definizioni, punteggi e numero di nodi sono uguali. Con 10 thread il
risultato è identico a quello con 1 thread, nodi compresi; nella partenza,
con 10 thread, in 3 casi su 25 il conteggio dei nodi differiva leggermente
(griglie uguali).

### Thread singolo (media su 3 semi, `gen <csv> 1 3 …`)

| Formato | Tempo prima | Tempo dopo | Allocazioni prima | Allocazioni dopo |
|---|---|---|---|---|
| 9 × 9 | 0,64 s | **0,37 s** | | |
| 13 × 13 | 4,47 s | **2,28 s** | 13,66 M (4,1 GB allocati) | **94 k** (21 MB) |
| 15 × 15 | 7,42 s | **4,02 s** | 19,34 M (6,2 GB) | **123 k** (31 MB) |
| 12 × 18 | 6,16 s | **3,10 s** | | |
| 12 × 22 | 6,57 s | **3,50 s** | | |

### Default dell'API (prima: un thread per CPU, 10; dopo: automatico, 2)

| Formato | Tempo prima | CPU prima | Tempo dopo | CPU dopo |
|---|---|---|---|---|
| 9 × 9 | 5,97 s | 47,0 s | **0,21 s** | 0,40 s |
| 13 × 13 | 17,20 s | 148,2 s | **1,38 s** | 2,68 s |
| 15 × 15 | 23,51 s | 204,9 s | **2,23 s** | 4,40 s |
| 12 × 18 | 23,73 s | 193,2 s | **1,71 s** | 3,37 s |
| 12 × 22 | 23,42 s | 198,6 s | **2,15 s** | 4,09 s |

### Memoria

| | Prima | Dopo |
|---|---|---|
| Dizionario in memoria (heap vivo) | 11,6 MiB, 431.931 allocazioni | **4,05 MiB, 747 allocazioni** |
| — definizioni | 6,4 MiB | 2,9 MiB |
| — parole e maschere | 2,2 MiB | 1,15 MiB |
| Picco di caricamento (senza il buffer CSV) | 13,4 MiB | **4,5 MiB** |
| Picco della generazione, un thread (13×13 / 15×15) | +0,90 / +1,31 MiB | +0,81 / +1,34 MiB |
| Footprint del processo, un thread (5 formati) | 27,2 MiB | **14,7 MiB** |
| Footprint del processo, default | 34,2 MiB | **16,9 MiB** |
| Caricamento | 38 ms | 37 ms |

Il footprint include il buffer CSV letto dal processo (2,9 MiB). A fine
esecuzione `footprint` mostra 13 MiB, così ripartiti:
- 4,1 MiB di dizionario;
- 2,9 MiB di `MALLOC_LARGE (empty)`: il buffer CSV già liberato, che
  l'allocatore tiene in cache;
- circa 3 MiB di `MALLOC_SMALL` liberati ma ancora "sporchi" (buffer
  temporanei di caricamento e dei solver per schema);
- 0,5 MiB di codice (`__TEXT`, pagine pulite).

La memoria trattenuta dipende dall'allocatore di sistema, non dalla libreria
(vedi «Proposte»).

### Numero di thread (4 formati × 3 semi, somma)

| Thread | Tempo | CPU | Footprint |
|---|---|---|---|
| 1 | 12,7 s | 12,7 s | 14,5 MiB |
| 2 | 7,5 s | 14,6 s (+15%) | 16,7 MiB |
| 3 | 6,0 s | 17,4 s (+37%) | 17,6 MiB |
| 4 | 5,2 s | 19,9 s (+57%) | 19,8 MiB |
| 6 | 5,2 s | 29,4 s (+131%) | 23,0 MiB |

Ogni thread in più costa circa 1,1 MiB: trail del solver (0,8–1,6 MiB di
picco a seconda del formato), cache dell'allocatore e stack. Oltre il numero
di griglie del pool i thread riempiono schemi destinati a essere scartati.
Per questo il default è min(CPU, pool): 2 senza tema, 4 con il tema. Con
`threads` esplicito si sceglie un altro compromesso.

### Dove va il tempo adesso

Il profilo resta dominato dalla propagazione: aggiornare i contatori di
supporto (lettera per lettera, per le parole rimosse) e ricostruirli quando
conviene. Nella suite rapida si fanno circa 2,5 M revisioni e si toccano circa
1,2 miliardi di contatori, a circa 1 ns l'uno. A parità di algoritmo è vicino
al minimo: per scendere ancora servirebbe fare meno revisioni (ordine della
coda, euristiche), cosa che cambia il risultato e va valutata con il
benchmark di qualità (`bench`).

## Limiti noti

- Al massimo **65.535 parole per lunghezza** (`MAX_WORDS_PER_LEN`): oltre, si
  tengono le migliori per punteggio.
- Il picco di memoria del solver dipende dal formato. È il trail, che
  conserva le istantanee dei domini e dei contatori lungo il percorso di
  ricerca: 0,8 MiB in 13×13, 1,3 MiB in 15×15, 1,6 MiB in 12×22, per
  thread.

## Proposte (non fatte)

1. **Definizioni compresse in memoria** (circa −1,2 MiB): sono la parte più
   grande del dizionario (2,1 MiB di testo) e servono solo per le circa 70
   voci della griglia scelta. Un dizionario di parole frequenti o una
   compressione a blocchi le dimezzerebbe.
2. **Dizionario senza definizioni** (1,15 MiB in tutto) per chi le tiene
   altrove: serve un'opzione di caricamento in API, FFI e SDK.
3. **Restituire la memoria al sistema dopo il caricamento**
   (`malloc_zone_pressure_relief` su macOS e iOS, `malloc_trim` su glibc), o
   caricare il dizionario all'avvio prima di tutto il resto: si recuperano i
   circa 6 MiB trattenuti dall'allocatore.
4. **Un solver per thread riusato tra gli schemi**, invece di uno nuovo per
   schema: meno allocazioni (oggi circa 1.800 per schema) e meno
   frammentazione.
5. **Budget per schema** (`node_budget`, oggi 20.000): è il fattore che
   decide il tempo (ogni schema lo usa tutto per migliorare la qualità). Con
   il motore più veloce si può ricalibrare il compromesso qualità/tempo del
   punto 27.
