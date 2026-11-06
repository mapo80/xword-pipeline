# Piano: algoritmi del motore di riempimento

Obiettivo: portare il motore ai migliori algoritmi noti per il riempimento di
cruciverba e la generazione di schemi (propagazione di vincoli, backjumping,
ordinamenti dinamici, ricerca sugli schemi).

## Metodo

- **TDD rosso → verde, anche sugli algoritmi.** Ogni punto parte da un test che
  fallisce sul *comportamento algoritmico* atteso (es. "la propagazione deve
  accorgersi di questo vicolo cieco prima di scendere", "il backjumping deve
  saltare i livelli estranei al conflitto", "il generatore deve rispettare
  questo vincolo"), poi si implementa fino al verde.
- **Copertura** misurata con `cargo llvm-cov`; ogni punto chiude con il codice
  nuovo coperto.
- **Benchmark** fisso (punto B) per confrontare le varianti e decidere i default
  con i numeri.

Legenda stato: ☐ da fare · ◐ in corso · ☑ fatto

**Verifica della correttezza.** Gli algoritmi sono implementati dalla loro
definizione (letteratura sui problemi di vincoli), senza codice di terzi come
riferimento. La correttezza si verifica con: test sul comportamento atteso
scritti prima del codice; test differenziali contro un oracolo semplice (es.
backjumping vs ricerca esaustiva cronologica su schemi e dizionari casuali;
cache delle lettere vs ricalcolo da zero); benchmark su schemi reali.

## Regola "configurabile o sempre attivo"

- **Sempre attivo**: correzioni di bug e cambi che non possono peggiorare il
  risultato (stessa soluzione o migliore, a parità di tempo).
- **Configurabile** (`SolveConfig` / opzione CLI): ciò che è un compromesso
  (velocità vs. potenza di potatura, qualità vs. tempo) o cambia il risultato.
  Il **default** si decide col benchmark e si registra qui.

## Punti

| # | Aspetto | Tipo | Default | Stato |
|---|---|---|---|---|
| 0 | Test d'integrazione dei binari (`xfill`, `library`) e correzione dei bug emersi | infrastruttura + bug fix | sempre attivo | ☑ |
| 0b | Pulizia del codice inutilizzato (eredità della versione inglese) | pulizia | — | ☑ |
| B | Benchmark fisso (`bench`): schemi italiani di varie dimensioni/densità; % riempiti, tempo, qualità | infrastruttura | — | ☑ |
| 1 | Parole già usate tolte dai domini (conteggio MRV e rilevamento vicoli ciechi corretti) | bug fix | **sempre attivo** | ☑ |
| 2 | Propagazione ad arco-consistenza sugli incroci (lettere ammesse per casella fino a punto fisso) | `propagation: forward \| arc` | **arc** | ☑ |
| 3 | Backjumping guidato dai conflitti (CBJ) con insiemi di colpevoli per profondità, corretto anche con la propagazione | `backjump: bool` | **on** | ☑ |
| 4 | Ordine degli slot: MRV, MRV per casella vuota, vincolo "collegato al già riempito" | `slot_order`, `linked` | **mrv, off** | ☑ |
| 5 | Ordine dei candidati con lookahead dinamico (domini reali degli incroci) sui primi K | `lookahead: K` (0 = statico) | **0 (spento)** | ☑ |
| 6 | Analisi di uno schema: slot impossibili, lettere ammesse/forzate per casella | API + `xfill --analyze` | su richiesta | ☑ |
| 7 | Generatore di schemi a ricerca (DFS su orbite di simmetria, memo degli stati visitati, budget e riavvii, vincoli sull'istogramma delle lunghezze) + scarto immediato degli schemi infattibili | `--gen search \| random`, `--max-two-letter` | **search**; limite 2 lettere spento | ☑ |
| 8 | Niente sottostringhe ripetute di lunghezza k tra parole diverse | `avoid_substrings: k` | **spento** (stilistico) | ☑ |
| 9 | Ricerca parallela su uno schema (portfolio di semi nel core, usato da `xfill`) | `threads` | n. CPU in `xfill` | ☑ |
| 10 | Riparazione locale mirata alle parole deboli (strappa le parole deboli e i loro incroci, riempie di nuovo solo quella zona) | `repair_max_weak` | **4** (attiva) | ☑ |
| 11 | Benchmark finale, scelta dei default, documentazione | — | — | ☑ |

### Risultato finale (benchmark, Db.csv, 30 schemi)

| Configurazione | Primo riempimento (5 s) | Qualità (3 s): riempiti · media · deboli |
|---|---|---|
| Di partenza (forward, senza backjump, senza riparazione) | 19/30 | 17/30 · 74,0 · 3,4 |
| **Default attuali** (arc, backjump, mrv, riparazione 4) | **25/30** | **24/30 · 73,8 · 2,7** |

Generatore di schemi (`library`, 3 famiglie × 40 schemi): riempiti 43 → **53**.
Copertura righe 76,6% → **96,4%** (funzioni 98,7%); 88 test unitari + 14 d'integrazione.

### Esclusi (per ora) e perché

- **Pangramma** e **rebus** (più lettere in una casella): non usati nei
  cruciverba italiani classici; il rebus richiede di cambiare il modello della
  griglia.
- **Divisione dei candidati del primo slot tra thread**: il portfolio di semi
  diversi (punto 9) dà la stessa diversità senza sincronizzazione; si rivaluta
  col benchmark se servisse.
- **Nogood di lettere persistenti tra i riavvii**: il backjumping (punto 3)
  copre la stessa esigenza dentro un tentativo, in modo dimostrabilmente
  corretto.

## Registro

| Data | Punto | Cosa |
|---|---|---|
| 2026-11-06 | — | Copertura di partenza: 76,6% righe (core 91–98%, `library.rs` 35%, binari 0%) |
| 2026-11-06 | 0 | Test d'integrazione dei binari scritti. Rossi: `library` con dizionario più corto del lato dello schema non genera nulla (lunghezza massima non limitata al dizionario); `xfill` con uno slot di lunghezza assente dal dizionario riparte a vuoto senza spiegare l'errore |
| 2026-11-06 | 0b | Rimossi: lista di parole vietate (`ban_answer`), metrica `iffy` (soglia 50 della scala inglese, sempre 0 con la nostra), campi tema nel JSON, overlay della wordlist inglese e formato `PAROLA;score` come input, `build_lib_grid_from`, documento di design originale. Default italiani nel core (`SolveConfig::default`: livelli 50/58/63, soglia deboli 58). Escape JSON completo (caratteri di controllo). |
| 2026-11-06 | 0 | Verde. Il solver creava un dominio con una "parola fantasma" per le lunghezze senza parole (`ones(n.max(1))`): ora dominio vuoto e `solve` risponde subito *unsolved* senza riavvii (test `infeasible_base_fails_without_restarts`). Nuovi `Wordlist::longest` e `missing_lengths`; `xfill` si ferma con "no dictionary words of length N"; `library` limita `--max-len` alla parola più lunga del dizionario. Il test di `library` usava parametri impossibili (4 nere in 5×6 con parole ≤ 5 e simmetria: ne servono almeno 6), corretto. |
| 2026-11-06 | 0 | Copertura righe 76,6% → **94,3%** (binari 0% → 83–90%, `library.rs` 35% → 93%). 44 test core + 7 integrazione. |
| 2026-11-06 | B | Modulo `bench` nel core (suite deterministica, esecuzione, riepilogo) + binario `bench` (`--suite standard\|smoke`, `--mode first\|quality`). Test rossi → verdi (3 unitari, 2 d'integrazione). |
| 2026-11-06 | B | **Riferimento** (`bench --time 5 --mode first`, Db.csv, 10 thread): 9x9/12 5/5 · 11x11/18 5/5 · 13x13/24 2/5 · 13x13/30 5/5 · 15x15/32 1/5 · 13x21/40 1/5 · **totale 19/30**, nodi medi 225.308. |
| 2026-11-06 | 1 | Rossi (3): conteggio MRV che include parole già usate, incrocio rimasto solo con una parola usata non rilevato, parole bloccate non escluse al reset. Verde: la parola assegnata esce dai domini degli slot della stessa lunghezza (salvata nel trail; undo in ordine inverso). Benchmark: **20/30** (19 prima). Sempre attivo. |
| 2026-11-06 | 2 | Rossi (4): vicolo cieco a distanza 2 non visto dal forward checking, undo completo della propagazione, schema con lettere già inserite infattibile da riconoscere senza ricerca, `Propagation` inesistente. Verde: AC-3 sulle lettere ammesse per casella (solo candidati sotto la soglia corrente), salvataggio una volta per passo, propagazione anche all'inizio di ogni tentativo. Opzione condivisa `SolveConfig::with_args` (`--propagation`) + `describe()`. |
| 2026-11-06 | 2 | Benchmark (first, 5 s): forward **20/30** (292.604 nodi medi) · arc **23/30** (1.485 nodi medi): 13x13/24 2→3, 15x15/32 1→3. Media 71,1 → 70,4 (risolve schemi più difficili). **Default: arc** (configurabile). |
| 2026-11-06 | 3 | Rosso: su uno schema con 4 slot indipendenti (2 candidati ciascuno) scelti prima di un 2×2 arco-consistente ma senza soluzione (AB AC BA BD DA), la ricerca esaustiva usava 31 nodi con e senza salto. Verde: insiemi di colpevoli per slot (profondità che ne hanno ridotto il dominio; la propagazione eredita quelli dello slot che causa la revisione), conflitto al vicolo cieco, salto quando il conflitto del figlio non coinvolge la scelta corrente, ritorno al cronologico se i candidati sono troncati. Test di correttezza: 120 coppie casuali schema/dizionario, stesso esito della ricerca cronologica. Opzione `--backjump on\|off`. |
| 2026-11-06 | 3 | Benchmark (first, 5 s, arc): off **23/30** (1.287 nodi medi) · on **25/30** (1.029): 13x21/40 2→4. **Default: on** (configurabile). |
| 2026-11-06 | 2b | Profilo: gli schemi irrisolti facevano ~400 nodi/s, quasi tutto il tempo nel ricalcolo delle lettere ammesse a ogni revisione. Verde (invariante: cache = ricalcolo, su assegnazioni/undo casuali): lettere ammesse in cache per slot/posizione, mantenute **incrementalmente** con contatori di supporto (posizione, lettera) decrementati solo per le parole rimosse; salvate nel trail. `Solver::new` parte dai default di `SolveConfig`. Controllo del tempo ogni 16 nodi (prima 1024: sforava la scadenza fino a 2 s). Mediane 13x13/24 0,26 → 0,15 s, 13x21/40 0,18 → 0,11 s. |
| 2026-11-06 | 3b | Rosso: sul 2×2 impossibile `solve` riavviava fino alla scadenza (3 s). Verde: un tentativo esaurito senza candidati troncati è una prova di infattibilità; si ferma (fase di fattibilità e caccia alle soglie alte). |
| 2026-11-06 | 2/3 | I 5 schemi non riempiti restano tali anche con 30 s (fino a 56.000 nodi; con forward checking 500.000 nodi in 5 s): probabilmente senza soluzione con questo dizionario. Il costo per nodo dell'arco-consistenza su domini grandi (rimozione di migliaia di parole per nodo) resta da rivedere nel punto 11. |
| 2026-11-06 | 4 | Rossi (3): scelta per candidati per casella vuota, preferenza per gli slot collegati alla parte riempita, opzioni. Verde: `SlotOrder::{Mrv, PerCell}`, `linked`, opzioni `--slot-order mrv\|per-cell`, `--linked on\|off`. Benchmark: tutte e 4 le combinazioni **25/30**, nodi identici: con arco-consistenza + backjumping l'MRV semplice basta. **Default: mrv, linked off** (configurabili). |
| 2026-11-06 | 5 | Rossi (3): con "?AB" e incrocio "??E", l'ordine statico (frequenza della X iniziale nel dizionario) preferisce XAB anche se lascia 1 parola all'incrocio contro 5 di ZAB; candidati che svuotano un incrocio non scartati; opzione. Verde: `Bitset::count_and_below`, i primi K candidati ricevono il conteggio reale delle parole compatibili per ogni incrocio aperto, scartati se zero, riordinati per minimo dentro la fascia di punteggio. Opzione `--lookahead K`. |
| 2026-11-06 | 5 | Benchmark first 5 s: K=0 **25/30** · K=8 22/30 · K=32 22/30 (13x21/40 4→1: costo aggiunto senza benefici con l'arco-consistenza). Quality 3 s: K=0 24/30 · K=32 22/30, media 73,8 uguale. **Default: 0 (spento)**, configurabile. |
| 2026-11-06 | 6 | Rossi (3 + 1 integrazione): slot impossibile segnalato, lettere forzate dedotte dalla propagazione ("?AB" × "??E" con solo XAE → X e A forzate), schema facile senza forzature; `xfill --analyze`. Verde: `Solver::analyze()` → `Analysis { feasible, impossible, candidates, cell_letters, forced }` (riusa l'arco-consistenza); `xfill --analyze` stampa fattibilità, slot impossibili, lettere forzate e griglia del numero di lettere possibili per casella, senza cercare. |
| 2026-11-06 | 7 | Rossi (4 + 1 integrazione): completezza (su 7 configurazioni piccole, verificate per forza bruta sui sottoinsiemi di orbite, la ricerca deve trovare uno schema se e solo se esiste), limite di parole da 2 lettere, limite inferiore sulle nere necessarie, `GenMode`; opzioni di `library`. Verde: `GenMode::Search` = DFS sulle orbite di simmetria ordinate per lunghezza delle sequenze (con rumore), memo degli insiemi di nere visitati, potatura esatta sulle regole monotone (2×2, adiacenza, caselle isolate, connettività) e sul limite inferiore `min_blacks_needed`, budget di nodi per tentativo (default 20.000; 0 = esaustivo). `library --gen random\|search`, `--max-two-letter PCT`, scarto prima del riempimento degli schemi che `analyze` dimostra impossibili, scarto dei duplicati. La suite del benchmark resta generata col cammino casuale (confrontabilità). |
| 2026-11-06 | 7 | `library` (40 schemi, 2 s ciascuno, Db.csv): 13x13/24 random 22/40 (migliore 75,3) · search **27/40** (75,9); 15x15/32 random 7/40 (72,8) · search **14/40** (74,9); 13x21/40 random **14/40** (72,1) · search 12/40 (**75,0**). Totale **53 vs 43**, migliore sempre più alto. **Default: search.** Lo scarto per infattibilità non ha trovato casi qui (costo trascurabile, mantenuto). |
| 2026-11-06 | 8 | Rossi (3): con punteggi che premiano ARTE+PARTE l'opzione deve vietare la coppia; correttezza del backjumping col filtro; opzione. Verde: indice k-gram → profondità delle parole piazzate (parole bloccate: vincolo fisso), filtro dei candidati, profondità responsabili aggiunte al conflitto del nodo. **Verifica con mutante**: togliendo l'aggiunta al conflitto il test casuale non se ne accorgeva; aggiunto un caso costruito (regione R1 con XAB/XYZ, 2×2 indipendente che si riempie solo con AB) che il mutante fa fallire. Opzione `--avoid-substrings K`. |
| 2026-11-06 | 8 | Benchmark first 5 s: K=0 25/30 · K=4 23/30 · K=5 24/30 (vincolo in più, costo atteso). **Default: spento** (scelta stilistica dell'autore). |
| 2026-11-06 | 9 | Rossi (2 core + 1 integrazione): flag di annullamento cooperativo, `solve_portfolio` (somma nodi/riavvii, migliore per media, miglior alternativa pulita tra tutti i thread, validazione delle parole bloccate, in modalità primo riempimento il vincitore ferma gli altri), `xfill` con un thread per CPU di default. Verde: `SolveConfig::cancel` controllato nei cicli di riavvio e ogni 16 nodi; `solve_portfolio` nel core; `xfill` lo usa (rimosso il codice duplicato; prima in modalità `--first` aspettava il thread più lento). `library` resta parallela sugli schemi (un thread per schema). |
| 2026-11-06 | 10 | Rossi (3): riparazione delle parole sotto la soglia "deboli" con la configurazione italiana (prima solo sotto la soglia minima: mai, coi nostri punteggi); fase di riparazione da tentare anche sul riempimento "pulito" (con i default coincide col migliore e prima veniva saltata — verificato con mutante); due deboli indipendenti con limite 1 (prima: rinuncia). Verde: zona riempita prima senza parole deboli, poi alla soglia minima; tutte insieme se entro `repair_max_weak`, poi **greedy una alla volta** dalla più debole, accettando solo miglioramenti, con budget di tempo diviso tra i passi. Benchmark: `--repair-max-weak` (nuova opzione) senza effetto; diagnosi: 147 zone su 147 dimostrate impossibili all'istante — in schemi fitti parola debole + incroci è bloccata dalle parole attorno. **Zona di raggio 2** nei passi greedy (test di regressione: raggio 1 non può togliere BEN, raggio 2 sì). Il benchmark ora riporta il riempimento pubblicato (traccia "pulita") e la colonna "deboli". |
| 2026-11-06 | 10 | Benchmark quality 3 s: off → **24/30, media 73,7, deboli 3,0** · on (4) → **24/30, media 73,8, deboli 2,7** (13x13/24 3,3→2,7; 15x15/32 8,7→7,7; 13x21/40 12,0→11,3), costo ~0,05 s. **Default: 4 (attiva).** |
| 2026-11-06 | 11 | Esperimento: non rivedere con l'arco-consistenza gli slot con più di T candidati (per ridurre il costo per nodo) → T=5000 24/30, T=1000 24/30, T=300 23/30 contro 25/30 senza limite: scartato, nessuna opzione aggiunta. |
| 2026-11-06 | 11 | Rosso: le opzioni del solver valevano solo in `bench` (`xfill` le rifiutava come argomenti sconosciuti). Verde: `SOLVER_FLAGS` nel core; `xfill` e `library` applicano `SolveConfig::with_args` e stampano la configurazione. Test d'integrazione delle opzioni di `xfill` (`--min-score`, `--seed`, `--tiers`, `--first`, lock verticale, errori). README del motore riscritto (algoritmi, opzioni, `bench`, `--analyze`). |
| 2026-11-06 | 11 | Benchmark finale e copertura: vedi "Risultato finale". |
