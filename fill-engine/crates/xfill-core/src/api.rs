//! Library API of the engine: one call that generates a filled, numbered and
//! clued crossword from a request, and a validator for finished grids. The
//! JSON form of [`GenerateRequest`], [`GenerateResult`], [`ValidateInput`]
//! and [`ValidationReport`] is the contract of the C interface.
//!
//! Sizes are always given as `rows` and `cols` (the engine's order).

use crate::clues::ClueDb;
use crate::gen_it::{generate_it, GenMode, ItGenConfig, Symmetry};
use crate::grid::Puzzle;
use crate::library::{build_lib_grid, mark_theme, select_keepers, LibGrid};
use crate::solver::{Optimize, SolveConfig, Solver, SOLVER_FLAGS, SOLVER_SWITCHES};
use crate::util::Rng;
use crate::wordlist::{Wordlist, MAX_LEN};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Engine version reported in every result.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Cooperative cancellation: set it to `true` and the running generation
/// stops at its next check (the search checks every 16 nodes).
pub type CancelFlag = Arc<AtomicBool>;

/// A fresh, unset cancellation flag.
pub fn cancel_flag() -> CancelFlag {
    Arc::new(AtomicBool::new(false))
}

/// The fill wordlist and the clue database, loaded once and shared
/// (read-only) by any number of generations.
pub struct Dictionary {
    pub words: Wordlist,
    pub clues: ClueDb,
}

impl Dictionary {
    /// Load a `WORD,clue` CSV (UTF-8 or Latin-1, optional BOM). Errors when
    /// no usable word is found.
    pub fn from_csv_bytes(bytes: &[u8]) -> Result<Dictionary, String> {
        let (words, clues) = crate::clues::dictionary_from_csv(bytes, 0, &HashSet::new(), &[]);
        if words.word_count() == 0 {
            return Err("no usable WORD,clue line in the CSV".into());
        }
        Ok(Dictionary { words, clues })
    }

    /// Words in the fill list.
    pub fn word_count(&self) -> usize {
        self.words.word_count()
    }

    /// Approximate heap memory of the words, their masks and the clues.
    pub fn heap_bytes(&self) -> usize {
        self.words.heap_bytes() + self.clues.heap_bytes()
    }
}

/// Block symmetry of the generated pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SymmetryKind {
    #[default]
    Rot,
    Mirror,
    None,
}

/// Pattern generator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GeneratorKind {
    #[default]
    Search,
    Random,
}

/// What to generate. Every field has a default, so `{}` is a valid request
/// (a 13×13 with ~15% blocks).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GenerateRequest {
    /// Grid rows (height).
    pub rows: usize,
    /// Grid columns (width).
    pub cols: usize,
    /// Black squares (±1 with symmetry); default ~15% of the cells.
    pub blocks: Option<usize>,
    pub symmetry: SymmetryKind,
    pub generator: GeneratorKind,
    /// Seed: same seed and request give the same result (with a node
    /// budget), whatever the number of threads.
    pub seed: u64,
    /// Worker threads (0 = one per CPU).
    pub threads: usize,
    /// Patterns to generate and try at most.
    pub candidates: usize,
    /// Filled grids to compare before choosing (the search stops early once
    /// this many pass the quality filters); default [`DEFAULT_POOL`], or
    /// [`DEFAULT_THEME_POOL`] with theme words.
    pub pool: Option<usize>,
    /// Search nodes per pattern (0 = time only, not reproducible).
    pub node_budget: u64,
    /// Time per pattern in seconds: the budget when `node_budget` is 0,
    /// otherwise a safety ceiling.
    pub time_per_grid_s: f64,
    /// Theme words (any case, accents folded); those in the dictionary are
    /// placed first and counted in the result.
    pub theme_words: Vec<String>,
    /// Weight of a theme word against the fill quality (see
    /// [`crate::SolveConfig::theme_weight`]).
    pub theme_weight: f64,
    /// With theme words: fill first the entries that can still take one
    /// (default [`DEFAULT_THEME_SLOTS`]).
    pub theme_slots: Option<bool>,
    /// Candidate ordering: none, weak, strong, max, reliability, theme
    /// (default: theme with theme words, weak without).
    pub optimize: Option<String>,
    /// Longest entry (default: the longer side, capped by the dictionary).
    pub max_len: Option<usize>,
    /// Allow black squares to touch.
    pub allow_adjacent: bool,
    /// At most this percentage of 2-letter entries.
    pub max_two_letter_pct: Option<u8>,
    /// At least N entries of length L, as [L, N] pairs.
    pub min_len_counts: Vec<[usize; 2]>,
    /// Entry lengths that must appear (with multiplicity).
    pub require_lengths: Vec<usize>,
    /// At most this many entries spanning a whole row or column.
    pub max_full_words: Option<usize>,
    /// Largest all-white rectangle (cells).
    pub max_white_rect: Option<usize>,
    /// Start from a lattice of blacks at (odd row, odd column).
    pub lattice: bool,
    /// Discard fills with a lower mean score.
    pub min_mean: f64,
    /// Discard fills with more weak entries.
    pub max_weak: Option<usize>,
    /// Entries scoring below this are weak.
    pub weak_bar: u8,
    /// Pick clues from the dictionary.
    pub clues: bool,
    /// Further solver options in command-line form, e.g.
    /// `["--propagation", "arc"]`.
    pub solver_args: Vec<String>,
}

impl Default for GenerateRequest {
    fn default() -> Self {
        GenerateRequest {
            rows: 13,
            cols: 13,
            blocks: None,
            symmetry: SymmetryKind::Rot,
            generator: GeneratorKind::Search,
            seed: 1,
            threads: 0,
            candidates: DEFAULT_CANDIDATES,
            pool: None,
            node_budget: DEFAULT_NODE_BUDGET,
            time_per_grid_s: DEFAULT_TIME_CEILING_S,
            theme_words: Vec::new(),
            theme_weight: DEFAULT_THEME_WEIGHT,
            theme_slots: None,
            optimize: None,
            max_len: None,
            allow_adjacent: true,
            max_two_letter_pct: None,
            min_len_counts: Vec::new(),
            require_lengths: Vec::new(),
            max_full_words: None,
            max_white_rect: None,
            lattice: false,
            min_mean: 0.0,
            max_weak: None,
            weak_bar: crate::SolveConfig::default().weak_bar,
            clues: true,
            solver_args: Vec::new(),
        }
    }
}

/// Default patterns to try at most.
pub const DEFAULT_CANDIDATES: usize = 40;
/// Default pool without / with theme words (see the benchmark in the plan).
pub const DEFAULT_POOL: usize = 2;
pub const DEFAULT_THEME_POOL: usize = 4;
/// Default work per pattern (nodes + candidates tried; see the benchmark in
/// the plan).
pub const DEFAULT_NODE_BUDGET: u64 = 20_000;
/// Default safety ceiling per pattern, seconds.
pub const DEFAULT_TIME_CEILING_S: f64 = 20.0;
/// Theme slots by default when the request has theme words.
pub const DEFAULT_THEME_SLOTS: bool = true;
/// Default weight of a theme word.
pub const DEFAULT_THEME_WEIGHT: f64 = 2.0;

/// Entry direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Across,
    Down,
}

/// One numbered entry of the grid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EntryOut {
    pub number: u32,
    pub direction: Direction,
    pub row: usize,
    pub col: usize,
    pub length: usize,
    pub answer: String,
    pub clue: Option<String>,
    /// Word score in the dictionary (0–100).
    pub score: u8,
    pub is_theme: bool,
}

impl Default for EntryOut {
    fn default() -> Self {
        EntryOut {
            number: 0,
            direction: Direction::Across,
            row: 0,
            col: 0,
            length: 0,
            answer: String::new(),
            clue: None,
            score: 0,
            is_theme: false,
        }
    }
}

/// Outcome of a generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// A grid was generated.
    Ok,
    /// No pattern could be generated or filled with these parameters.
    NoGrid,
    /// Stopped by the cancellation flag.
    Cancelled,
    /// The request is invalid (see `error`).
    InvalidRequest,
}

/// Result of [`generate`]. Grid fields are empty unless `status` is `ok`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GenerateResult {
    pub status: Status,
    pub error: Option<String>,
    pub engine_version: String,
    pub seed: u64,
    pub rows: usize,
    pub cols: usize,
    /// Number of black squares.
    pub blocks: usize,
    /// Black squares as [row, col].
    pub black_cells: Vec<[usize; 2]>,
    /// Pattern rows: `#` black, `.` white.
    pub pattern: Vec<String>,
    /// Solution rows: letters, `#` black.
    pub grid: Vec<String>,
    /// Entries by number, across before down.
    pub entries: Vec<EntryOut>,
    /// Mean word score of the fill.
    pub score: f64,
    pub min_score: u8,
    /// Answers scoring below the weak bar.
    pub weak_words: Vec<String>,
    /// Number of theme answers in the grid.
    pub theme_count: usize,
    /// The theme answers, in entry order.
    pub theme_words: Vec<String>,
    /// Theme words requested / found in the dictionary.
    pub theme_requested: usize,
    pub theme_in_dictionary: usize,
    /// Patterns generated, and patterns filled before choosing.
    pub patterns: usize,
    pub attempts: usize,
    /// Patterns filled successfully among the attempts.
    pub filled: usize,
    /// Search nodes over all attempts.
    pub nodes: u64,
    pub duration_ms: u64,
}

impl GenerateResult {
    fn empty(status: Status, seed: u64, rows: usize, cols: usize) -> GenerateResult {
        GenerateResult {
            status,
            error: None,
            engine_version: ENGINE_VERSION.to_string(),
            seed,
            rows,
            cols,
            blocks: 0,
            black_cells: Vec::new(),
            pattern: Vec::new(),
            grid: Vec::new(),
            entries: Vec::new(),
            score: 0.0,
            min_score: 0,
            weak_words: Vec::new(),
            theme_count: 0,
            theme_words: Vec::new(),
            theme_requested: 0,
            theme_in_dictionary: 0,
            patterns: 0,
            attempts: 0,
            filled: 0,
            nodes: 0,
            duration_ms: 0,
        }
    }
}

/// Largest side accepted in a request.
pub const MAX_SIDE: usize = 32;

/// A resolved generation plan: pattern generator, solver configuration and
/// selection rules. Built from a [`GenerateRequest`] by [`plan_for`], or
/// directly (the `library` command builds its own from the command line).
#[derive(Clone)]
pub struct Plan {
    pub gen: ItGenConfig,
    /// Solver configuration per pattern (budget, theme, ordering…); the
    /// per-pattern seed and the cancellation flag are set by the run.
    pub solve: SolveConfig,
    /// Patterns to generate at most.
    pub candidates: usize,
    /// Generator tries per pattern (each a budgeted search).
    pub gen_tries: usize,
    /// Quality filters on a filled pattern.
    pub min_mean: f64,
    pub max_weak: usize,
    /// Stop once this many filled patterns pass the filters (counted in
    /// pattern order, so the outcome doesn't depend on thread timing).
    pub stop_after: usize,
    pub threads: usize,
    pub seed: u64,
}

/// Generator tries per pattern in the API (the `library` command keeps 50).
pub const API_GEN_TRIES: usize = 5;

/// Generator calls in a row without a pattern after which, if none was
/// found at all, the constraints are taken as impossible.
pub const GIVE_UP_MISSES: usize = 3;

/// Generated patterns with their fill seeds.
pub struct Patterns {
    /// (pattern rows joined by newlines, seed of its fill)
    pub jobs: Vec<(String, u64)>,
    /// Generator calls made.
    pub tries: usize,
    /// Patterns the dictionary provably can't fill (skipped).
    pub infeasible: usize,
    pub cancelled: bool,
}

/// Generate up to `plan.candidates` distinct patterns whose entry lengths
/// all exist in `wl` and that aren't provably unfillable. Deterministic for
/// a given plan.
pub fn generate_patterns(wl: &Wordlist, plan: &Plan, cancel: &CancelFlag) -> Patterns {
    let gen = ItGenConfig {
        cancel: Some(cancel.clone()),
        ..plan.gen.clone()
    };
    let mut rng = Rng::new(plan.seed);
    let mut jobs: Vec<(String, u64)> = Vec::with_capacity(plan.candidates);
    let mut tries = 0usize;
    let mut seen = HashSet::new();
    let mut infeasible = 0usize;
    let max_tries = plan.candidates.saturating_mul(20).max(200);
    let mut misses = 0usize;
    while jobs.len() < plan.candidates && tries < max_tries {
        if cancel.load(Ordering::Relaxed) {
            return Patterns {
                jobs,
                tries,
                infeasible,
                cancelled: true,
            };
        }
        tries += 1;
        let jseed = rng.next_u64();
        let Some(tpl) = generate_it(&gen, &mut rng, plan.gen_tries) else {
            // each call already made `gen_tries` tries: a few failures in a row
            // with nothing found mean no pattern fits the constraints
            misses += 1;
            if misses >= GIVE_UP_MISSES && jobs.is_empty() {
                break;
            }
            continue;
        };
        misses = 0;
        let p = Puzzle::from_template_with_rebus(&tpl, &wl.rebus);
        // every entry length must exist in the word list
        if p.orphan_cells() > 0 || p.entries.iter().any(|e| wl.len_data(e.len).n == 0) {
            continue;
        }
        if !seen.insert(tpl.clone()) {
            continue;
        }
        // don't spend fill time on patterns the dictionary provably can't fill
        if !Solver::new(&p, wl).analyze().feasible {
            infeasible += 1;
            continue;
        }
        jobs.push((tpl, jseed));
    }
    Patterns {
        jobs,
        tries,
        infeasible,
        cancelled: false,
    }
}

/// Outcome of [`fill_patterns`].
pub struct FillRun {
    /// Filled patterns that passed the filters, in pattern order.
    pub kept: Vec<LibGrid>,
    /// Patterns counted (a prefix of the jobs: later ones are ignored once
    /// enough grids are kept).
    pub done: usize,
    /// Patterns filled among them.
    pub filled: usize,
    /// Search nodes over the counted patterns.
    pub nodes: u64,
    pub elapsed_s: f64,
    pub cancelled: bool,
}

/// Fill `jobs` on `plan.threads` threads. A worker takes the next pattern
/// in order; once the patterns finished from the start (without gaps) hold
/// `plan.stop_after` kept grids, the rest are skipped and any later results
/// dropped — so with a node budget the outcome is the same for any number
/// of threads. `progress(done, total)` is called after each fill.
pub fn fill_patterns(
    wl: &Wordlist,
    plan: &Plan,
    jobs: &[(String, u64)],
    cancel: &CancelFlag,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> FillRun {
    struct Slot {
        grid: Option<LibGrid>,
        filled: bool,
        nodes: u64,
    }
    struct State {
        slots: Vec<Option<Slot>>,
        /// patterns 0..prefix are finished
        prefix: usize,
        kept: usize,
        /// count only patterns before this one
        cut: usize,
    }
    let t0 = Instant::now();
    let n = jobs.len();
    let state = Mutex::new(State {
        slots: (0..n).map(|_| None).collect(),
        prefix: 0,
        kept: 0,
        cut: n,
    });
    let next = AtomicUsize::new(0);
    let finished = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let threads = plan.threads.clamp(1, n.max(1));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                if stop.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
                    break;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    break;
                }
                let (tpl, jseed) = &jobs[i];
                let p = Puzzle::from_template_with_rebus(tpl, &wl.rebus);
                let cfg = SolveConfig {
                    seed: *jseed,
                    cancel: Some(cancel.clone()),
                    ..plan.solve.clone()
                };
                let r = Solver::new(&p, wl).solve(&cfg);
                if cancel.load(Ordering::Relaxed) {
                    break; // the result may be cut short: don't count it
                }
                let grid = build_lib_grid(&p, &r);
                let filled = grid.is_some();
                let grid = grid
                    .filter(|g| g.mean >= plan.min_mean && g.weak <= plan.max_weak)
                    .map(|mut g| {
                        mark_theme(&mut g, wl, &plan.solve.theme);
                        g
                    });
                let mut st = state.lock().expect("fill state");
                st.slots[i] = Some(Slot {
                    grid,
                    filled,
                    nodes: r.nodes,
                });
                while st.prefix < st.cut {
                    let k = st.prefix;
                    let Some(slot) = &st.slots[k] else { break };
                    let kept = slot.grid.is_some() as usize;
                    st.prefix += 1;
                    st.kept += kept;
                    if st.kept >= plan.stop_after {
                        st.cut = st.prefix;
                        stop.store(true, Ordering::Relaxed);
                    }
                }
                drop(st);
                progress(finished.fetch_add(1, Ordering::Relaxed) + 1, n);
            });
        }
    });
    let st = state.into_inner().expect("fill state");
    let cancelled = cancel.load(Ordering::Relaxed) && st.prefix < st.cut;
    let counted: Vec<Slot> = st
        .slots
        .into_iter()
        .take(st.cut)
        .take_while(Option::is_some)
        .flatten()
        .collect();
    FillRun {
        done: counted.len(),
        filled: counted.iter().filter(|s| s.filled).count(),
        nodes: counted.iter().map(|s| s.nodes).sum(),
        kept: counted.into_iter().filter_map(|s| s.grid).collect(),
        elapsed_s: t0.elapsed().as_secs_f64(),
        cancelled,
    }
}

/// Pick a clue for every entry of `grids` from `clues` (seeded: the same
/// seed picks the same clues).
pub fn assign_clues(grids: &mut [LibGrid], clues: &ClueDb, seed: u64) {
    let mut crng = Rng::new(seed ^ 0xC1E5);
    for g in grids {
        for e in &mut g.entries {
            e.clue = clues.pick_clue(&e.answer, |n| crng.below(n));
        }
    }
}

/// Resolve a request against the dictionary: (plan, theme words found in
/// the dictionary).
pub fn plan_for(wl: &Wordlist, req: &GenerateRequest) -> Result<(Plan, usize), String> {
    let side = 2..=MAX_SIDE;
    if !side.contains(&req.rows) || !side.contains(&req.cols) {
        return Err(format!(
            "rows and cols must be between 2 and {MAX_SIDE} (got {} × {})",
            req.rows, req.cols
        ));
    }
    let cells = req.rows * req.cols;
    let blocks = req.blocks.unwrap_or(cells * 15 / 100);
    if blocks > cells / 2 {
        return Err(format!("too many blocks: {blocks} of {cells} cells"));
    }
    if req.candidates == 0 || req.pool == Some(0) {
        return Err("candidates and pool must be at least 1".into());
    }
    if !(req.time_per_grid_s > 0.0 && req.time_per_grid_s.is_finite()) {
        return Err("time_per_grid_s must be a positive number of seconds".into());
    }
    if !(req.theme_weight >= 0.0 && req.theme_weight.is_finite()) {
        return Err("theme_weight must be zero or positive".into());
    }
    let mut i = 0;
    while i < req.solver_args.len() {
        let a = req.solver_args[i].as_str();
        if SOLVER_FLAGS.contains(&a) {
            i += 2;
        } else if SOLVER_SWITCHES.contains(&a) {
            i += 1;
        } else {
            return Err(format!("unknown solver option '{a}'"));
        }
    }
    let mut solve = SolveConfig::default().with_args(&req.solver_args)?;
    let theme = crate::solver::theme_ids(wl, &req.theme_words);
    let found = theme.len();
    solve.optimize = match &req.optimize {
        Some(o) => Optimize::parse(o).ok_or(format!(
            "bad optimize '{o}' (use none, weak, strong, max, reliability or theme)"
        ))?,
        None if !theme.is_empty() => Optimize::Theme,
        None => solve.optimize,
    };
    solve.theme_slots = !theme.is_empty() && req.theme_slots.unwrap_or(DEFAULT_THEME_SLOTS);
    solve.theme = Arc::new(theme);
    solve.theme_weight = req.theme_weight;
    solve.time_limit_s = req.time_per_grid_s;
    solve.node_limit = req.node_budget;
    solve.weak_bar = req.weak_bar;
    let max_len = req
        .max_len
        .unwrap_or(req.rows.max(req.cols))
        .min(MAX_LEN)
        .min(wl.longest());
    let gen = ItGenConfig {
        symmetry: match req.symmetry {
            SymmetryKind::Rot => Symmetry::Rotational,
            SymmetryKind::Mirror => Symmetry::Mirror,
            SymmetryKind::None => Symmetry::None,
        },
        max_len,
        allow_adjacent: req.allow_adjacent,
        mode: match req.generator {
            GeneratorKind::Search => GenMode::Search,
            GeneratorKind::Random => GenMode::Random,
        },
        max_two_letter_pct: req.max_two_letter_pct,
        min_len_counts: req.min_len_counts.iter().map(|&[l, n]| (l, n)).collect(),
        max_full_words: req.max_full_words,
        require_lengths: req.require_lengths.clone(),
        max_white_rect: req.max_white_rect,
        lattice: req.lattice,
        ..ItGenConfig::new(req.rows, req.cols, blocks)
    };
    let threads = match req.threads {
        0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
        t => t,
    };
    Ok((
        Plan {
            gen,
            solve,
            candidates: req.candidates,
            gen_tries: API_GEN_TRIES,
            min_mean: req.min_mean,
            max_weak: req.max_weak.unwrap_or(usize::MAX),
            stop_after: req.pool.unwrap_or(if found > 0 {
                DEFAULT_THEME_POOL
            } else {
                DEFAULT_POOL
            }),
            threads,
            seed: req.seed,
        },
        found,
    ))
}

/// Generate one crossword: patterns, parallel fill, choice of the best
/// (theme words weighed in), numbering and clues.
pub fn generate(dict: &Dictionary, req: &GenerateRequest, cancel: &CancelFlag) -> GenerateResult {
    let t0 = Instant::now();
    let mut out = GenerateResult::empty(Status::Ok, req.seed, req.rows, req.cols);
    out.theme_requested = req.theme_words.len();
    let finish = |mut out: GenerateResult, status: Status, error: Option<String>| {
        out.status = status;
        out.error = error;
        out.duration_ms = t0.elapsed().as_millis() as u64;
        out
    };
    let (plan, found) = match plan_for(&dict.words, req) {
        Ok(x) => x,
        Err(e) => return finish(out, Status::InvalidRequest, Some(e)),
    };
    out.theme_in_dictionary = found;
    if cancel.load(Ordering::Relaxed) {
        return finish(out, Status::Cancelled, None);
    }
    let patterns = generate_patterns(&dict.words, &plan, cancel);
    out.patterns = patterns.jobs.len();
    if patterns.cancelled {
        return finish(out, Status::Cancelled, None);
    }
    if patterns.jobs.is_empty() {
        let msg = format!(
            "no valid {}×{} pattern with {} blocks after {} tries",
            req.rows, req.cols, plan.gen.blacks, patterns.tries
        );
        return finish(out, Status::NoGrid, Some(msg));
    }
    let run = fill_patterns(&dict.words, &plan, &patterns.jobs, cancel, &|_, _| {});
    out.attempts = run.done;
    out.filled = run.filled;
    out.nodes = run.nodes;
    if run.cancelled {
        return finish(out, Status::Cancelled, None);
    }
    let mut kept = select_keepers(run.kept, 0, plan.solve.theme_weight);
    kept.truncate(1);
    if req.clues {
        assign_clues(&mut kept, &dict.clues, req.seed);
        // never publish a clue that spells its answer (the dictionary picks
        // one when it has nothing else): the entry stays without a clue
        for g in &mut kept {
            for e in &mut g.entries {
                if e.clue
                    .as_deref()
                    .is_some_and(|c| clue_contains_answer(c, &e.answer))
                {
                    e.clue = None;
                }
            }
        }
    }
    let Some(g) = kept.pop() else {
        let msg = format!("none of {} patterns filled within the budget", run.done);
        return finish(out, Status::NoGrid, Some(msg));
    };
    out.blocks = g.blocks;
    out.black_cells = g
        .template
        .iter()
        .enumerate()
        .flat_map(|(r, row)| {
            row.char_indices()
                .filter(|&(_, ch)| ch == '#')
                .map(move |(c, _)| [r, c])
        })
        .collect();
    out.pattern = g.template.clone();
    out.grid = g.fill.clone();
    out.score = (g.mean * 100.0).round() / 100.0;
    out.min_score = g.min;
    out.entries = g
        .entries
        .iter()
        .map(|e| EntryOut {
            number: e.num,
            direction: if e.dir == 'A' {
                Direction::Across
            } else {
                Direction::Down
            },
            row: e.row,
            col: e.col,
            length: e.len,
            answer: e.answer.clone(),
            clue: e.clue.clone(),
            score: e.score,
            is_theme: e.is_theme,
        })
        .collect();
    out.weak_words = out
        .entries
        .iter()
        .filter(|e| plan.solve.weak_bar > 0 && e.score < plan.solve.weak_bar)
        .map(|e| e.answer.clone())
        .collect();
    out.theme_words = out
        .entries
        .iter()
        .filter(|e| e.is_theme)
        .map(|e| e.answer.clone())
        .collect();
    out.theme_count = out.theme_words.len();
    finish(out, Status::Ok, None)
}

/// A finished grid to check (a [`GenerateResult`] can be passed as is).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ValidateInput {
    pub rows: usize,
    pub cols: usize,
    /// Solution rows: letters, `#` black.
    pub grid: Vec<String>,
    pub entries: Vec<EntryOut>,
}

/// One problem found by [`validate`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    /// Machine-readable code (see [`validate`]).
    pub code: String,
    pub message: String,
    pub number: Option<u32>,
    pub direction: Option<Direction>,
    pub row: Option<usize>,
    pub col: Option<usize>,
}

/// Result of [`validate`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValidationReport {
    /// No errors (warnings allowed).
    pub valid: bool,
    pub errors: Vec<Issue>,
    pub warnings: Vec<Issue>,
    /// Entries expected from the black pattern.
    pub slots: usize,
}

/// The clue spells the answer (answers of 3+ letters; letters compared
/// after folding accents and dropping punctuation).
pub fn clue_contains_answer(clue: &str, answer: &str) -> bool {
    let answer = answer.to_uppercase();
    answer.chars().count() >= 3 && crate::clues::normalize_letters_lossy(clue).contains(&answer)
}

fn issue(code: &str, message: String, e: Option<&EntryOut>) -> Issue {
    Issue {
        code: code.to_string(),
        message,
        number: e.map(|e| e.number),
        direction: e.map(|e| e.direction),
        row: e.map(|e| e.row),
        col: e.map(|e| e.col),
    }
}

/// Check a finished grid. Error codes: `bad_size` (grid not rows × cols),
/// `bad_cell` (neither a letter nor `#`), `bad_coordinates` (entry outside
/// the grid), `unknown_slot` (no entry of the pattern starts there in that
/// direction), `duplicate_entry`, `missing_entry`, `bad_number` (not the
/// standard numbering), `bad_length`, `answer_mismatch` (answer differs from
/// the grid letters), `crossing_conflict` (two answers disagree on a shared
/// cell), `repeated_word` (same answer twice, 3+ letters),
/// `clue_contains_answer` (3+ letters). Warnings: `repeated_word` (2
/// letters), `missing_clue`.
pub fn validate(input: &ValidateInput) -> ValidationReport {
    let mut errors: Vec<Issue> = Vec::new();
    let mut warnings: Vec<Issue> = Vec::new();
    let (rows, cols) = (input.rows, input.cols);
    let cells: Vec<Vec<char>> = input.grid.iter().map(|r| r.chars().collect()).collect();
    if rows == 0 || cols == 0 || cells.len() != rows || cells.iter().any(|r| r.len() != cols) {
        errors.push(issue(
            "bad_size",
            format!("the grid must have {rows} rows of {cols} cells"),
            None,
        ));
        return ValidationReport {
            valid: false,
            errors,
            warnings,
            slots: 0,
        };
    }
    for (r, row) in cells.iter().enumerate() {
        for (c, &ch) in row.iter().enumerate() {
            if ch != '#' && !ch.is_ascii_uppercase() {
                let mut i = issue("bad_cell", format!("cell ({r}, {c}) holds '{ch}'"), None);
                i.row = Some(r);
                i.col = Some(c);
                errors.push(i);
            }
        }
    }
    // slots of the pattern with their standard numbers
    let white = |r: usize, c: usize| cells[r][c] != '#';
    let mut slots: HashMap<(usize, usize, Direction), (u32, usize)> = HashMap::new();
    let mut order: Vec<(usize, usize, Direction)> = Vec::new();
    let mut number = 0u32;
    for r in 0..rows {
        for c in 0..cols {
            if !white(r, c) {
                continue;
            }
            let run = |dr: usize, dc: usize| {
                let mut k = 0;
                while r + dr * k < rows && c + dc * k < cols && white(r + dr * k, c + dc * k) {
                    k += 1;
                }
                k
            };
            let across = (c == 0 || !white(r, c - 1)) && run(0, 1) >= 2;
            let down = (r == 0 || !white(r - 1, c)) && run(1, 0) >= 2;
            if across || down {
                number += 1;
            }
            if across {
                slots.insert((r, c, Direction::Across), (number, run(0, 1)));
                order.push((r, c, Direction::Across));
            }
            if down {
                slots.insert((r, c, Direction::Down), (number, run(1, 0)));
                order.push((r, c, Direction::Down));
            }
        }
    }
    let mut seen: HashSet<(usize, usize, Direction)> = HashSet::new();
    let mut placed: HashMap<(usize, usize), char> = HashMap::new();
    let mut conflicts: HashSet<(usize, usize)> = HashSet::new();
    let mut answers: HashMap<String, Vec<&EntryOut>> = HashMap::new();
    for e in &input.entries {
        let key = (e.row, e.col, e.direction);
        if e.row >= rows || e.col >= cols {
            errors.push(issue(
                "bad_coordinates",
                format!("entry {} starts outside the grid", e.number),
                Some(e),
            ));
            continue;
        }
        let Some(&(num, len)) = slots.get(&key) else {
            errors.push(issue(
                "unknown_slot",
                format!(
                    "no entry starts at ({}, {}) {:?}",
                    e.row, e.col, e.direction
                ),
                Some(e),
            ));
            continue;
        };
        if !seen.insert(key) {
            errors.push(issue(
                "duplicate_entry",
                format!("entry {} {:?} given twice", e.number, e.direction),
                Some(e),
            ));
            continue;
        }
        if e.number != num {
            errors.push(issue(
                "bad_number",
                format!("entry numbered {} should be {num}", e.number),
                Some(e),
            ));
        }
        if e.length != len {
            errors.push(issue(
                "bad_length",
                format!(
                    "entry {} has length {} instead of {len}",
                    e.number, e.length
                ),
                Some(e),
            ));
        }
        let answer: Vec<char> = e.answer.to_uppercase().chars().collect();
        let (dr, dc) = match e.direction {
            Direction::Across => (0, 1),
            Direction::Down => (1, 0),
        };
        let in_grid: String = (0..len)
            .map(|k| cells[e.row + dr * k][e.col + dc * k])
            .collect();
        if answer.iter().collect::<String>() != in_grid {
            errors.push(issue(
                "answer_mismatch",
                format!(
                    "entry {} reads {in_grid} in the grid, not {}",
                    e.number, e.answer
                ),
                Some(e),
            ));
        }
        for (k, &ch) in answer.iter().enumerate().take(len) {
            let cell = (e.row + dr * k, e.col + dc * k);
            match placed.get(&cell) {
                Some(&other) if other != ch && conflicts.insert(cell) => {
                    errors.push(issue(
                        "crossing_conflict",
                        format!(
                            "cell ({}, {}) is {other} in one answer and {ch} in entry {}",
                            cell.0, cell.1, e.number
                        ),
                        Some(e),
                    ));
                }
                Some(_) => {}
                None => {
                    placed.insert(cell, ch);
                }
            }
        }
        match e.clue.as_deref().map(str::trim) {
            None | Some("") => warnings.push(issue(
                "missing_clue",
                format!("entry {} {:?} has no clue", e.number, e.direction),
                Some(e),
            )),
            Some(clue) => {
                if clue_contains_answer(clue, &e.answer) {
                    errors.push(issue(
                        "clue_contains_answer",
                        format!("the clue of entry {} contains the answer", e.number),
                        Some(e),
                    ));
                }
            }
        }
        answers.entry(e.answer.to_uppercase()).or_default().push(e);
    }
    for key in &order {
        if !seen.contains(key) {
            let (num, _) = slots[key];
            let mut i = issue(
                "missing_entry",
                format!("no answer for entry {num} {:?}", key.2),
                None,
            );
            (i.number, i.direction, i.row, i.col) =
                (Some(num), Some(key.2), Some(key.0), Some(key.1));
            errors.push(i);
        }
    }
    let mut repeated: Vec<(&String, &Vec<&EntryOut>)> =
        answers.iter().filter(|(_, v)| v.len() > 1).collect();
    repeated.sort_by_key(|(a, _)| a.as_str());
    for (answer, list) in repeated {
        let i = issue(
            "repeated_word",
            format!("{answer} appears {} times", list.len()),
            Some(list[1]),
        );
        if answer.chars().count() >= 3 {
            errors.push(i);
        } else {
            warnings.push(i);
        }
    }
    ValidationReport {
        valid: errors.is_empty(),
        errors,
        warnings,
        slots: order.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    /// Every 2..=5-letter string over eight letters, each with a clue, a few
    /// "themed" words carrying more clues (higher scores).
    fn csv() -> String {
        let alpha = ['A', 'E', 'I', 'O', 'R', 'S', 'T', 'N'];
        let mut words: Vec<String> = alpha.iter().map(|c| c.to_string()).collect();
        let mut out = String::new();
        for len in 2..=5 {
            words = words
                .iter()
                .flat_map(|w| alpha.iter().map(move |c| format!("{w}{c}")))
                .collect();
            for (i, w) in words.iter().enumerate() {
                // keep the list small enough for fast tests: every 3rd word
                // of length 5
                if len == 5 && i % 3 != 0 {
                    continue;
                }
                // clues must not spell the answer
                out.push_str(&format!("{w},Voce {len}-{i}\n"));
                if i % 4 == 0 {
                    out.push_str(&format!("{w},Altra voce {len}-{i}\n"));
                }
            }
        }
        out
    }

    fn dict() -> Dictionary {
        Dictionary::from_csv_bytes(csv().as_bytes()).expect("test dictionary")
    }

    fn small(rows: usize, cols: usize) -> GenerateRequest {
        GenerateRequest {
            rows,
            cols,
            candidates: 12,
            pool: Some(3),
            node_budget: 4_000,
            ..Default::default()
        }
    }

    #[test]
    fn dictionary_loads_csv_and_reports_memory() {
        let d = dict();
        assert!(d.word_count() > 1000);
        assert!(d.heap_bytes() > d.word_count());
        assert_eq!(d.clues.get("AA").len(), 2);
        assert!(Dictionary::from_csv_bytes(b"").is_err());
        assert!(Dictionary::from_csv_bytes(b"# solo commenti\n,\n").is_err());
    }

    #[test]
    fn request_json_defaults_and_unknown_fields() {
        let r: GenerateRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(r, GenerateRequest::default());
        let r: GenerateRequest = serde_json::from_str(
            r#"{"rows":12,"cols":22,"symmetry":"mirror","theme_words":["roma"]}"#,
        )
        .unwrap();
        assert_eq!((r.rows, r.cols, r.symmetry), (12, 22, SymmetryKind::Mirror));
        assert!(serde_json::from_str::<GenerateRequest>(r#"{"size":"13x13"}"#).is_err());
        assert!(serde_json::from_str::<GenerateRequest>(r#"{"symmetry":"diag"}"#).is_err());
    }

    /// A rectangular request gives `rows` rows of `cols` letters, numbered
    /// entries consistent with the grid, and a valid report.
    #[test]
    fn generates_a_rectangular_grid_rows_by_cols() {
        let d = dict();
        // words of at most 5 letters: every row of 9 needs blocks
        let req = GenerateRequest {
            blocks: Some(12),
            ..small(6, 9)
        };
        let r = generate(&d, &req, &cancel_flag());
        assert_eq!(r.status, Status::Ok, "{:?}", r.error);
        assert_eq!((r.rows, r.cols), (6, 9));
        assert_eq!(r.grid.len(), 6);
        assert!(r.grid.iter().all(|row| row.chars().count() == 9));
        assert_eq!(r.pattern.len(), 6);
        assert_eq!(r.blocks, r.black_cells.len());
        for &[row, col] in &r.black_cells {
            assert_eq!(r.grid[row].as_bytes()[col], b'#');
        }
        assert!(!r.entries.is_empty());
        assert!(r.entries.iter().all(|e| e.clue.is_some()), "clues picked");
        assert!(r.attempts >= 1 && r.filled >= 1 && r.patterns >= r.attempts);
        assert!(r.score > 0.0);
        assert_eq!(r.engine_version, ENGINE_VERSION);
        let report = validate(&ValidateInput {
            rows: r.rows,
            cols: r.cols,
            grid: r.grid.clone(),
            entries: r.entries.clone(),
        });
        assert!(report.valid, "{:?}", report.errors);
        assert_eq!(report.slots, r.entries.len());
        // the result JSON is itself a valid validation input
        let json = serde_json::to_string(&r).unwrap();
        let back: ValidateInput = serde_json::from_str(&json).unwrap();
        assert!(validate(&back).valid);
    }

    /// Same seed and request ⇒ same grid, with one thread or several.
    #[test]
    fn same_seed_same_result_with_any_thread_count() {
        let d = dict();
        let req = |threads| GenerateRequest {
            threads,
            seed: 7,
            ..small(7, 7)
        };
        let strip = |mut r: GenerateResult| {
            r.duration_ms = 0;
            r
        };
        let a = strip(generate(&d, &req(1), &cancel_flag()));
        let b = strip(generate(&d, &req(4), &cancel_flag()));
        let c = strip(generate(&d, &req(4), &cancel_flag()));
        assert_eq!(a.status, Status::Ok);
        assert_eq!(a, b);
        assert_eq!(b, c);
        let other = generate(&d, &GenerateRequest { seed: 8, ..req(4) }, &cancel_flag());
        assert_ne!(a.grid, other.grid, "another seed, another grid");
    }

    /// Theme words present in the dictionary are counted and flagged.
    #[test]
    fn theme_words_are_counted_and_flagged() {
        let d = dict();
        let theme: Vec<String> = [
            "arte", "rete", "sera", "tana", "nero", "oste", "rosa", "zzzz",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let r = generate(
            &d,
            &GenerateRequest {
                theme_words: theme,
                ..small(7, 7)
            },
            &cancel_flag(),
        );
        assert_eq!(r.status, Status::Ok);
        assert_eq!((r.theme_requested, r.theme_in_dictionary), (8, 7));
        let flagged: Vec<String> = r
            .entries
            .iter()
            .filter(|e| e.is_theme)
            .map(|e| e.answer.clone())
            .collect();
        assert_eq!(r.theme_words, flagged);
        assert_eq!(r.theme_count, flagged.len());
        assert!(r.theme_count >= 1, "theme words placed first");
    }

    /// Cancelling stops a long generation quickly.
    #[test]
    fn cancellation_stops_quickly() {
        let d = dict();
        let flag = cancel_flag();
        let f2 = flag.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            f2.store(true, Ordering::Relaxed);
            Instant::now()
        });
        let req = GenerateRequest {
            node_budget: 0,
            time_per_grid_s: 30.0,
            candidates: 50,
            pool: Some(50),
            threads: 2,
            ..small(9, 9)
        };
        let r = generate(&d, &req, &flag);
        let done = Instant::now();
        let set = t.join().unwrap();
        assert_eq!(r.status, Status::Cancelled);
        assert!(r.grid.is_empty());
        assert!(
            done.duration_since(set) < Duration::from_millis(500),
            "stopped {:?} after the flag",
            done.duration_since(set)
        );
        // an already-set flag returns at once
        let r = generate(&d, &small(7, 7), &flag);
        assert_eq!(r.status, Status::Cancelled);
    }

    #[test]
    fn invalid_requests_and_impossible_patterns() {
        let d = dict();
        let bad = |req: GenerateRequest| {
            let r = generate(&d, &req, &cancel_flag());
            assert_eq!(r.status, Status::InvalidRequest, "{req:?}");
            assert!(r.error.is_some());
        };
        bad(GenerateRequest {
            rows: 1,
            ..small(5, 5)
        });
        bad(GenerateRequest {
            cols: 40,
            ..small(5, 5)
        });
        bad(GenerateRequest {
            optimize: Some("fast".into()),
            ..small(5, 5)
        });
        bad(GenerateRequest {
            solver_args: vec!["--propagation".into(), "x".into()],
            ..small(5, 5)
        });
        bad(GenerateRequest {
            blocks: Some(25),
            ..small(5, 5)
        });
        // words of at most 5 letters cannot fill a 9×9 without blocks
        let r = generate(
            &d,
            &GenerateRequest {
                blocks: Some(0),
                ..small(9, 9)
            },
            &cancel_flag(),
        );
        assert_eq!(r.status, Status::NoGrid);
        assert!(r.error.is_some());
    }

    fn sample() -> ValidateInput {
        // C A S A
        // O # E R
        // L I T E   (rows), columns: COL, A#I (no), ...
        let grid = vec!["CASA".to_string(), "O#ER".to_string(), "LITE".to_string()];
        let e = |number, direction, row, col, answer: &str| EntryOut {
            number,
            direction,
            row,
            col,
            length: answer.len(),
            answer: answer.into(),
            clue: Some(format!("Definizione {number}")),
            ..Default::default()
        };
        ValidateInput {
            rows: 3,
            cols: 4,
            grid,
            entries: vec![
                e(1, Direction::Across, 0, 0, "CASA"),
                e(1, Direction::Down, 0, 0, "COL"),
                e(2, Direction::Down, 0, 2, "SET"),
                e(3, Direction::Down, 0, 3, "ARE"),
                e(4, Direction::Across, 1, 2, "ER"),
                e(5, Direction::Across, 2, 0, "LITE"),
            ],
        }
    }

    fn codes(r: &ValidationReport) -> Vec<&str> {
        r.errors.iter().map(|i| i.code.as_str()).collect()
    }

    #[test]
    fn validate_accepts_a_correct_grid() {
        let r = validate(&sample());
        assert!(r.valid, "{:?}", r.errors);
        assert_eq!(r.slots, 6);
    }

    #[test]
    fn validate_reports_each_kind_of_error() {
        let mut v = sample();
        v.entries[2].number = 9;
        assert_eq!(codes(&validate(&v)), vec!["bad_number"]);

        let mut v = sample();
        v.entries[5].answer = "LIME".into();
        assert!(codes(&validate(&v)).contains(&"answer_mismatch"));

        let mut v = sample();
        v.entries[3].answer = "ARA".into(); // crosses LITE's E with A
        assert!(codes(&validate(&v)).contains(&"crossing_conflict"));

        let mut v = sample();
        v.entries.remove(4);
        assert_eq!(codes(&validate(&v)), vec!["missing_entry"]);

        let mut v = sample();
        v.entries.push(v.entries[0].clone());
        assert_eq!(codes(&validate(&v)), vec!["duplicate_entry"]);

        let mut v = sample();
        v.entries[1].row = 7;
        assert!(codes(&validate(&v)).contains(&"bad_coordinates"));

        let mut v = sample();
        v.entries[1].col = 1; // (0,1) down is not a slot (A then black)
        assert!(codes(&validate(&v)).contains(&"unknown_slot"));

        let mut v = sample();
        v.grid = vec!["CASA".into(), "LITE".into()];
        assert_eq!(codes(&validate(&v)), vec!["bad_size"]);

        let mut v = sample();
        v.grid[0] = "CA.A".into();
        assert!(codes(&validate(&v)).contains(&"bad_cell"));

        let mut v = sample();
        v.entries[0].clue = Some("La casa di tutti".into());
        assert_eq!(codes(&validate(&v)), vec!["clue_contains_answer"]);
    }

    #[test]
    fn validate_flags_repeated_words() {
        // CASA across twice in a 3×4 with a black middle row
        let e = |number, direction, row, col, answer: &str| EntryOut {
            number,
            direction,
            row,
            col,
            length: answer.len(),
            answer: answer.into(),
            clue: None,
            ..Default::default()
        };
        let v = ValidateInput {
            rows: 3,
            cols: 4,
            grid: vec!["CASA".into(), "####".into(), "CASA".into()],
            entries: vec![
                e(1, Direction::Across, 0, 0, "CASA"),
                e(2, Direction::Across, 2, 0, "CASA"),
            ],
        };
        let r = validate(&v);
        assert_eq!(codes(&r), vec!["repeated_word"]);
        assert!(r.warnings.iter().any(|w| w.code == "missing_clue"));
        // 2-letter repeats are only a warning
        let v = ValidateInput {
            rows: 3,
            cols: 2,
            grid: vec!["NO".into(), "##".into(), "NO".into()],
            entries: vec![
                e(1, Direction::Across, 0, 0, "NO"),
                e(2, Direction::Across, 2, 0, "NO"),
            ],
        };
        let r = validate(&v);
        assert!(r.valid);
        assert!(r.warnings.iter().any(|w| w.code == "repeated_word"));
    }

    /// A request with no possible pattern gives up quickly instead of
    /// spending every generator try on it.
    #[test]
    fn hopeless_patterns_give_up_quickly() {
        let short: String = csv()
            .lines()
            .filter(|l| l.split(',').next().is_some_and(|w| w.len() <= 4))
            .map(|l| format!("{l}\n"))
            .collect();
        let d = Dictionary::from_csv_bytes(short.as_bytes()).unwrap();
        let t = Instant::now();
        let r = generate(&d, &small(9, 9), &cancel_flag());
        assert_eq!(r.status, Status::NoGrid);
        // 0.9 s here, ~5 s on slow CI machines; before the fix: minutes
        assert!(t.elapsed() < Duration::from_secs(15), "{:?}", t.elapsed());
    }

    /// Theme slots follow the request, default on with theme words, never
    /// without them.
    #[test]
    fn theme_slots_in_the_plan() {
        let d = dict();
        let themed = GenerateRequest {
            theme_words: vec!["arte".into()],
            ..small(5, 5)
        };
        let (plan, _) = plan_for(&d.words, &themed).unwrap();
        assert_eq!(plan.solve.theme_slots, DEFAULT_THEME_SLOTS);
        let off = GenerateRequest {
            theme_slots: Some(false),
            ..themed.clone()
        };
        assert!(!plan_for(&d.words, &off).unwrap().0.solve.theme_slots);
        let on = GenerateRequest {
            theme_slots: Some(true),
            ..themed
        };
        assert!(plan_for(&d.words, &on).unwrap().0.solve.theme_slots);
        let plain = GenerateRequest {
            theme_slots: Some(true),
            ..small(5, 5)
        };
        assert!(!plan_for(&d.words, &plain).unwrap().0.solve.theme_slots);
        let r: GenerateRequest = serde_json::from_str(r#"{"theme_slots": false}"#).unwrap();
        assert_eq!(r.theme_slots, Some(false));
    }

    /// Without an explicit pool, a themed request compares more grids.
    #[test]
    fn default_pool_depends_on_the_theme() {
        let d = dict();
        let plain = GenerateRequest::default();
        assert_eq!(
            plan_for(&d.words, &plain).unwrap().0.stop_after,
            DEFAULT_POOL
        );
        let themed = GenerateRequest {
            theme_words: vec!["arte".into()],
            ..Default::default()
        };
        assert_eq!(
            plan_for(&d.words, &themed).unwrap().0.stop_after,
            DEFAULT_THEME_POOL
        );
        let explicit = GenerateRequest {
            pool: Some(7),
            ..themed
        };
        assert_eq!(plan_for(&d.words, &explicit).unwrap().0.stop_after, 7);
        assert_eq!(plain.candidates, DEFAULT_CANDIDATES);
    }

    /// A generated grid always passes validation: a clue that spells its
    /// answer is never published (the entry stays without a clue).
    #[test]
    fn generated_grids_never_carry_clues_containing_the_answer() {
        // every word of 3+ letters has only a clue that contains it
        let csv: String = csv()
            .lines()
            .map(|l| {
                let w = l.split(',').next().unwrap();
                if w.len() >= 3 {
                    format!("{w},La parola {w}\n")
                } else {
                    format!("{l}\n")
                }
            })
            .collect();
        let d = Dictionary::from_csv_bytes(csv.as_bytes()).unwrap();
        for seed in 1..=3 {
            let r = generate(
                &d,
                &GenerateRequest {
                    seed,
                    ..small(7, 7)
                },
                &cancel_flag(),
            );
            assert_eq!(r.status, Status::Ok);
            let v = validate(&ValidateInput {
                rows: r.rows,
                cols: r.cols,
                grid: r.grid.clone(),
                entries: r.entries.clone(),
            });
            assert!(v.valid, "seed {seed}: {:?}", v.errors);
            assert!(r
                .entries
                .iter()
                .filter(|e| e.length >= 3)
                .all(|e| e.clue.is_none()));
        }
    }
}
