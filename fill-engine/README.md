# xfill — crossword fill engine

A fast, quality-aware crossword fill engine in Rust. Given a grid skeleton (the
black-square pattern) and a scored wordlist, it produces a complete, *clean*
fill of dense NYT-density grids (15×15, ~30–40 blocks, ~60–78 interlocking
answers) — preferring high-quality words, not just any valid fill.

It pairs a classical CSP search (bitset domains, dynamic MRV variable ordering,
LCV value ordering, random restarts, quality tiering, best-of) with random-grid
**generation and selection**: most random grids fill ugly or not at all, so the
practical path is to generate many and keep the ones that fill cleanly.

See [docs/rust-fill-engine-design.md](docs/rust-fill-engine-design.md) for the
full design rationale.

## Layout

```
fill-engine/                 (under the xword-pipeline monorepo)
  crates/
    xfill-core/        library: bitset, wordlist, grid, generator, solver
    xfill-cli/         four binaries: xfill, screen, library, theme
  data/xwordlist.dict  scored wordlist (Crossword Nexus, MIT; WORD;score lines)
  data/LICENSE-wordlist.txt  the wordlist's MIT license + attribution
  templates/           example valid 15×15 grids (g30_*, g34_*, g38_*)
  docs/                design doc
```

`library`/`theme` write to the shared `../out/libraries/` by default (the
monorepo's `out/`); pass `--out` to override.

## Build

```bash
cargo build --release
```

Binaries land in `target/release/`: `xfill`, `screen`, `library`, `theme`.

> **Run from the `fill-engine/` directory.** The default `--wordlist` path is
> `data/xwordlist.dict` and the default `--out` is `../out/libraries/…`, both
> resolved relative to the current directory. From elsewhere, pass absolute
> `--wordlist` / `--out`.

You can also run via cargo, e.g. `cargo run --release --bin screen -- --count 20`.

## Grid template format

A plain-text grid, one row per line:

- `#` — black square (block)
- `.` — empty white cell to be filled
- `A`–`Z` — a pre-placed letter (theme entry / rebus seed); the solver fills around it

Spaces within a row are ignored; blank lines are skipped. Runs shorter than 3
cells are not treated as entries. Example: [templates/g38_2.txt](templates/g38_2.txt).

---

## `xfill` — fill one grid

Fills a single template and prints the result, the lowest-scoring entries, and
stats (nodes, restarts, mean/min answer score).

```bash
./target/release/xfill templates/g38_2.txt --time 5 --boxed
./target/release/xfill templates/g30_0.txt --time 10 --workers 10
```

| Flag | Default | Description |
|------|---------|-------------|
| `<template>` | (required) | Path to the grid template file |
| `--wordlist PATH` | `data/xwordlist.dict` | Scored wordlist (`WORD;score` per line) |
| `--min-score N` | `40` | Drop wordlist entries scoring below N |
| `--time SECS` | `30` | Wall-clock budget |
| `--seed N` | `0` | RNG seed (change for different fills) |
| `--tiers a,b,…` | `50,40` | Quality floors. Feasibility-first: secures a fill at the *lowest* floor, then polishes toward the higher (cleaner) floors via best-of |
| `--first` | off | Return the first solution found (skip best-of quality polishing) — fastest |
| `--boxed` | off | Render as a box-drawn grid instead of bare letters |
| `--workers N` | `1` | Parallel best-of: race N independent searches (distinct seeds), keep the best fill. Useful for hard or themed (pre-placed) grids |
| `--lock R,C,DIR,ANSWER` | — | Lock a theme / pre-placed answer (repeatable). See below |

Exit code is `0` if solved, `1` otherwise.

### Theme / pre-placed answers (`--lock`)

For themed puzzles, lock specific answers into place and let the engine fill
around them. `--lock` takes `ROW,COL,DIR,ANSWER`, where `DIR` is `A` (across) or
`D` (down) and `ROW`/`COL` are the 0-indexed start cell of the entry. Repeat the
flag for multiple theme answers. The answer **need not be in the wordlist** —
theme phrases are excluded from the search and only constrain their crossings;
if a locked answer *is* in the wordlist it won't be reused elsewhere.

```bash
# Lock a 15-letter theme answer into the down entry starting at row 0, col 7:
./target/release/xfill templates/library_g0.txt \
    --lock "0,7,D,WAITINGFORGODOT" --workers 10 --boxed
```

Quality stats (mean / iffy) are reported over the *searched* entries only —
the locked theme answers are a given, not a measure of fill quality. Letters can
also be pre-placed directly in the template (`A`–`Z` cells), but `--lock` is
preferred for whole theme answers since it handles out-of-wordlist phrases and
duplicate avoidance.

---

## `screen` — explore grid fillability/quality

Generates many random valid grids, fills each under a short budget (in
parallel), and prints the quality distribution plus the best grid found. Use
this to understand how a block count behaves and to eyeball sample fills.

```bash
./target/release/screen --blocks 36 --count 40 --time 2.5 --seed 7
./target/release/screen --blocks 32 --count 60 --time 3        # harder, themeless density
./target/release/screen --size 5 --count 40 --time 1           # 5×5 minis
```

| Flag | Default | Description |
|------|---------|-------------|
| `--wordlist PATH` | `data/xwordlist.dict` | Scored wordlist |
| `--size N` | `15` | Grid dimension (e.g. `5` or `10` for minis) |
| `--blocks N` | ~16% of area | Target black squares per grid (lower = harder); default scales with size |
| `--count N` | `50` | Number of random grids to generate and fill |
| `--time SECS` | `2.0` | Per-grid fill budget |
| `--seed N` | `1` | RNG seed (changes which grids are generated) |
| `--keep-mean F` | `68.0` | "Keeper" threshold for the summary count |
| `--workers N` | # CPU cores | Worker threads (grids filled in parallel) |

Progress prints to stderr; the report to stdout (append `2>/dev/null` to hide
progress). Wall time ≈ `count × time / workers` + ~0.1s load.

---

## `library` — build a curated grid library

The production tool: generate candidates, fill in parallel, keep only **clean**
grids (high mean score, few/no weak entries), deduplicate, rank by quality, and
emit a JSON library with full metadata — including numbered answers ready for a
clue-writing pipeline.

```bash
./target/release/library --blocks 36 --candidates 200 --time 2 \
    --keep-mean 78 --max-iffy 0 --top 20   # → ../out/libraries/grid-library.json
./target/release/library --size 5 --candidates 60 --keep-mean 60 --top 5 \
    --out ../out/libraries/mini5-library.json   # 5×5 mini library
```

| Flag | Default | Description |
|------|---------|-------------|
| `--wordlist PATH` | `data/xwordlist.dict` | Scored wordlist |
| `--size N` | `15` | Grid dimension (e.g. `5` or `10` for minis) |
| `--blocks N` | ~16% of area | Target black squares per grid; default scales with size (15→36, 10→16, 5→4) |
| `--candidates N` | `200` | Random grids to generate and try |
| `--time SECS` | `2.0` | Per-grid fill budget |
| `--keep-mean F` | `72.0` | Keep only fills with mean answer score ≥ this |
| `--max-iffy N` | `3` | Keep only fills with ≤ N entries scoring below 50 |
| `--weak-bar N` | `70` | Entries scoring below this count as *weak* (the gluey tail an editor counts); keepers sort fewest-weak-first. `0` = off |
| `--max-weak N` | none | Keep only fills with ≤ N weak entries |
| `--min-words N` | auto | Reject candidate shapes with fewer answers before filling — more words fill clean far more reliably. Auto = calibrated per size (16 at 7×7, 28 at 9×9, 72 at 15×15; none below 7×7); `0` = off |
| `--per-shape N` | `4` | Keep at most N fills of any one shape; the best fills of each shape survive and exact-duplicate fills are dropped. Small symmetric grids have very few shapes (five at 5×5), so this is where their variety comes from. `0` = no cap |
| `--top N` | `25` | Keep the best N grids (fewest weak, then mean) after dedup |
| `--bank PATH` / `--bank-fraction F` | `data/tpl-bank-<size>.txt` / auto | Draw this fraction of candidates from the template bank of clean-proven shapes (auto = 0.7 at 15×15, 0 below) |
| `--seed N` | `1` | RNG seed |
| `--workers N` | # CPU cores | Worker threads |
| `--out PATH` | `../out/libraries/grid-library.json` | Output JSON path |

### Output schema

Both `library` (themeless) and `theme` (themed) emit this identical schema:

```json
{
  "wordlist": "data/xwordlist.dict",
  "target_blocks": 36,
  "themed": false,
  "themes": [],
  "count": 10,
  "grids": [
    {
      "id": 0,
      "blocks": 36,
      "themed": false,
      "mean_score": 79.53,
      "min_score": 50,
      "iffy": 0,
      "template": ["OFOLD#ATLAS#SAT", "..."],
      "fill":     ["OFOLD#ATLAS#SAT", "..."],
      "entries": [
        {"num": 1, "dir": "A", "row": 0, "col": 0, "len": 5, "answer": "OFOLD", "score": 60, "theme": false},
        {"num": 2, "dir": "D", "row": 0, "col": 1, "len": 15, "answer": "FINEGRAINEDSAND", "score": 65, "theme": false}
      ]
    }
  ]
}
```

`entries` carries standard crossword clue numbers, direction (`A`/`D`),
position, length, the answer, its quality score, and a `theme` flag — the exact
input a clue-writing stage needs. For themed libraries, `themed` is `true`, the
top-level `themes` lists the theme answers, and each theme entry has
`"theme": true`. `mean_score`/`iffy` are computed over the non-theme entries.

---

## `theme` — generate themed puzzles

Given theme answers, auto-places them as symmetric across entries, generates
valid themed grids, locks the answers in, and fills around them in parallel,
keeping the cleanest fill.

```bash
./target/release/theme \
    --theme HOMERUN --theme HATTRICK --theme SLAMDUNK \
    --blocks 44 --candidates 150 --time 5
```

| Flag | Default | Description |
|------|---------|-------------|
| `--theme ANSWER` | (≥1 required) | A theme answer (A–Z), repeatable. 1–4 themes supported. **7–11 letters fill best**; a 13–15 letter answer (and its full-width mirror slot) crosses most of the grid and is much harder to fill. Never exactly 12 |
| `--blocks N` | `40` | Target black squares (themed grids want more — 42–44 — to ease the fill) |
| `--candidates N` | `100` | Themed grids to generate and try |
| `--time SECS` | `3.0` | Per-grid fill budget |
| `--seed N` | `1` | RNG seed |
| `--keep-mean F` | `68.0` | Keep grids whose non-theme mean score ≥ this |
| `--max-iffy N` | `18` | Keep grids with ≤ N non-theme entries scoring below 50 |
| `--top N` | `10` | Keep the best N themed grids after dedup |
| `--out PATH` | `../out/libraries/theme-library.json` | Output JSON path (same schema as `library`) |
| `--workers N` | # CPU cores | Worker threads |
| `--wordlist PATH` | `data/xwordlist.dict` | Scored wordlist |

It prints the best themed fill (boxed) and writes a JSON library identical in
shape to `library`'s, with two additions: a top-level `"themes": [...]` /
`"themed": true`, and each theme answer's entry flagged `"theme": true` — so the
clue-writing stage can treat theme answers specially (e.g. a shared gimmick or a
revealer).

**Placement.** Theme answers are auto-placed centered on spread-out rows; each
theme's 180° mirror becomes an equal-length non-theme slot in the opposite half,
so the grid stays symmetric and balanced for any mix of theme lengths. (A length
of exactly 12 can't be placed in a 15-wide grid as a single across with legal
edge gaps — pick another length.) Quality stats are over the *non-theme* entries.

**Expect lower fill rates than themeless.** Three theme entries plus their three
equal-length mirror slots make themed grids heavily interlocked, so only a few
percent of candidates fill cleanly — screen many (parallelism makes this cheap)
and raise `--blocks` toward 44 if fills come out weak.

---

## Scoring notes

Answer quality is measured on the wordlist's 1–100 scale (Crossword Nexus
convention: 50 = good/normal, 40 = questionable, lower = junk). An "iffy" entry
scores below 50. Clean NYT-grade fills run mean ~75–80 with few or no iffy
entries; the engine reaches these by *selecting* grids that admit them, not by
forcing a single hard grid.

## Third-party data

The bundled wordlist `data/xwordlist.dict` is the
[Crossword Nexus Collaborative Word List](https://github.com/Crossword-Nexus/collaborative-word-list),
redistributed under the **MIT License** — free to use, modify, distribute, and
sell. The license requires the copyright/permission notice to travel with any
substantial copy, so it is reproduced verbatim in
[data/LICENSE-wordlist.txt](data/LICENSE-wordlist.txt). Keep that file alongside
the wordlist whenever you distribute it.

## Blocklist

`data/blocklist.txt` (next to the wordlist) lists words the engine will **never**
place, regardless of score — the only way to keep out junk that's mis-scored high
(e.g. `TOONIEBAR;100`). One word per line; `#` comments; case/spaces/punctuation
ignored. Auto-loaded by `Wordlist::load`; prints `blocklist: excluding N word(s)`.

## Tests

```bash
cargo test
```
