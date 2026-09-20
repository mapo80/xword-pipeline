# Changelog

## [0.1.0]

### Added
- **Best fills per shape, several per shape.** Screeners now rank kept fills
  before deduplicating, drop exact-duplicate fills, and keep up to `--per-shape`
  (default 4) fills of any one template. Small symmetric grids have tiny shape
  spaces (five 5×5 templates in total, ~23 at 7×7), so the old template-level
  dedup collapsed every 5×5 run to five grids and kept whichever fill finished
  first rather than the best. Same run, 5×5: 5 → 16 keepers.
- **Word floor at midi sizes.** `gen::word_floor` now covers 7×7–12×12
  (16/22/28/34/40/50 words), calibrated the same way as the 15×15 floor, and
  applies to seeded minis too: 9×9 shapes with 30 words were 95% weak-free vs
  0% at 26; the gluey seeded 7×7s were all 14-word shapes. With both changes
  at publication gates: 7×7 keepers 12–13 → 20, all weak-free, sub-80 entries
  per grid 0.5–0.85 → 0.15–0.25; 9×9 runs ~35% faster (denser keepers hit the
  early stop sooner) with fewer sub-80 entries.
- **Template bank.** The `tpl_bank` bin probes random word-floor-passing
  15×15s and persists the clean-proven minority (kept local); generation
  draws a per-preset fraction from it ("Bank %", Publication 70) — ~1.9×
  publication keepers and a fully weak-free grid in nearly every banked run.
- **Weak-entry objective.** Entries under a weak bar (default 70) are counted
  per fill, the quality ladder gains a 70 tier, clean-fill selection and
  keeper ordering prefer fewest-weak-then-mean, and screeners can gate on it
  (`--max-weak` / per-preset "Max weak"). 9×9 top-20 keepers: ~2.4 weak
  entries → 0.0; 15×15 best-per-run: 16–18 → 0–8, incl. the first fully
  weak-free full-size grid.
- **Word-count floor at template generation (full-size themeless).** Candidate
  15×15 templates with fewer answers than a floor are rejected before filling.
- **Post-solve region repair (rip-and-refill).** When the search phases end
  with an almost-clean best fill — 1–4 searched entries below the clean
  floor — and no better clean alternative, the solver now locks every other
  entry in place, rips the weak entries plus their crossings, and re-solves
  just that region with clean-floor words on a 15% time slice.
- **Dup-retry: ban-and-refill instead of discard.** When a gate-passing fill
  hits the root-duplicate gate, the screeners now ban the more disposable
  member of the pair (lower score first, shorter on ties; locked seed/theme
  answers are never banned — `Solver::ban_answer`) and re-solve the candidate
  at half budget, up to two bans, keeping the refill if it passes the same
  gates and dup check.
- **Quality-ladder fill: clean-fill tracking + preference.** The solver's
  floor ladder now climbs past 50 (tiers 40/50/55/60 in every caller). After
  the feasibility baseline, the floors at/above the "clean" bar
  (`SolveConfig::clean_floor`, default 55) get the first half of the
  remaining time, and the best fill with NO entry below the bar is returned
  alongside the best-by-mean fill (`SolveResult::clean`).
- **Per-candidate block-count jitter (up-only).** When `--blocks` is auto,
  each candidate template draws its count from `target ..= target + jitter`
  (`gen::jittered_blocks`; +2 on full grids, +1 on minis/midis,
  `--block-jitter` overrides, pinned counts stay exact). Blockier grids fill
  cleaner and pass quality gates more often;
- **`--tier easy|medium|hard|expert`** on the clue writer — the canonical
  player-facing difficulty vocabulary, alongside the finer-grained `--day`
  (which additionally reaches Tuesday and Thursday/"Tricky"). The single
  tier→day mapping now lives in one place (`TIER_TO_DAY` in `styleGuide.ts`);
  `run-pipeline.sh` and the docs mirror it.
- **Scoped QA review (`qa --scope grid|clue|full`).** Multi-tier runs now
  review the grid once (fill, duplicate answers, Naticks, theme answers —
  identical across tiers → `${name}.qa.grid.json`) and the clues per tier
  (accuracy/difficulty/style → `${name}.qa.<tier>.json`), instead of a full
  review per tier. Cuts the redundant grid analysis from N passes to 1
  (≈⅓ off QA for 3 tiers) and stops the same fill/dupe finding repeating in
  every tier report. Single-tier runs are unchanged (one `full` review). The
  three scopes share one cached system prompt.
- **Expert clue tier** — clue writing now spans the full Easy → Expert range.
- **Duplicate-answer detection at two layers.** The Rust fill gate rejects any
  grid whose answers share a root — equal stems (`TEN`/`TENTH`, `EVEN`/`UNEVENLY`),
  one answer contained in another (`HOME`/`HOMERUN`), or a prominent shared
  embedded word (`BALL` in `BASEBALL`/`SEVEBALLESTEROS`) — so duplicates never
  reach the clue/QA steps. A deterministic clue checker then catches a grid answer
  appearing inside *another* entry's clue and auto-revises only the offending
  clues. Eliminates the most common high-severity QA finding; covered by tests in
  CI.
- **Interactive wizard** for `run-pipeline.sh` — prompts for mode, difficulty, and
  options instead of requiring flags.
- **Batch puzzle generation** via `generate-batch.sh` (many puzzles, varied
  sizes/days, in one command).
- **Detailed per-step pipeline timing** so you can see where wall-clock goes
  (fill vs. clue vs. QA).
- **Supplemental wordlist** support to broaden the fill engine's vocabulary.

### Changed
- **Size-aware difficulty defaults.** A bare themeless mini (≤7) now clues at
  Monday and a midi (8–12) at Wednesday; full-size themeless keeps the Saturday
  default (themed unchanged at Wednesday). The interactive wizard's suggested
  day follows the chosen size, and `run-pipeline.sh` prints a note when a ≤7
  mini is paired with a late-week `--day`.
- **Single-tier output filenames use the friendly-word token** (`…clued.expert.json`
  instead of `…clued.saturday.json`), matching multi-tier `…clued.<tier>.json`
  so both modes share one vocabulary. Explicit `--out` is unaffected.
- **Post-solve explanations default to Haiku 4.5** to cut cost — they're short and
  don't need a frontier model (override with `--explain-model`).
- **Themed generation is far more reliable.** Theme placement is now randomized
  across candidates (so their fills aren't correlated and fail together), per-mode
  defaults (gates / time / block count) are tuned separately for themed vs.
  themeless, the candidate screener early-stops once enough grids are kept, and a
  solver forward-check bug was corrected.
- **README:** added a link to the in-browser player at
  [wordfuzz.com/test](https://wordfuzz.com/test) — play puzzles you generate;
  files stay client-side.

### Fixed
- **Day names no longer leak as difficulty labels.** Export notes derive the
  friendly word from the day when a puzzle carries no explicit `difficulty`, so
  a `.puz`/`.ipuz` note never reads "Difficulty: Saturday". (The player has the
  matching fix: a badge shows "Medium", never "Wednesday".)
- **QA judged minis against full-size day expectations.** The editorial reviewer
  now receives the same per-day rubric the clue writer wrote to plus a
  size-class rubric (mini ≤7 / midi 8–12 / full 13+), and every prompt (clue,
  QA, revise) states the grid's size — so a "Wednesday 5×5" is graded as a
  Wednesday-worded mini instead of a mid-week 15×15. Friday/Saturday guidance
  no longer asserts themeless-ness when the puzzle is themed.
- **Themed-generation deadlock.** Short or dense theme sets used to produce *zero*
  valid templates: a 2-cell vertical run trapped above a theme's bounding blocks
  could never be rescued one cell at a time. A seed-repair pass now blocks out
  doomed 1–2 cell runs (mirrored, to a fixpoint) so themed grids actually fill.
- **QA across multiple difficulty tiers** — the editorial pass now reviews each
  tier correctly instead of mishandling multi-tier puzzles.
