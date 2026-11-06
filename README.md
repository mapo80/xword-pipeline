# xword-pipeline — versione italiana

Generatore di cruciverba all'italiana da riga di comando, in Rust.

Fork di [ekorbia/xword-pipeline](https://github.com/ekorbia/xword-pipeline)
(licenza MIT, vedi [LICENSE](LICENSE)). Del progetto originale è stato tenuto il
motore di riempimento (`fill-engine`); la parte TypeScript/Claude per le
definizioni in inglese e i generatori di schemi in stile americano sono stati
rimossi.

## Regole dello schema

- parole di almeno 2 lettere; una lettera isolata in una direzione è ammessa
  (non ha definizione) purché appartenga a una parola nell'altra direzione;
- caselle bianche tutte collegate, nessun blocco 2×2 di caselle nere;
- griglie rettangolari, simmetria opzionale (`none`, `rot`, `mirror`);
- nessuna parola ripetuta.

## Dizionario

Un CSV `PAROLA,definizione`, una definizione per riga (la stessa parola può
comparire più volte). Il punteggio di qualità di ogni parola è ricavato dal
numero di definizioni: più definizioni → parola più usata nei cruciverba
(1 → 50, 3 → 63, 10 → 78, massimo 90). Il percorso predefinito è
`../../dataset/Db.csv` (relativo a `fill-engine/`). Un file `blocklist.txt`
accanto al CSV esclude parole specifiche.

## Uso

```bash
cd fill-engine
cargo build --release

# schema generato: dimensione RIGHExCOLONNE + numero di caselle nere
./target/release/library --size 13x13 --blocks 26 --top 3 \
    --out ../out/libreria.json --pdf ../out/pdf

# schema dato: '#' nera, '.' bianca, lettere già inserite
./target/release/xfill schema.txt --boxed
```

Dettagli delle opzioni in [fill-engine/README.md](fill-engine/README.md).
