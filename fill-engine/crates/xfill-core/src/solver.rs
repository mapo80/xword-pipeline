//! M2 solver: bitset-domain DFS with dynamic MRV + score-ordered value
//! selection, wrapped in random restarts, quality tiering, and best-of search.
//!
//! Why restarts: pure DFS gets trapped in a doomed subtree near the root and
//! grinds millions of nodes without backtracking far enough to escape. Each
//! attempt runs under a node budget; on exhaustion we restart from scratch with
//! a fresh RNG, and value ordering is shuffled within equal-score bands so each
//! attempt explores a different region while still preferring high-score words.
//!
//! Quality tiering: an outer loop first tries to fill using only words scoring
//! \>= a high floor (a tier = an index cutoff, since words are score-sorted);
//! the floor is relaxed only if a tier can't solve in its time slice. Across all
//! restarts the best fill (highest mean answer score) is retained.

use crate::bitset::Bitset;
use crate::grid::{Dir, Puzzle};
use crate::util::Rng;
use crate::wordlist::Wordlist;
use std::time::Instant;

/// A theme / pre-placed answer: lock the entry starting at (row, col) in `dir`
/// to `answer`. The answer need NOT be in the wordlist — locked entries are
/// excluded from the search and only propagate their letters as constraints.
#[derive(Clone, Debug)]
pub struct Lock {
    pub row: usize,
    pub col: usize,
    pub dir: Dir,
    pub answer: String,
}

/// How assignments propagate to the rest of the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Propagation {
    /// Filter only the entries crossing the assigned one.
    Forward,
    /// Arc consistency: keep only words whose letter at every crossing is
    /// still possible for the crossing entry, transitively, to a fixed point.
    Arc,
}

impl Propagation {
    pub fn parse(s: &str) -> Option<Propagation> {
        match s {
            "forward" => Some(Propagation::Forward),
            "arc" => Some(Propagation::Arc),
            _ => None,
        }
    }
}

/// Which entry to fill next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotOrder {
    /// Fewest remaining candidates (ties: longer entry first).
    Mrv,
    /// Fewest remaining candidates per still-empty cell.
    PerCell,
}

impl SlotOrder {
    pub fn parse(s: &str) -> Option<SlotOrder> {
        match s {
            "mrv" => Some(SlotOrder::Mrv),
            "per-cell" => Some(SlotOrder::PerCell),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct SolveConfig {
    /// Constraint propagation after each assignment (see [`Propagation`]).
    pub propagation: Propagation,
    /// Conflict-directed backjumping: on failure, jump back to the deepest
    /// assignment involved in the conflict instead of the previous one.
    pub backjump: bool,
    /// Entry-selection heuristic (see [`SlotOrder`]).
    pub slot_order: SlotOrder,
    /// Prefer entries crossing the already-filled part of the grid.
    pub linked: bool,
    /// Value ordering: rescore the first K candidates by the real number of
    /// options they leave each crossing entry (0 = static letter frequency).
    pub lookahead: usize,
    /// Forbid two different answers sharing a substring of this many
    /// letters (0 = off).
    pub avoid_substrings: usize,
    /// Cooperative cancellation: when set, the search stops at the next
    /// check (used by [`solve_portfolio`] to stop the losing workers).
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub time_limit_s: f64,
    /// Quality floors tried high→low. A floor of 50 forbids "questionable"
    /// (<50) entries; relaxing to 40 permits glue.
    pub tiers: Vec<u8>,
    pub seed: u64,
    pub initial_budget: u64,
    pub max_budget: u64,
    /// Max candidates materialized per node (cap for band-shuffle cost).
    pub cand_cap: usize,
    /// Return the first solution found (skip best-of optimization).
    pub stop_on_first: bool,
    /// A fill whose every searched entry scores >= this is "clean"; the best
    /// clean fill (by mean) is tracked and returned alongside the overall
    /// best so callers can prefer it when it passes their gates. 0 disables.
    pub clean_floor: u8,
    /// Post-solve region repair: if the search phases end with no clean fill
    /// (or only a worse-than-best one) and the best fill has 1..=this many
    /// searched entries below `clean_floor`, rip those entries plus their
    /// crossings out, lock the rest of the grid, and re-solve just that
    /// region at the clean floor on a small budget. Beyond ~4 the region
    /// approaches a whole-grid re-solve (each weak word drags 3-5 crossings
    /// with it), which the clean hunt already attempted. 0 disables.
    pub repair_max_weak: usize,
    /// Entries scoring below this are WEAK — admissible but the tail an editor
    /// would rather avoid (with clue-count scores: words with a single clue).
    /// Fills track `weak_count`, and the clean track prefers FEWER weak
    /// entries before higher mean. 0 disables (count is always 0).
    pub weak_bar: u8,
}

/// Command-line options handled by [`SolveConfig::with_args`] (each takes a
/// value).
pub const SOLVER_FLAGS: &[&str] = &[
    "--propagation",
    "--backjump",
    "--slot-order",
    "--linked",
    "--lookahead",
    "--avoid-substrings",
    "--repair-max-weak",
];

impl SolveConfig {
    /// Apply the solver-variant options found in command-line `args`
    /// (`--propagation forward|arc`, `--backjump on|off`,
    /// `--slot-order mrv|per-cell`, `--linked on|off`, `--lookahead K`,
    /// `--avoid-substrings K`, `--repair-max-weak N`); other arguments are
    /// ignored.
    pub fn with_args(mut self, args: &[String]) -> Result<SolveConfig, String> {
        let mut i = 0;
        while i < args.len() {
            let value = || {
                args.get(i + 1)
                    .map(String::as_str)
                    .ok_or(format!("{} needs a value", args[i]))
            };
            match args[i].as_str() {
                "--propagation" => {
                    let v = value()?;
                    self.propagation = Propagation::parse(v)
                        .ok_or(format!("bad --propagation '{v}' (use forward or arc)"))?;
                    i += 1;
                }
                "--slot-order" => {
                    let v = value()?;
                    self.slot_order = SlotOrder::parse(v)
                        .ok_or(format!("bad --slot-order '{v}' (use mrv or per-cell)"))?;
                    i += 1;
                }
                "--linked" => {
                    self.linked = match value()? {
                        "on" => true,
                        "off" => false,
                        v => return Err(format!("bad --linked '{v}' (use on or off)")),
                    };
                    i += 1;
                }
                "--lookahead" => {
                    let v = value()?;
                    self.lookahead = v
                        .parse()
                        .map_err(|_| format!("bad --lookahead '{v}' (use a number, 0 = off)"))?;
                    i += 1;
                }
                "--repair-max-weak" => {
                    let v = value()?;
                    self.repair_max_weak = v.parse().map_err(|_| {
                        format!("bad --repair-max-weak '{v}' (use a number, 0 = off)")
                    })?;
                    i += 1;
                }
                "--avoid-substrings" => {
                    let v = value()?;
                    self.avoid_substrings = v.parse().map_err(|_| {
                        format!("bad --avoid-substrings '{v}' (use a length, 0 = off)")
                    })?;
                    i += 1;
                }
                "--backjump" => {
                    self.backjump = match value()? {
                        "on" => true,
                        "off" => false,
                        v => return Err(format!("bad --backjump '{v}' (use on or off)")),
                    };
                    i += 1;
                }
                _ => {}
            }
            i += 1;
        }
        Ok(self)
    }

    /// One-line description of the solver variant.
    pub fn describe(&self) -> String {
        let p = match self.propagation {
            Propagation::Forward => "forward",
            Propagation::Arc => "arc",
        };
        let on = |b: bool| if b { "on" } else { "off" };
        let so = match self.slot_order {
            SlotOrder::Mrv => "mrv",
            SlotOrder::PerCell => "per-cell",
        };
        format!(
            "propagation {p}, backjump {}, slot-order {so}, linked {}, lookahead {}, \
                          avoid-substrings {}, repair-max-weak {}",
            on(self.backjump),
            on(self.linked),
            self.lookahead,
            self.avoid_substrings,
            self.repair_max_weak
        )
    }
}

/// What a grid allows before any search (see [`Solver::analyze`]).
pub struct Analysis {
    /// No entry is impossible, even after arc-consistent propagation.
    pub feasible: bool,
    /// Entries left with no candidate (before propagation if any; else the
    /// entry where propagation ran dry).
    pub impossible: Vec<usize>,
    /// Per entry: candidates left after propagation.
    pub candidates: Vec<u32>,
    /// Per white cell (cell id): letters still possible, bit c = letter c.
    pub cell_letters: Vec<u32>,
    /// Empty cells with exactly one possible letter: (row, col, b'A'..=b'Z').
    pub forced: Vec<(usize, usize, u8)>,
}

/// Solve with `threads` independent searches (distinct seeds) in parallel and
/// keep the best fill by mean (merging the best clean alternative found by
/// any worker). With `stop_on_first`, the first worker to solve stops the
/// others. Errors on invalid locks.
pub fn solve_portfolio(
    p: &Puzzle,
    wl: &Wordlist,
    locks: &[Lock],
    cfg: &SolveConfig,
    threads: usize,
) -> Result<SolveResult, String> {
    Solver::with_locks(p, wl, locks)?;
    if threads <= 1 {
        return Ok(Solver::with_locks(p, wl, locks)?.solve(cfg));
    }
    let flag = cfg
        .cancel
        .clone()
        .unwrap_or_else(|| std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)));
    let results: Vec<SolveResult> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|k| {
                let mut cfg_k = cfg.clone();
                cfg_k.seed = cfg
                    .seed
                    .wrapping_add((k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                cfg_k.cancel = Some(flag.clone());
                let flag = flag.clone();
                s.spawn(move || {
                    let r = Solver::with_locks(p, wl, locks)
                        .expect("locks validated")
                        .solve(&cfg_k);
                    if cfg_k.stop_on_first && r.reason == "solved" {
                        flag.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    r
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("worker"))
            .collect()
    });
    let mut total_nodes = 0u64;
    let mut total_restarts = 0u64;
    let mut best: Option<SolveResult> = None;
    let mut best_clean: Option<SolvedFill> = None;
    for res in results {
        total_nodes += res.nodes;
        total_restarts += res.restarts;
        if let Some(c) = &res.clean {
            if best_clean
                .as_ref()
                .is_none_or(|b| Solver::better_clean(c, b))
            {
                best_clean = Some(c.clone());
            }
        }
        let take = match &best {
            None => true,
            Some(b) => {
                res.reason == "solved"
                    && b.mean_score
                        .is_none_or(|bm| res.mean_score.is_some_and(|m| m > bm))
            }
        };
        if take {
            best = Some(res);
        }
    }
    let mut r = best.expect("at least two workers");
    r.nodes = total_nodes;
    r.restarts = total_restarts;
    r.clean = best_clean;
    Ok(r)
}

/// Fraction of `time_limit_s` granted to the post-solve repair pass. Region
/// re-solves are tiny (a handful of entries, most letters pinned), so they
/// either succeed almost immediately or are infeasible.
const REPAIR_TIME_FRAC: f64 = 0.15;

impl Default for SolveConfig {
    fn default() -> Self {
        SolveConfig {
            // benchmark (first fill, 5 s): arc 23/30 vs forward 20/30
            propagation: Propagation::Arc,
            // benchmark (first fill, 5 s): on 25/30 vs off 23/30
            backjump: true,
            slot_order: SlotOrder::Mrv,
            linked: false,
            lookahead: 0,
            avoid_substrings: 0,
            cancel: None,
            time_limit_s: 30.0,
            // Clue-count scores: 1 clue → 50, 2 → 58, 3 → 63. Secure any fill
            // first, then hunt fills of better-known words.
            tiers: vec![50, 58, 63],
            seed: 0,
            initial_budget: 2_000,
            max_budget: 400_000,
            cand_cap: 400,
            stop_on_first: false,
            clean_floor: 50,
            repair_max_weak: 4,
            weak_bar: 58,
        }
    }
}

/// A complete fill with its quality stats. `letters` is per-cell (0..25);
/// `fill` lists (entry_id, answer, score) for searched entries followed by
/// locked theme answers. Stats cover the SEARCHED entries only.
#[derive(Clone)]
pub struct SolvedFill {
    pub letters: Vec<Option<u8>>,
    pub mean_score: f64,
    pub min_score: u8,
    /// Searched entries scoring below `SolveConfig::weak_bar` — the gluey
    /// tail an editor would count. 0 when the bar is disabled.
    pub weak_count: usize,
    pub fill: Vec<(usize, String, u8)>,
}

pub struct SolveResult {
    pub letters: Option<Vec<Option<u8>>>,
    pub nodes: u64,
    pub restarts: u64,
    pub elapsed_s: f64,
    pub reason: &'static str, // "solved" | "unsolved"
    pub mean_score: Option<f64>,
    pub min_score: Option<u8>,
    /// Sub-`weak_bar` entry count of the primary fill (see `SolvedFill`).
    pub weak_count: Option<usize>,
    pub fill: Option<Vec<(usize, String, u8)>>,
    /// Best fill with no searched entry below `cfg.clean_floor`, when one
    /// was found — "best" = fewest weak (sub-`weak_bar`) entries, then
    /// highest mean. May describe the same fill as the primary fields.
    /// Callers should prefer it whenever it passes their keep gates on its
    /// own — same yield, strictly fewer weak entries.
    pub clean: Option<SolvedFill>,
}

/// A domain snapshot taken before an entry's domain is changed.
struct Saved {
    entry: usize,
    domain: Bitset,
    count: u32,
    culprits: Bitset,
    masks: Vec<u32>,
    support: Vec<u32>,
}

struct TrailFrame {
    entry: usize,
    word: usize,
    len: usize,
    nbrs: Vec<Saved>,
    set_cells: Vec<usize>,
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Outcome {
    Solved,
    Exhausted,
    Budget,
    TimeLimit,
}

pub struct Solver<'a> {
    p: &'a Puzzle,
    wl: &'a Wordlist,
    base_domains: Vec<Bitset>, // domains after prefilled, reused each attempt
    domains: Vec<Bitset>,
    dom_count: Vec<u32>,
    cutoff_by_entry: Vec<usize>,
    /// per entry: (pos_in_entry, neighbor_entry, neighbor_pos) for each crossing
    crossings: Vec<Vec<(usize, usize, usize)>>,
    /// per entry: the other searchable entries of the same length (a word
    /// placed in one is removed from their domains — no duplicate answers)
    same_len: Vec<Vec<usize>>,
    assigned: Vec<Option<usize>>,
    cell_letter: Vec<Option<u8>>,
    used: Vec<Bitset>,
    n_assigned: usize,
    // Theme / pre-placed support:
    is_locked: Vec<bool>,
    base_cell_letter: Vec<Option<u8>>, // template prefill + locked answers
    locked_used: Vec<(usize, usize)>,  // (len, word_idx) of locks present in wordlist
    locked_out: Vec<(usize, String, u8)>, // (entry_id, answer, score) for output
    n_searchable: usize,               // non-locked entries to fill
    nodes: u64,
    attempt_budget: u64,
    deadline: Instant,
    floor: u8,
    cand_cap: usize,
    rng: Rng,
    propagation: Propagation,
    backjump: bool,
    slot_order: SlotOrder,
    linked: bool,
    lookahead: usize,
    avoid_substrings: usize,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// k-grams (k = avoid_substrings) of the placed answers → depths of the
    /// entries holding them (`STATIC` for locked answers)
    grams: std::collections::HashMap<u64, Vec<usize>>,
    /// depths whose k-grams rejected candidates at the current node
    gram_conflict: Bitset,
    /// per entry: serial of the last trail frame that saved its domain
    saved_in: Vec<u64>,
    serial: u64,
    /// per entry: search depths whose assignments reduced its domain
    culprits: Vec<Bitset>,
    /// conflict set of the last failure (depths involved)
    conflict: Bitset,
    /// per entry, per position: letters still possible (bit c = letter c)
    masks: Vec<Vec<u32>>,
    /// per entry: in-tier candidates with letter c at position p, at
    /// index p * 26 + c (keeps `masks` up to date incrementally)
    support: Vec<Vec<u32>>,
    /// did the current attempt cut any candidate list (cand_cap)?
    truncated: bool,
}

impl<'a> Solver<'a> {
    pub fn new(p: &'a Puzzle, wl: &'a Wordlist) -> Self {
        Self::with_locks(p, wl, &[]).expect("no locks cannot fail")
    }

    /// Construct a solver with theme / pre-placed answers locked into place.
    /// Returns an error if a lock doesn't resolve to an entry, the answer length
    /// mismatches, or two locks disagree on a shared cell.
    pub fn with_locks(p: &'a Puzzle, wl: &'a Wordlist, locks: &[Lock]) -> Result<Self, String> {
        let n = p.entries.len();
        let domains: Vec<Bitset> = p
            .entries
            .iter()
            .map(|e| match wl.len_data(e.len).n {
                0 => Bitset::zeros(1), // no words of this length: empty domain
                n => Bitset::ones(n),
            })
            .collect();
        let used: Vec<Bitset> = (0..=crate::wordlist::MAX_LEN)
            .map(|l| Bitset::zeros(wl.by_len[l].n.max(1)))
            .collect();
        // crossings: for each entry, the perpendicular neighbor at each cell.
        let mut crossings: Vec<Vec<(usize, usize, usize)>> = vec![Vec::new(); n];
        for (ei, cx) in crossings.iter_mut().enumerate() {
            for (pos, &cid) in p.entries[ei].cells.iter().enumerate() {
                for &(other, opos) in &p.cell_entries[cid] {
                    if other != ei {
                        cx.push((pos, other, opos));
                    }
                }
            }
        }

        // Resolve locks: build the combined prefill (template + locked answers),
        // mark locked entries, and record their wordlist indices (for dup
        // avoidance) and scores (for output).
        let mut is_locked = vec![false; n];
        let mut base_cell = p.prefilled.clone();
        let mut locked_used = Vec::new();
        let mut locked_out = Vec::new();
        for lk in locks {
            let ei = p
                .find_entry(lk.row, lk.col, lk.dir)
                .ok_or_else(|| format!("no entry at r{}c{} {:?}", lk.row, lk.col, lk.dir))?;
            let answer = lk.answer.to_ascii_uppercase();
            let mut letters: Vec<u8> = Vec::with_capacity(answer.len());
            for ch in answer.chars() {
                if !ch.is_ascii_uppercase() {
                    return Err(format!("answer '{}' has non-letter '{}'", answer, ch));
                }
                letters.push(ch as u8 - b'A');
            }
            let e = &p.entries[ei];
            if letters.len() != e.len {
                return Err(format!(
                    "answer '{}' (len {}) doesn't fit entry at r{}c{} {:?} (len {})",
                    answer,
                    letters.len(),
                    lk.row,
                    lk.col,
                    lk.dir,
                    e.len
                ));
            }
            is_locked[ei] = true;
            for (pos, &cid) in e.cells.iter().enumerate() {
                match base_cell[cid] {
                    Some(x) if x != letters[pos] => {
                        return Err(format!("lock conflict at cell r{}c{}", lk.row, lk.col));
                    }
                    _ => base_cell[cid] = Some(letters[pos]),
                }
            }
            let score = match wl.len_data(e.len).index_of(&letters) {
                Some(w) => {
                    locked_used.push((e.len, w));
                    wl.len_data(e.len).scores[w]
                }
                None => 90, // theme answer not in wordlist: treat as intentional/clean
            };
            locked_out.push((ei, answer, score));
        }
        let n_searchable = is_locked.iter().filter(|&&b| !b).count();
        let same_len: Vec<Vec<usize>> = (0..n)
            .map(|ei| {
                (0..n)
                    .filter(|&o| o != ei && !is_locked[o] && p.entries[o].len == p.entries[ei].len)
                    .collect()
            })
            .collect();

        let defaults = SolveConfig::default();
        let mut s = Solver {
            p,
            wl,
            base_domains: Vec::new(),
            domains,
            dom_count: vec![0; n],
            cutoff_by_entry: vec![0; n],
            crossings,
            same_len,
            assigned: vec![None; n],
            cell_letter: vec![None; p.n_cells],
            used,
            n_assigned: 0,
            is_locked,
            base_cell_letter: base_cell,
            locked_used,
            locked_out,
            n_searchable,
            nodes: 0,
            attempt_budget: u64::MAX,
            deadline: Instant::now(),
            floor: 0,
            cand_cap: 256,
            rng: Rng::new(0),
            propagation: defaults.propagation,
            backjump: defaults.backjump,
            slot_order: defaults.slot_order,
            linked: defaults.linked,
            lookahead: defaults.lookahead,
            avoid_substrings: defaults.avoid_substrings,
            cancel: None,
            grams: std::collections::HashMap::new(),
            gram_conflict: Bitset::zeros(n.max(1)),
            saved_in: vec![0; n],
            serial: 0,
            culprits: vec![Bitset::zeros(n.max(1)); n],
            conflict: Bitset::zeros(n.max(1)),
            masks: p.entries.iter().map(|e| vec![0u32; e.len]).collect(),
            support: p.entries.iter().map(|e| vec![0u32; e.len * 26]).collect(),
            truncated: false,
        };
        s.compute_base();
        Ok(s)
    }

    fn cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Analyze the grid without searching: entries with no candidate,
    /// letters still possible in each cell and letters forced by
    /// arc-consistent propagation of the prefilled letters and locks.
    pub fn analyze(&mut self) -> Analysis {
        self.propagation = Propagation::Arc;
        self.reset(0);
        let n = self.p.entries.len();
        let searchable = |s: &Self, e: usize| !s.is_locked[e];
        let mut impossible: Vec<usize> = (0..n)
            .filter(|&e| searchable(self, e) && self.dom_count[e] == 0)
            .collect();
        let feasible = impossible.is_empty() && self.propagate_all();
        if impossible.is_empty() && !feasible {
            impossible = (0..n)
                .filter(|&e| searchable(self, e) && self.dom_count[e] == 0)
                .collect();
        }
        let all = (1u32 << 26) - 1;
        let mut cell_letters = vec![all; self.p.n_cells];
        let mut forced = Vec::new();
        for (cid, mask) in cell_letters.iter_mut().enumerate() {
            if let Some(c) = self.base_cell_letter[cid] {
                *mask = 1 << c;
                continue;
            }
            for &(e, pos) in &self.p.cell_entries[cid] {
                if searchable(self, e) {
                    *mask &= self.masks[e][pos];
                }
            }
            if mask.count_ones() == 1 {
                let (r, c) = self.p.cell_rc[cid];
                forced.push((r, c, b'A' + mask.trailing_zeros() as u8));
            }
        }
        Analysis {
            feasible,
            impossible,
            candidates: self.dom_count.clone(),
            cell_letters,
            forced,
        }
    }

    /// Constrain every entry's domain by the combined prefill (template + locks)
    /// and snapshot as the per-attempt base.
    fn compute_base(&mut self) {
        for ei in 0..self.p.entries.len() {
            let e = &self.p.entries[ei];
            let ld = self.wl.len_data(e.len);
            for (pos, &cid) in e.cells.iter().enumerate() {
                if let Some(c) = self.base_cell_letter[cid] {
                    let mask = ld.compat(pos, c).clone();
                    self.domains[ei].and_assign(&mask);
                }
            }
        }
        self.base_domains = self.domains.clone();
    }

    pub fn solve(&mut self, cfg: &SolveConfig) -> SolveResult {
        self.cand_cap = cfg.cand_cap;
        self.propagation = cfg.propagation;
        self.backjump = cfg.backjump;
        self.slot_order = cfg.slot_order;
        self.linked = cfg.linked;
        self.lookahead = cfg.lookahead;
        self.avoid_substrings = cfg.avoid_substrings;
        self.cancel = cfg.cancel.clone();
        self.rng = Rng::new(cfg.seed);
        let t0 = Instant::now();
        let global_deadline = t0 + std::time::Duration::from_secs_f64(cfg.time_limit_s);

        let mut best: Option<SolvedFill> = None;
        let mut clean: Option<SolvedFill> = None;
        let mut total_nodes = 0u64;
        let mut restarts = 0u64;

        // Tiers are quality floors. We work feasibility-first: the LOWEST floor
        // is most permissive and most likely to solve, so we secure a baseline
        // there before spending any time on stricter (higher-quality) floors —
        // this avoids the trap of burning the budget on an infeasible high
        // floor (almost every grid needs some sub-50 glue).
        let mut tiers = if cfg.tiers.is_empty() {
            vec![0u8]
        } else {
            cfg.tiers.clone()
        };
        tiers.sort_unstable();
        tiers.dedup();
        let lowest = tiers[0];

        // An entry with no candidates at all can't be fixed by any restart;
        // with arc consistency, neither can a base that propagates to a
        // wipeout.
        let mut infeasible = (0..self.p.entries.len())
            .any(|ei| !self.is_locked[ei] && self.base_domains[ei].count_ones() == 0);
        if !infeasible && self.propagation == Propagation::Arc {
            self.reset(lowest);
            infeasible = !self.propagate_all();
        }
        if infeasible {
            return SolveResult {
                letters: None,
                nodes: 0,
                restarts: 0,
                elapsed_s: t0.elapsed().as_secs_f64(),
                reason: "unsolved",
                mean_score: None,
                min_score: None,
                weak_count: None,
                fill: None,
                clean: None,
            };
        }

        // Phase 1 — feasibility: restart at the lowest floor until first solve.
        let mut budget = cfg.initial_budget;
        while Instant::now() < global_deadline && best.is_none() && !self.cancelled() {
            restarts += 1;
            let seed = self.rng.next_u64();
            let outcome = self.attempt(lowest, budget, seed, global_deadline);
            total_nodes += self.nodes;
            if outcome == Outcome::Solved {
                self.consider(&mut best, &mut clean, cfg);
            }
            if outcome == Outcome::Exhausted && !self.truncated {
                break; // the whole tree was explored: proven unfillable
            }
            budget = (budget * 2).min(cfg.max_budget);
        }

        if best.is_some() && !cfg.stop_on_first {
            // Phase 2 — clean hunt: the floors at/above the clean bar get the
            // FIRST slice (half) of the remaining time, highest floor first,
            // each with its own doubling budgets. Clean fills are found fast
            // when they exist at all (measured on 15x15: the floor-60 solve
            // rate barely moves between 2s and 1s of dedicated search), so a
            // bounded up-front slice captures most of them without starving
            // the mean polish below. First clean solve ends the hunt — the
            // polish round-robin keeps improving every floor afterwards.
            let hunt_floors: Vec<u8> = tiers
                .iter()
                .rev()
                .copied()
                .filter(|&f| cfg.clean_floor > 0 && f >= cfg.clean_floor && f > lowest)
                .collect();
            let hunt_start = Instant::now();
            if !hunt_floors.is_empty() && hunt_start < global_deadline && clean.is_none() {
                let slice = (global_deadline - hunt_start) / (2 * hunt_floors.len() as u32);
                'hunt: for (i, &floor) in hunt_floors.iter().enumerate() {
                    let tier_deadline = (hunt_start + slice * (i as u32 + 1)).min(global_deadline);
                    budget = cfg.initial_budget;
                    while Instant::now() < tier_deadline && !self.cancelled() {
                        restarts += 1;
                        let seed = self.rng.next_u64();
                        let outcome = self.attempt(floor, budget, seed, tier_deadline);
                        total_nodes += self.nodes;
                        if outcome == Outcome::Solved {
                            self.consider(&mut best, &mut clean, cfg);
                            break 'hunt;
                        }
                        if outcome == Outcome::Exhausted && !self.truncated {
                            break; // this floor is proven infeasible
                        }
                        budget = (budget * 2).min(cfg.max_budget);
                    }
                }
            }

            // Phase 3 — polish: spend the remaining time round-robining over
            // the tiers (highest/cleanest first), keeping the best fill by
            // mean score — and the best clean fill alongside. Higher floors
            // yield cleaner fills when feasible; the lowest floor keeps
            // contributing diverse alternatives.
            let mut polish_floors: Vec<u8> = tiers.clone();
            polish_floors.sort_unstable_by(|a, b| b.cmp(a)); // desc
            let mut idx = 0usize;
            budget = cfg.initial_budget;
            while Instant::now() < global_deadline && !self.cancelled() {
                let floor = polish_floors[idx % polish_floors.len()];
                idx += 1;
                restarts += 1;
                let seed = self.rng.next_u64();
                let outcome = self.attempt(floor, budget, seed, global_deadline);
                total_nodes += self.nodes;
                if outcome == Outcome::Solved {
                    self.consider(&mut best, &mut clean, cfg);
                }
                budget = (budget * 2).min(cfg.max_budget);
                // reset budget growth each full cycle so every floor gets small
                // (fast) attempts too
                if idx.is_multiple_of(polish_floors.len()) {
                    budget = cfg.initial_budget;
                }
            }
        }

        // Phase 4 — repair: the clean hunt re-searches the WHOLE grid from
        // scratch, which on large grids often times out even when the best
        // fill is one bad corner away from clean. Rip the weak entries plus
        // their crossings out of the best fill, lock the rest, and re-solve
        // just that region at the clean floor — a tiny search that converts
        // almost-clean fills into clean ones. Skipped under stop_on_first
        // (those callers want the first answer, not polish).
        if !cfg.stop_on_first && cfg.repair_max_weak > 0 && cfg.clean_floor > 0 {
            self.repair_phase(&best, &mut clean, cfg);
        }

        let elapsed = t0.elapsed().as_secs_f64();
        match best {
            Some(b) => {
                let SolvedFill {
                    letters,
                    mean_score,
                    min_score,
                    weak_count,
                    fill,
                } = b;
                SolveResult {
                    letters: Some(letters),
                    nodes: total_nodes,
                    restarts,
                    elapsed_s: elapsed,
                    reason: "solved",
                    mean_score: Some(mean_score),
                    min_score: Some(min_score),
                    weak_count: Some(weak_count),
                    fill: Some(fill),
                    clean,
                }
            }
            None => SolveResult {
                letters: None,
                nodes: total_nodes,
                restarts,
                elapsed_s: elapsed,
                reason: "unsolved",
                mean_score: None,
                min_score: None,
                weak_count: None,
                fill: None,
                clean: None,
            },
        }
    }

    /// Clean-track ordering: fewest weak (sub-`weak_bar`) entries first, mean
    /// as the tie-break. With the bar disabled both counts are 0 and this
    /// degenerates to pure mean.
    fn better_clean(a: &SolvedFill, b: &SolvedFill) -> bool {
        a.weak_count < b.weak_count || (a.weak_count == b.weak_count && a.mean_score > b.mean_score)
    }

    /// Fold the just-solved fill into the running bests: overall (highest
    /// mean) and clean (fewest weak, then highest mean, among fills whose
    /// min >= `clean_floor`).
    fn consider(
        &self,
        best: &mut Option<SolvedFill>,
        clean: &mut Option<SolvedFill>,
        cfg: &SolveConfig,
    ) {
        let s = self.snapshot_solution(cfg.weak_bar);
        if cfg.clean_floor > 0
            && s.min_score >= cfg.clean_floor
            && clean.as_ref().is_none_or(|c| Self::better_clean(&s, c))
        {
            *clean = Some(s.clone());
        }
        if best.as_ref().is_none_or(|b| s.mean_score > b.mean_score) {
            *best = Some(s);
        }
    }

    fn snapshot_solution(&self, weak_bar: u8) -> SolvedFill {
        // Quality stats (mean/min/weak) are computed over the SEARCHED entries
        // only — locked theme answers are a given, not a measure of fill skill.
        let mut total = 0u32;
        let mut min_s = 100u8;
        let mut weak = 0usize;
        let mut fill = Vec::with_capacity(self.p.entries.len());
        for ei in 0..self.p.entries.len() {
            if self.is_locked[ei] {
                continue;
            }
            let w = self.assigned[ei].unwrap();
            let ld = self.wl.len_data(self.p.entries[ei].len);
            let sc = ld.scores[w];
            total += sc as u32;
            min_s = min_s.min(sc);
            if weak_bar > 0 && sc < weak_bar {
                weak += 1;
            }
            fill.push((ei, ld.word_string(w), sc));
        }
        // Append locked theme answers to the fill list (for clue output).
        for (ei, answer, score) in &self.locked_out {
            fill.push((*ei, answer.clone(), *score));
        }
        let n = self.n_searchable.max(1);
        SolvedFill {
            mean_score: total as f64 / n as f64,
            letters: self.cell_letter.clone(),
            fill,
            min_score: if self.n_searchable == 0 { 100 } else { min_s },
            weak_count: weak,
        }
    }

    /// Try to repair both the mean-best and the clean-best fill; keep a repair
    /// when it beats the current clean fill (fewer weak entries, then mean).
    fn repair_phase(
        &self,
        best: &Option<SolvedFill>,
        clean: &mut Option<SolvedFill>,
        cfg: &SolveConfig,
    ) {
        let targets: Vec<SolvedFill> = best.iter().chain(clean.iter()).cloned().collect();
        for t in targets {
            if let Some(rep) = self.repair(&t, cfg) {
                if clean.as_ref().is_none_or(|c| Self::better_clean(&rep, c)) {
                    *clean = Some(rep);
                }
            }
        }
    }

    /// Rip the weak entries (searched, score < `weak_bar`, or < `clean_floor`
    /// without a weak bar) plus their
    /// crossing entries out of `b`, lock every other entry to its current
    /// answer, and re-solve just that region with clean-floor words on a
    /// small budget (an inner solver; its nodes aren't added to the outer
    /// totals). On success the merged fill has min >= `clean_floor` by
    /// construction: every kept searched entry was already at/above the
    /// floor, and region words are drawn from at/above it. Locked theme
    /// answers are never ripped. Returns None when there is nothing to repair,
    /// the weak set exceeds `repair_max_weak`, or the region doesn't
    /// re-solve in its slice.
    fn repair(&self, b: &SolvedFill, cfg: &SolveConfig) -> Option<SolvedFill> {
        if cfg.repair_max_weak == 0 {
            return None;
        }
        // Targets: entries under the weak bar (or, without one, under the
        // clean floor), weakest first.
        let bar = cfg.weak_bar.max(cfg.clean_floor);
        let mut weak: Vec<(u8, usize)> = b
            .fill
            .iter()
            .filter(|(ei, _, sc)| !self.is_locked[*ei] && *sc < bar)
            .map(|(ei, _, sc)| (*sc, *ei))
            .collect();
        if weak.is_empty() {
            return None;
        }
        weak.sort_unstable();
        let slice = (cfg.time_limit_s * REPAIR_TIME_FRAC).max(0.05);
        // 1) all weak entries at once, when few enough
        if weak.len() <= cfg.repair_max_weak {
            let targets: Vec<usize> = weak.iter().map(|&(_, e)| e).collect();
            if let Some(r) = self.repair_region(b, cfg, &targets, 1, slice) {
                return Some(r);
            }
        }
        // 2) greedy: one weak entry (plus its crossings) at a time, keeping
        //    each refill that leaves fewer weak entries
        let step = (slice / weak.len() as f64).max(0.05);
        let mut cur = b.clone();
        let mut improved = false;
        for &(_, ei) in &weak {
            let still_weak = cur.fill.iter().any(|(e, _, sc)| *e == ei && *sc < bar);
            if !still_weak {
                continue;
            }
            // radius 2: in dense grids a weak word plus its crossings is
            // pinned by the words around it; the second ring frees it
            if let Some(r) = self.repair_region(&cur, cfg, &[ei], 2, step) {
                if Self::better_clean(&r, &cur) {
                    cur = r;
                    improved = true;
                }
            }
        }
        improved.then_some(cur)
    }

    /// Rip `targets` and the entries within `radius` crossings out of `b`,
    /// lock the rest, and refill
    /// that region (first without weak words, else at the clean floor).
    fn repair_region(
        &self,
        b: &SolvedFill,
        cfg: &SolveConfig,
        targets: &[usize],
        radius: usize,
        time_s: f64,
    ) -> Option<SolvedFill> {
        let n = self.p.entries.len();
        // Region: the targets plus every searched entry within `radius`
        // crossings of one.
        let mut in_region = vec![false; n];
        let mut frontier: Vec<usize> = targets.to_vec();
        for &ei in targets {
            in_region[ei] = true;
        }
        for _ in 0..radius {
            let mut next = Vec::new();
            for &ei in &frontier {
                for &(_, other, _) in &self.crossings[ei] {
                    if !self.is_locked[other] && !in_region[other] {
                        in_region[other] = true;
                        next.push(other);
                    }
                }
            }
            frontier = next;
        }

        // Everything outside the region — kept searched entries and original
        // theme locks alike — becomes a lock for the inner solver.
        let mut answer_of: Vec<Option<&String>> = vec![None; n];
        for (ei, ans, _sc) in &b.fill {
            answer_of[*ei] = Some(ans);
        }
        let mut locks: Vec<Lock> = Vec::with_capacity(n);
        for (ei, region) in in_region.iter().enumerate() {
            if *region {
                continue;
            }
            let e = &self.p.entries[ei];
            locks.push(Lock {
                row: e.row,
                col: e.col,
                dir: e.dir,
                answer: answer_of[ei]?.clone(),
            });
        }

        // Refill the region with no weak word if possible; else at least
        // with clean ones.
        let floors: Vec<u8> = if cfg.weak_bar > cfg.clean_floor {
            vec![cfg.weak_bar, cfg.clean_floor]
        } else {
            vec![cfg.clean_floor]
        };
        floors
            .into_iter()
            .find_map(|floor| self.resolve_region(cfg, &locks, floor, time_s))
    }

    /// Re-solve the entries not covered by `locks` with words scoring at
    /// least `floor`, merged into a full fill with the outer solver's stats.
    fn resolve_region(
        &self,
        cfg: &SolveConfig,
        locks: &[Lock],
        floor: u8,
        time_s: f64,
    ) -> Option<SolvedFill> {
        let n = self.p.entries.len();
        let mut inner = Solver::with_locks(self.p, self.wl, locks).ok()?;
        let r = inner.solve(&SolveConfig {
            time_limit_s: time_s,
            tiers: vec![floor],
            seed: cfg.seed.wrapping_add(0x9E37_79B9),
            initial_budget: cfg.initial_budget,
            max_budget: cfg.max_budget,
            cand_cap: cfg.cand_cap,
            stop_on_first: true, // any region solve is clean by construction
            clean_floor: cfg.clean_floor,
            repair_max_weak: 0, // no recursive repair
            weak_bar: cfg.weak_bar,
            propagation: cfg.propagation,
            backjump: cfg.backjump,
            slot_order: cfg.slot_order,
            linked: cfg.linked,
            lookahead: cfg.lookahead,
            avoid_substrings: cfg.avoid_substrings,
            cancel: cfg.cancel.clone(),
        });
        let letters = r.letters?;
        let inner_fill = r.fill?;

        // Merge back into the OUTER solver's stats convention: quality over
        // outer-searched entries only, locked theme answers appended. Kept
        // entries come back through the inner solver's lock reporting with
        // their real wordlist scores, so the merge reads uniformly.
        let mut by_entry: Vec<Option<(String, u8)>> = vec![None; n];
        for (ei, ans, sc) in inner_fill {
            by_entry[ei] = Some((ans, sc));
        }
        let mut total = 0u32;
        let mut min_s = 100u8;
        let mut weak_n = 0usize;
        let mut fill = Vec::with_capacity(n);
        for (ei, slot) in by_entry.iter_mut().enumerate() {
            if self.is_locked[ei] {
                continue;
            }
            let (ans, sc) = slot.take()?;
            total += sc as u32;
            min_s = min_s.min(sc);
            if cfg.weak_bar > 0 && sc < cfg.weak_bar {
                weak_n += 1;
            }
            fill.push((ei, ans, sc));
        }
        for (ei, answer, score) in &self.locked_out {
            fill.push((*ei, answer.clone(), *score));
        }
        debug_assert!(
            self.n_searchable == 0 || min_s >= cfg.clean_floor,
            "repair must produce a clean fill (min {} < floor {})",
            min_s,
            cfg.clean_floor
        );
        let ns = self.n_searchable.max(1);
        Some(SolvedFill {
            letters,
            mean_score: total as f64 / ns as f64,
            min_score: if self.n_searchable == 0 { 100 } else { min_s },
            weak_count: weak_n,
            fill,
        })
    }

    /// One bounded attempt at a given quality floor. Resets state first.
    fn attempt(&mut self, floor: u8, budget: u64, seed: u64, deadline: Instant) -> Outcome {
        self.floor = floor;
        self.attempt_budget = budget;
        self.deadline = deadline;
        self.nodes = 0;
        self.rng = Rng::new(seed);
        self.truncated = false;
        self.reset(floor);
        if self.propagation == Propagation::Arc && !self.propagate_all() {
            return Outcome::Exhausted;
        }
        self.recurse()
    }

    /// Reset the working state to the base domains for a search at `floor`.
    fn reset(&mut self, floor: u8) {
        let n = self.p.entries.len().max(1);
        for c in self.culprits.iter_mut() {
            *c = Bitset::zeros(n);
        }
        for l in 0..self.used.len() {
            self.used[l] = Bitset::zeros(self.wl.by_len[l].n.max(1));
        }
        // Locked answers present in the wordlist are marked used so the search
        // can't reuse them elsewhere (no duplicate answers).
        for &(len, w) in &self.locked_used {
            self.used[len].set(w);
        }
        // locked answers' substrings are taken from the start (no depth)
        self.grams.clear();
        let locked: Vec<Vec<u8>> = self
            .locked_out
            .iter()
            .map(|(_, a, _)| a.bytes().map(|b| b - b'A').collect())
            .collect();
        for l in locked {
            self.push_grams(&l, usize::MAX);
        }
        for ei in 0..self.p.entries.len() {
            self.domains[ei].copy_from(&self.base_domains[ei]);
            self.assigned[ei] = None;
            let len = self.p.entries[ei].len;
            if !self.is_locked[ei] {
                for &(ulen, w) in &self.locked_used {
                    if ulen == len {
                        self.domains[ei].clear(w);
                    }
                }
            }
            let cutoff = self.wl.len_data(len).tier_cutoff(floor);
            self.cutoff_by_entry[ei] = cutoff;
            self.rebuild(ei);
        }
        for cid in 0..self.p.n_cells {
            self.cell_letter[cid] = self.base_cell_letter[cid];
        }
        self.n_assigned = 0;
    }

    /// After `assign`, is some unassigned crossing entry left with no
    /// candidates? (Checks the post-propagation counts; the frame only holds
    /// the saved pre-assignment values for undo.)
    fn frame_dead(&mut self, frame: &TrailFrame) -> bool {
        match frame.nbrs.iter().find(|sv| self.dom_count[sv.entry] == 0) {
            Some(sv) => {
                self.conflict = self.culprits[sv.entry].clone();
                true
            }
            None => false,
        }
    }

    fn recurse(&mut self) -> Outcome {
        if self.n_assigned == self.n_searchable {
            return Outcome::Solved;
        }
        self.nodes += 1;
        if self.nodes >= self.attempt_budget {
            return Outcome::Budget;
        }
        if self.nodes & 15 == 0 && (Instant::now() >= self.deadline || self.cancelled()) {
            return Outcome::TimeLimit;
        }

        let depth = self.n_assigned;
        let Some(ei) = self.select_entry() else {
            self.conflict_all_below(depth);
            return Outcome::Exhausted;
        };

        let candidates = self.ordered_candidates(ei);
        // With the candidate cap hit, failure doesn't prove the domain empty.
        let truncated =
            candidates.len() >= self.cand_cap && (self.dom_count[ei] as usize) > candidates.len();
        self.truncated |= truncated;
        let mut conf = self.culprits[ei].clone();
        // candidates rejected for repeated substrings depend on the entries
        // holding those substrings
        conf.or_assign(&self.gram_conflict);

        for w in candidates {
            let mut frame = self.assign(ei, w);
            let alive = !self.frame_dead(&frame)
                && (self.propagation == Propagation::Forward || self.propagate(&mut frame));
            if alive {
                match self.recurse() {
                    Outcome::Solved => return Outcome::Solved,
                    r @ (Outcome::Budget | Outcome::TimeLimit) => {
                        self.undo(frame);
                        return r;
                    }
                    Outcome::Exhausted => {
                        if self.backjump && !self.conflict.get(depth) {
                            // The failure below doesn't involve this choice:
                            // no other candidate here can fix it — jump.
                            self.undo(frame);
                            return Outcome::Exhausted;
                        }
                    }
                }
            }
            conf.or_assign(&self.conflict);
            self.undo(frame);
        }
        conf.clear(depth);
        if truncated {
            conf.or_assign(&Self::all_below(conf.nbits(), depth));
        }
        self.conflict = conf;
        Outcome::Exhausted
    }

    fn all_below(nbits: usize, depth: usize) -> Bitset {
        let mut b = Bitset::zeros(nbits);
        for d in 0..depth.min(nbits) {
            b.set(d);
        }
        b
    }

    fn conflict_all_below(&mut self, depth: usize) {
        self.conflict = Self::all_below(self.conflict.nbits(), depth);
    }

    fn select_entry(&self) -> Option<usize> {
        let open = |ei: usize| self.assigned[ei].is_none() && !self.is_locked[ei];
        let filled = |ei: usize| self.assigned[ei].is_some() || self.is_locked[ei];
        // "linked": restrict to entries crossing the filled part, if any do.
        let only_linked = self.linked
            && (0..self.p.entries.len())
                .any(|ei| open(ei) && self.crossings[ei].iter().any(|&(_, o, _)| filled(o)));
        let mut best = None;
        // key: candidates (per empty cell for PerCell, scaled to stay
        // integral), then longer entry first
        let mut best_key = (u64::MAX, 0usize);
        for ei in 0..self.p.entries.len() {
            if !open(ei) {
                continue;
            }
            if only_linked && !self.crossings[ei].iter().any(|&(_, o, _)| filled(o)) {
                continue;
            }
            let cnt = self.dom_count[ei] as u64;
            let len = self.p.entries[ei].len;
            let score = match self.slot_order {
                SlotOrder::Mrv => cnt,
                SlotOrder::PerCell => {
                    let empty = self.p.entries[ei]
                        .cells
                        .iter()
                        .filter(|&&c| self.cell_letter[c].is_none())
                        .count()
                        .max(1) as u64;
                    cnt * 1000 / empty
                }
            };
            if score < best_key.0 || (score == best_key.0 && len > best_key.1) {
                best_key = (score, len);
                best = Some(ei);
            }
        }
        best
    }

    /// In-tier candidates ordered by score band (desc), then least-constraining
    /// value (high neighbor freedom), with light random jitter for restart
    /// diversity. The band term dominates so quality ordering is preserved.
    /// Keys of the k-grams of `letters` (empty when k is 0 or too long).
    fn gram_keys(letters: &[u8], k: usize) -> Vec<u64> {
        if k == 0 || k > 12 || letters.len() < k {
            return Vec::new();
        }
        letters
            .windows(k)
            .map(|g| g.iter().fold(0u64, |acc, &c| acc * 32 + c as u64 + 1))
            .collect()
    }

    fn push_grams(&mut self, letters: &[u8], depth: usize) {
        for g in Self::gram_keys(letters, self.avoid_substrings) {
            self.grams.entry(g).or_default().push(depth);
        }
    }

    fn pop_grams(&mut self, letters: &[u8]) {
        for g in Self::gram_keys(letters, self.avoid_substrings) {
            if let Some(v) = self.grams.get_mut(&g) {
                v.pop();
                if v.is_empty() {
                    self.grams.remove(&g);
                }
            }
        }
    }

    fn ordered_candidates(&mut self, ei: usize) -> Vec<usize> {
        let len = self.p.entries[ei].len;
        let cutoff = self.cutoff_by_entry[ei];

        // Active crossings to unassigned neighbors: (pos_in_ei, nbr_len, nbr_pos)
        let mut cross: Vec<(usize, usize, usize)> = Vec::new();
        for &(pos, nbr, npos) in &self.crossings[ei] {
            if self.assigned[nbr].is_none() && !self.is_locked[nbr] {
                cross.push((pos, self.p.entries[nbr].len, npos));
            }
        }

        // (word, score, lcv)
        let mut cands: Vec<(usize, u8, u32)> = Vec::new();
        let mut gram_culprits: Vec<usize> = Vec::new();
        {
            let ld = self.wl.len_data(len);
            let used = &self.used[len];
            for w in self.domains[ei].iter_ones() {
                if w >= cutoff {
                    break;
                }
                if used.get(w) {
                    continue;
                }
                let letters = &ld.letters[w];
                if self.avoid_substrings > 0 {
                    let clash: Vec<usize> = Self::gram_keys(letters, self.avoid_substrings)
                        .iter()
                        .filter_map(|g| self.grams.get(g))
                        .flatten()
                        .copied()
                        .collect();
                    if !clash.is_empty() {
                        gram_culprits.extend(clash);
                        continue;
                    }
                }
                let mut lcv: u32 = 0;
                for &(pos, nlen, npos) in &cross {
                    lcv = lcv.saturating_add(self.wl.len_data(nlen).pos_count(npos, letters[pos]));
                }
                cands.push((w, ld.scores[w], lcv));
                if cands.len() >= self.cand_cap {
                    break;
                }
            }
        }

        let nbits = self.gram_conflict.nbits();
        self.gram_conflict = Bitset::zeros(nbits);
        for d in gram_culprits {
            if d < nbits {
                self.gram_conflict.set(d); // STATIC (locked) grams carry no depth
            }
        }

        // key = band * BIG + jittered lcv. band dominates → score order kept.
        let mut keyed: Vec<(usize, i64)> = cands
            .iter()
            .map(|&(w, sc, lcv)| {
                let band = (sc / 5) as i64;
                let j = 850 + self.rng.below(301) as i64; // 0.85..1.15
                let key = band * 1_000_000_000 + (lcv as i64 * j / 1000);
                (w, key)
            })
            .collect();
        keyed.sort_by_key(|a| std::cmp::Reverse(a.1));
        let mut out: Vec<usize> = keyed.into_iter().map(|(w, _)| w).collect();
        if self.lookahead > 0 {
            self.apply_lookahead(ei, &mut out);
        }
        out
    }

    /// Rescore the first `lookahead` candidates by the options they really
    /// leave: for each open crossing, the in-tier words of the crossing
    /// entry compatible with the candidate's letter there. A candidate that
    /// leaves some crossing with none is dropped; the rest keep their score
    /// band and are reordered within it by the smallest such count.
    fn apply_lookahead(&self, ei: usize, cands: &mut Vec<usize>) {
        let len = self.p.entries[ei].len;
        let ld = self.wl.len_data(len);
        let k = self.lookahead.min(cands.len());
        let cross: Vec<(usize, usize, usize)> = self.crossings[ei]
            .iter()
            .copied()
            .filter(|&(_, nbr, _)| self.assigned[nbr].is_none() && !self.is_locked[nbr])
            .collect();
        let mut head: Vec<(usize, i64, u32)> = Vec::with_capacity(k);
        for &w in &cands[..k] {
            let mut least = u32::MAX;
            for &(pos, nbr, npos) in &cross {
                let nld = self.wl.len_data(self.p.entries[nbr].len);
                let c = self.domains[nbr].count_and_below(
                    nld.compat(npos, ld.letters[w][pos]),
                    self.cutoff_by_entry[nbr],
                );
                least = least.min(c);
            }
            if least > 0 {
                head.push((w, (ld.scores[w] / 5) as i64, least));
            }
        }
        head.sort_by_key(|&(_, band, least)| (std::cmp::Reverse(band), std::cmp::Reverse(least)));
        let tail = cands.split_off(k);
        cands.clear();
        cands.extend(head.into_iter().map(|(w, _, _)| w));
        cands.extend(tail);
    }

    fn assign(&mut self, ei: usize, w: usize) -> TrailFrame {
        let len = self.p.entries[ei].len;
        let letters: Vec<u8> = self.wl.len_data(len).letters[w].to_vec();

        self.assigned[ei] = Some(w);
        self.used[len].set(w);
        self.n_assigned += 1;
        if self.avoid_substrings > 0 {
            self.push_grams(&letters, self.n_assigned - 1);
        }

        self.serial += 1;
        let depth = self.n_assigned - 1; // this assignment's search depth
        let mut frame = TrailFrame {
            entry: ei,
            word: w,
            len,
            nbrs: Vec::new(),
            set_cells: Vec::new(),
        };
        // The word is taken: drop it from the other same-length domains.
        for k in 0..self.same_len[ei].len() {
            let o = self.same_len[ei][k];
            if self.assigned[o].is_some() || !self.domains[o].get(w) {
                continue;
            }
            self.save(&mut frame, o);
            self.culprits[o].set(depth);
            self.remove_one(o, w);
        }
        let cells = self.p.entries[ei].cells.clone();
        for (pos, &cid) in cells.iter().enumerate() {
            let ch = letters[pos];
            if self.cell_letter[cid].is_none() {
                self.cell_letter[cid] = Some(ch);
                frame.set_cells.push(cid);
            }
            for &(nbr, npos) in &self.p.cell_entries[cid] {
                if nbr == ei || self.assigned[nbr].is_some() || self.is_locked[nbr] {
                    continue;
                }
                let nlen = self.p.entries[nbr].len;
                let mut diff = self.domains[nbr].clone();
                diff.andnot_assign(self.wl.len_data(nlen).compat(npos, ch));
                self.save(&mut frame, nbr);
                self.culprits[nbr].set(depth);
                self.remove_words(nbr, &diff);
            }
        }
        frame
    }

    /// Snapshot `e`'s domain into `frame` (once per frame) before changing it.
    fn save(&mut self, frame: &mut TrailFrame, e: usize) {
        if self.saved_in[e] != self.serial {
            self.saved_in[e] = self.serial;
            frame.nbrs.push(Saved {
                entry: e,
                domain: self.domains[e].clone(),
                count: self.dom_count[e],
                culprits: self.culprits[e].clone(),
                masks: self.masks[e].clone(),
                support: self.support[e].clone(),
            });
        }
    }

    #[cfg(test)]
    fn letter_mask(&self, e: usize, pos: usize) -> u32 {
        self.masks[e][pos]
    }

    /// Recompute `e`'s in-tier count, letter supports and masks from its
    /// domain (attempt start).
    fn rebuild(&mut self, e: usize) {
        let cutoff = self.cutoff_by_entry[e];
        let ld = self.wl.len_data(self.p.entries[e].len);
        let len = self.p.entries[e].len;
        let mut sup = vec![0u32; len * 26];
        let mut count = 0u32;
        for w in self.domains[e].iter_ones() {
            if w >= cutoff {
                break;
            }
            count += 1;
            for (p, &c) in ld.letters[w].iter().enumerate() {
                sup[p * 26 + c as usize] += 1;
            }
        }
        let mut m = vec![0u32; len];
        for (p, mp) in m.iter_mut().enumerate() {
            for c in 0..26 {
                if sup[p * 26 + c] > 0 {
                    *mp |= 1 << c;
                }
            }
        }
        self.dom_count[e] = count;
        self.support[e] = sup;
        self.masks[e] = m;
    }

    /// Remove `diff` (a subset of `e`'s domain) from the domain, updating
    /// count, supports and masks for the in-tier words removed.
    fn remove_words(&mut self, e: usize, diff: &Bitset) {
        let cutoff = self.cutoff_by_entry[e];
        let ld = self.wl.len_data(self.p.entries[e].len);
        for w in diff.iter_ones() {
            if w >= cutoff {
                break;
            }
            self.dom_count[e] -= 1;
            for (p, &c) in ld.letters[w].iter().enumerate() {
                let k = p * 26 + c as usize;
                self.support[e][k] -= 1;
                if self.support[e][k] == 0 {
                    self.masks[e][p] &= !(1 << c);
                }
            }
        }
        self.domains[e].andnot_assign(diff);
    }

    fn remove_one(&mut self, e: usize, w: usize) {
        let mut d = Bitset::zeros(self.domains[e].nbits());
        d.set(w);
        self.remove_words(e, &d);
    }

    /// Letters (bit c = letter c) that entry `e` can still have at `pos`,
    /// counting only in-tier candidates.
    #[cfg(test)]
    fn letters_at(&self, e: usize, pos: usize) -> u32 {
        let ld = self.wl.len_data(self.p.entries[e].len);
        let cutoff = self.cutoff_by_entry[e];
        let mut m = 0u32;
        for c in 0..26u8 {
            if ld.pos_count(pos, c) > 0
                && self.domains[e].intersects_below(ld.compat(pos, c), cutoff)
            {
                m |= 1 << c;
            }
        }
        m
    }

    /// Arc consistency from the entries changed in `frame`, to a fixed
    /// point. Every changed domain is saved in `frame`. Returns false on a
    /// wipeout (some entry left with no in-tier candidate).
    fn propagate(&mut self, frame: &mut TrailFrame) -> bool {
        let start: Vec<usize> = frame.nbrs.iter().map(|sv| sv.entry).collect();
        self.revise_from(start, Some(frame))
    }

    /// Arc consistency over the whole grid (attempt start; no trail).
    fn propagate_all(&mut self) -> bool {
        let start: Vec<usize> = (0..self.p.entries.len())
            .filter(|&e| self.assigned[e].is_none() && !self.is_locked[e])
            .collect();
        self.revise_from(start, None)
    }

    fn revise_from(&mut self, start: Vec<usize>, mut frame: Option<&mut TrailFrame>) -> bool {
        let n = self.p.entries.len();
        let mut in_queue = vec![false; n];
        let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
        for e in start {
            if !in_queue[e] {
                in_queue[e] = true;
                queue.push_back(e);
            }
        }
        while let Some(s) = queue.pop_front() {
            in_queue[s] = false;
            for k in 0..self.crossings[s].len() {
                let (pos, t, tpos) = self.crossings[s][k];
                if self.assigned[t].is_some() || self.is_locked[t] {
                    continue;
                }
                let allowed = self.masks[s][pos];
                let removed = self.masks[t][tpos] & !allowed;
                if removed == 0 {
                    continue; // nothing to remove from t
                }
                let ld = self.wl.len_data(self.p.entries[t].len);
                let mut drop = Bitset::zeros(ld.n.max(1));
                for c in 0..26u8 {
                    if removed & (1 << c) != 0 {
                        drop.or_assign(ld.compat(tpos, c));
                    }
                }
                if let Some(f) = frame.as_deref_mut() {
                    self.save(f, t);
                    // t shrinks because s did: inherit s's culprits
                    let cs = self.culprits[s].clone();
                    self.culprits[t].or_assign(&cs);
                }
                let mut diff = self.domains[t].clone();
                diff.and_assign(&drop);
                self.remove_words(t, &diff);
                if self.dom_count[t] == 0 {
                    self.conflict = self.culprits[t].clone();
                    return false;
                }
                if !in_queue[t] {
                    in_queue[t] = true;
                    queue.push_back(t);
                }
            }
        }
        true
    }

    fn undo(&mut self, frame: TrailFrame) {
        // reverse order: an entry saved twice gets its oldest snapshot back
        for sv in frame.nbrs.into_iter().rev() {
            self.domains[sv.entry].copy_from(&sv.domain);
            self.dom_count[sv.entry] = sv.count;
            self.culprits[sv.entry] = sv.culprits;
            self.masks[sv.entry] = sv.masks;
            self.support[sv.entry] = sv.support;
        }
        for cid in frame.set_cells {
            self.cell_letter[cid] = None;
        }
        if self.avoid_substrings > 0 {
            let letters = self.wl.len_data(frame.len).letters[frame.word].to_vec();
            self.pop_grams(&letters);
        }
        self.used[frame.len].clear(frame.word);
        self.assigned[frame.entry] = None;
        self.n_assigned -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wordlist::Wordlist;

    fn dict() -> Wordlist {
        let src = "\
CAB;80
ORE;70
TEN;75
COT;60
ARE;65
BEN;55
CAT;50
CAR;50
RAT;40
DOG;40
";
        Wordlist::from_str(src, 40)
    }

    /// Quality-mechanism config on a 0–100 scale: tiers 40..70, clean floor
    /// 55, weak bar 70 (the mechanism tests below are written against it).
    fn q_cfg(time_limit_s: f64) -> SolveConfig {
        SolveConfig {
            time_limit_s,
            tiers: vec![40, 50, 55, 60, 70],
            clean_floor: 55,
            weak_bar: 70,
            ..Default::default()
        }
    }

    fn fast_cfg() -> SolveConfig {
        SolveConfig {
            time_limit_s: 2.0,
            stop_on_first: true,
            ..Default::default()
        }
    }

    #[test]
    fn solves_3x3_open() {
        let p = Puzzle::from_template("...\n...\n...\n");
        let wl = dict();
        let mut s = Solver::new(&p, &wl);
        let r = s.solve(&fast_cfg());
        assert_eq!(r.reason, "solved", "should fill a 3x3 double word square");
        let letters = r.letters.unwrap();
        let words: std::collections::HashSet<String> =
            s.p.entries
                .iter()
                .map(|e| {
                    e.cells
                        .iter()
                        .map(|&c| (b'A' + letters[c].unwrap()) as char)
                        .collect::<String>()
                })
                .collect();
        assert_eq!(words.len(), 6, "all six answers must be distinct");
    }

    #[test]
    fn respects_prefilled() {
        let p = Puzzle::from_template("CAB\n...\n...\n");
        let wl = dict();
        let mut s = Solver::new(&p, &wl);
        let r = s.solve(&fast_cfg());
        assert_eq!(r.reason, "solved");
        let letters = r.letters.unwrap();
        assert_eq!(letters[0], Some(2)); // C
        assert_eq!(letters[1], Some(0)); // A
        assert_eq!(letters[2], Some(1)); // B
    }

    #[test]
    fn locks_theme_answer_not_in_wordlist() {
        // ZZZ is not in the dict; lock it as the top across entry and fill the
        // 3x3 around it. The down entries must start with Z,Z,Z respectively.
        let p = Puzzle::from_template("...\n...\n...\n");
        let wl = dict();
        let locks = vec![Lock {
            row: 0,
            col: 0,
            dir: Dir::Across,
            answer: "ZZZ".into(),
        }];
        let mut s = Solver::with_locks(&p, &wl, &locks).unwrap();
        // The down words would need to start with Z — dict has none, so this
        // particular lock is unsatisfiable; assert we don't panic and report it.
        let r = s.solve(&fast_cfg());
        assert_eq!(r.reason, "unsolved");

        // Now a satisfiable lock: top row CAB (in dict), fill around it.
        let locks = vec![Lock {
            row: 0,
            col: 0,
            dir: Dir::Across,
            answer: "CAB".into(),
        }];
        let mut s = Solver::with_locks(&p, &wl, &locks).unwrap();
        let r = s.solve(&fast_cfg());
        assert_eq!(r.reason, "solved");
        let letters = r.letters.unwrap();
        assert_eq!(
            (letters[0], letters[1], letters[2]),
            (Some(2), Some(0), Some(1))
        );
        // The locked answer appears in the fill output.
        let fill = r.fill.unwrap();
        assert!(fill.iter().any(|(_, w, _)| w == "CAB"));
    }

    /// Two disjoint fills of the open 3x3 exist in this dict: a "dirty" one
    /// with the top mean (76.7) that leans on WEN;50, and a clean one
    /// (min 55, mean 67.5): CAB/ORE/TEN x COT/ARE/BEN vs PAW/IRE/TEN x
    /// PIT/ARE/WEN. Mean polish must keep the dirty fill as the primary
    /// result while the clean hunt surfaces the all->=55 fill alongside.
    #[test]
    fn tracks_best_clean_fill_alongside_mean_best() {
        let src =
            "CAB;80\nORE;70\nTEN;75\nCOT;60\nARE;65\nBEN;55\nPAW;90\nIRE;90\nPIT;90\nWEN;50\n";
        let wl = Wordlist::from_str(src, 40);
        let p = Puzzle::from_template("...\n...\n...\n");
        let mut s = Solver::new(&p, &wl);
        let r = s.solve(&q_cfg(1.0));
        assert_eq!(r.reason, "solved");
        assert!(
            (r.mean_score.unwrap() - 460.0 / 6.0).abs() < 0.1,
            "primary best is the higher-mean dirty fill, got {}",
            r.mean_score.unwrap()
        );
        assert_eq!(r.min_score.unwrap(), 50, "dirty fill bottoms out at WEN;50");
        let clean = r.clean.expect("clean hunt must find the all->=55 fill");
        assert_eq!(clean.min_score, 55);
        assert!((clean.mean_score - 67.5).abs() < 0.01);
    }

    /// With a clean floor no fill can reach (both families rely on sub-70
    /// words), `clean` stays empty while the primary fill still solves.
    #[test]
    fn clean_absent_when_floor_unreachable() {
        let src =
            "CAB;80\nORE;70\nTEN;75\nCOT;60\nARE;65\nBEN;55\nPAW;90\nIRE;90\nPIT;90\nWEN;50\n";
        let wl = Wordlist::from_str(src, 40);
        let p = Puzzle::from_template("...\n...\n...\n");
        let mut s = Solver::new(&p, &wl);
        let r = s.solve(&SolveConfig {
            clean_floor: 70,
            ..q_cfg(0.5)
        });
        assert_eq!(r.reason, "solved");
        assert!(r.clean.is_none(), "no fill has min >= 70");
    }

    /// Repair dict: the dirty 3x3 (CAB/ORE/TEN x COT/ARE/BEN) leans on
    /// ORE;50. Ripping ORE plus its crossings (all three downs) with CAB/TEN
    /// kept leaves exactly one clean region solve: UXU;60 with CUT/AXE/BUN —
    /// the only >=55 middle row compatible with the pinned C?T/A?E/B?N.
    fn repair_dict(down_alt_score: u8) -> Wordlist {
        let src = format!(
            "CAB;80\nTEN;75\nORE;50\nUXU;60\nCOT;90\nARE;90\nBEN;90\nCUT;{s}\nAXE;{s}\nBUN;{s}\n",
            s = down_alt_score
        );
        Wordlist::from_str(&src, 40)
    }

    /// The dirty best fill of the open 3x3 over `repair_dict`, hand-built so
    /// the repair mechanism can be exercised deterministically.
    fn dirty_fill(p: &Puzzle) -> SolvedFill {
        let e = |r, c, d| p.find_entry(r, c, d).unwrap();
        let fill = vec![
            (e(0, 0, Dir::Across), "CAB".to_string(), 80u8),
            (e(1, 0, Dir::Across), "ORE".to_string(), 50),
            (e(2, 0, Dir::Across), "TEN".to_string(), 75),
            (e(0, 0, Dir::Down), "COT".to_string(), 90),
            (e(0, 1, Dir::Down), "ARE".to_string(), 90),
            (e(0, 2, Dir::Down), "BEN".to_string(), 90),
        ];
        SolvedFill {
            letters: Vec::new(), // repair reads answers from `fill`, not letters
            mean_score: 475.0 / 6.0,
            min_score: 50,
            weak_count: 1, // ORE;50 under the default bar
            fill,
        }
    }

    /// Direct mechanism test: ripping the weak entry + crossings and
    /// re-solving at the clean floor produces the unique merged clean fill.
    #[test]
    fn repair_converts_weak_fill_to_clean() {
        let wl = repair_dict(90);
        let p = Puzzle::from_template("...\n...\n...\n");
        let s = Solver::new(&p, &wl);
        let b = dirty_fill(&p);
        let cfg = q_cfg(2.0);
        let rep = s
            .repair(&b, &cfg)
            .expect("region must re-solve at floor 55");
        assert_eq!(rep.min_score, 60, "weakest repaired entry is UXU;60");
        assert!(
            (rep.mean_score - 485.0 / 6.0).abs() < 0.01,
            "merged mean over all six entries, got {}",
            rep.mean_score
        );
        let words: Vec<&str> = rep.fill.iter().map(|(_, w, _)| w.as_str()).collect();
        for w in ["CAB", "TEN", "UXU", "CUT", "AXE", "BUN"] {
            assert!(words.contains(&w), "repaired fill must contain {w}");
        }
        assert!(!words.contains(&"ORE"), "the weak word must be gone");
        // middle row letters rewritten to U X U
        assert_eq!(rep.letters[3], Some(20));
        assert_eq!(rep.letters[4], Some(23));
        assert_eq!(rep.letters[5], Some(20));

        // Guard: repair_max_weak = 0 disables.
        let off = SolveConfig {
            repair_max_weak: 0,
            ..cfg
        };
        assert!(s.repair(&b, &off).is_none());
    }

    /// Theme locks are never ripped, keep their verbatim answer, and stay
    /// excluded from the merged quality stats.
    #[test]
    fn repair_preserves_theme_locks_and_stats() {
        let wl = repair_dict(90);
        let p = Puzzle::from_template("...\n...\n...\n");
        let locks = vec![Lock {
            row: 0,
            col: 0,
            dir: Dir::Across,
            answer: "CAB".into(),
        }];
        let s = Solver::with_locks(&p, &wl, &locks).unwrap();
        let e = |r, c, d| p.find_entry(r, c, d).unwrap();
        let b = SolvedFill {
            letters: Vec::new(),
            mean_score: 395.0 / 5.0,
            min_score: 50,
            weak_count: 1, // ORE;50 under the default bar
            fill: vec![
                (e(1, 0, Dir::Across), "ORE".to_string(), 50),
                (e(2, 0, Dir::Across), "TEN".to_string(), 75),
                (e(0, 0, Dir::Down), "COT".to_string(), 90),
                (e(0, 1, Dir::Down), "ARE".to_string(), 90),
                (e(0, 2, Dir::Down), "BEN".to_string(), 90),
                (e(0, 0, Dir::Across), "CAB".to_string(), 80), // locked, appended
            ],
        };
        let cfg = q_cfg(2.0);
        let rep = s.repair(&b, &cfg).expect("region must re-solve");
        assert_eq!(rep.min_score, 60);
        assert!(
            (rep.mean_score - 405.0 / 5.0).abs() < 0.01,
            "stats cover the five searched entries only, got {}",
            rep.mean_score
        );
        assert!(
            rep.fill.iter().any(|(_, w, _)| w == "CAB"),
            "locked theme answer stays in the fill output"
        );
    }

    /// End-to-end: when the clean hunt can't run (no >=55 tier configured)
    /// the repair phase still surfaces the clean alternative, while the
    /// higher-mean dirty fill stays the primary result.
    #[test]
    fn solve_repair_fills_clean_slot() {
        let wl = repair_dict(60); // low-scored alt downs: dirty keeps the mean lead
        let p = Puzzle::from_template("...\n...\n...\n");
        let mut s = Solver::new(&p, &wl);
        let r = s.solve(&SolveConfig {
            tiers: vec![40],
            ..q_cfg(0.5)
        });
        assert_eq!(r.reason, "solved");
        assert!(
            (r.mean_score.unwrap() - 475.0 / 6.0).abs() < 0.1,
            "primary best stays the dirty fill, got {}",
            r.mean_score.unwrap()
        );
        assert_eq!(r.min_score.unwrap(), 50);
        let clean = r.clean.expect("repair must surface the clean alternative");
        assert_eq!(clean.min_score, 60);
        assert!(
            (clean.mean_score - 395.0 / 6.0).abs() < 0.05,
            "clean fill is the UXU variant, got {}",
            clean.mean_score
        );
    }

    /// An entry with no candidates at all (here a 4-letter slot, the dict
    /// only has 3-letter words) is infeasible before any search: solve must
    /// report it at once instead of burning the whole budget on restarts.
    #[test]
    fn infeasible_base_fails_without_restarts() {
        let p = Puzzle::from_template("....\n");
        let wl = dict();
        let mut s = Solver::new(&p, &wl);
        let r = s.solve(&SolveConfig {
            time_limit_s: 5.0,
            ..Default::default()
        });
        assert_eq!(r.reason, "unsolved");
        assert_eq!(r.restarts, 0, "no restart can fix an empty domain");
        assert!(r.elapsed_s < 0.5, "must not run to the deadline");
    }

    // ---- punto 1: parole già usate fuori dai domini ----

    /// Assigning a word removes it from every other unassigned entry of the
    /// same length, so MRV counts only words that can still be placed.
    #[test]
    fn used_word_leaves_other_domains() {
        // two independent 3-letter across entries (row 1 is all black)
        let p = Puzzle::from_template("...\n###\n...\n");
        let wl = Wordlist::from_str("CAB;80\nORE;70\n", 0);
        let mut s = Solver::new(&p, &wl);
        s.reset(0);
        assert_eq!(s.dom_count[1], 2);
        let cab = wl.len_data(3).index_of(&[2, 0, 1]).unwrap();
        let f = s.assign(0, cab);
        assert_eq!(s.dom_count[1], 1, "CAB is taken: only ORE remains");
        s.undo(f);
        assert_eq!(s.dom_count[1], 2, "undo restores the count");
    }

    /// A crossing entry whose only compatible word is already used is a dead
    /// end, and must be detected right after the assignment.
    #[test]
    fn crossing_left_with_only_used_word_is_dead() {
        // 2x2 open grid: A0 (row 0), A1 (row 1), D0 (col 0), D1 (col 1)
        let p = Puzzle::from_template("..\n..\n");
        // AB is the only word starting with A
        let wl = Wordlist::from_str("AB;80\nBB;70\nBA;60\n", 0);
        let mut s = Solver::new(&p, &wl);
        s.reset(0);
        let a0 = p.find_entry(0, 0, Dir::Across).unwrap();
        let ab = wl.len_data(2).index_of(&[0, 1]).unwrap();
        let f = s.assign(a0, ab);
        // D0 must start with A → only AB, which A0 just used
        assert!(s.frame_dead(&f), "D0 has no placeable word left");
    }

    /// Locked answers that are in the word list are taken from the start.
    #[test]
    fn locked_answer_leaves_other_domains_at_reset() {
        let p = Puzzle::from_template("...\n###\n...\n");
        let wl = Wordlist::from_str("CAB;80\nORE;70\n", 0);
        let locks = vec![Lock {
            row: 0,
            col: 0,
            dir: Dir::Across,
            answer: "CAB".into(),
        }];
        let mut s = Solver::with_locks(&p, &wl, &locks).unwrap();
        s.reset(0);
        assert_eq!(s.dom_count[1], 1, "only ORE is placeable in row 2");
    }

    // ---- punto 2: arco-consistenza ----

    /// ABC across the top forces the downs to A?? / B?? / C??; with this
    /// dict every such down has X in the middle, so the middle across must
    /// start with X — and no word does. Forward checking only looks at the
    /// direct crossings and misses it; arc consistency must find the wipeout.
    fn ac_dict() -> Wordlist {
        Wordlist::from_str("ABC;80\nAXQ;70\nBXQ;70\nCXQ;70\nYYY;60\nQQQ;60\n", 0)
    }

    #[test]
    fn arc_consistency_finds_wipeout_forward_checking_misses() {
        let p = Puzzle::from_template("...\n...\n...\n");
        let wl = ac_dict();
        let mut s = Solver::new(&p, &wl);
        s.reset(0);
        let a0 = p.find_entry(0, 0, Dir::Across).unwrap();
        let abc = wl.len_data(3).index_of(&[0, 1, 2]).unwrap();
        let mut f = s.assign(a0, abc);
        assert!(!s.frame_dead(&f), "forward checking alone sees no dead end");
        assert!(
            !s.propagate(&mut f),
            "arc consistency must detect the wipeout"
        );
    }

    #[test]
    fn propagation_is_fully_undone() {
        let p = Puzzle::from_template("...\n...\n...\n");
        let wl = ac_dict();
        let mut s = Solver::new(&p, &wl);
        s.reset(0);
        let before: Vec<Bitset> = s.domains.clone();
        let counts = s.dom_count.clone();
        let a0 = p.find_entry(0, 0, Dir::Across).unwrap();
        let abc = wl.len_data(3).index_of(&[0, 1, 2]).unwrap();
        let mut f = s.assign(a0, abc);
        let _ = s.propagate(&mut f);
        s.undo(f);
        assert_eq!(s.domains, before);
        assert_eq!(s.dom_count, counts);
    }

    #[test]
    fn arc_consistency_rejects_infeasible_prefill_before_search() {
        let p = Puzzle::from_template("ABC\n...\n...\n");
        let wl = ac_dict();
        let mut s = Solver::new(&p, &wl);
        let r = s.solve(&SolveConfig {
            time_limit_s: 2.0,
            propagation: Propagation::Arc,
            ..Default::default()
        });
        assert_eq!(r.reason, "unsolved");
        assert_eq!(r.restarts, 0, "the base propagation already proves it");
    }

    #[test]
    fn arc_consistency_still_solves() {
        let p = Puzzle::from_template("...\n...\n...\n");
        let wl = dict();
        let mut s = Solver::new(&p, &wl);
        let r = s.solve(&SolveConfig {
            propagation: Propagation::Arc,
            ..fast_cfg()
        });
        assert_eq!(r.reason, "solved");
        assert_eq!(Propagation::parse("arc"), Some(Propagation::Arc));
        assert_eq!(Propagation::parse("forward"), Some(Propagation::Forward));
        assert_eq!(Propagation::parse("x"), None);
    }

    #[test]
    fn solver_options_from_cli_args() {
        let args: Vec<String> = ["--time", "3", "--propagation", "arc", "--other"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let cfg = SolveConfig::default().with_args(&args).unwrap();
        assert_eq!(cfg.propagation, Propagation::Arc);
        assert!(cfg.describe().starts_with("propagation arc"));
        let bad: Vec<String> = vec!["--propagation".into(), "boh".into()];
        assert!(SolveConfig::default().with_args(&bad).is_err());
        let bj: Vec<String> = vec!["--backjump".into(), "on".into()];
        let cfg = SolveConfig::default().with_args(&bj).unwrap();
        assert!(cfg.backjump);
        assert!(cfg.describe().contains("backjump on"));
        let bj: Vec<String> = vec!["--backjump".into(), "off".into()];
        assert!(!SolveConfig::default().with_args(&bj).unwrap().backjump);
        let bad_bj: Vec<String> = vec!["--backjump".into(), "forse".into()];
        assert!(SolveConfig::default().with_args(&bad_bj).is_err());
        let rw: Vec<String> = vec!["--repair-max-weak".into(), "0".into()];
        let cfg = SolveConfig::default().with_args(&rw).unwrap();
        assert_eq!(cfg.repair_max_weak, 0);
        assert!(cfg.describe().contains("repair-max-weak 0"));
        let rw: Vec<String> = vec!["--repair-max-weak".into(), "x".into()];
        assert!(SolveConfig::default().with_args(&rw).is_err());
        let missing: Vec<String> = vec!["--propagation".into()];
        assert!(SolveConfig::default().with_args(&missing).is_err());
        // arc consistency is the default (benchmark: 23/30 vs 20/30 filled)
        assert_eq!(SolveConfig::default().propagation, Propagation::Arc);
        // backjumping is the default (benchmark: 25/30 vs 23/30 filled)
        assert!(SolveConfig::default().backjump);
        assert!(SolveConfig::default()
            .describe()
            .starts_with("propagation arc"));
    }

    // ---- punto 3: backjumping guidato dai conflitti ----

    /// Run one exhaustive attempt (no budget, no deadline) and report the
    /// outcome and the node count.
    fn exhaust(p: &Puzzle, wl: &Wordlist, backjump: bool) -> (Outcome, u64) {
        let mut s = Solver::new(p, wl);
        s.propagation = Propagation::Arc;
        s.backjump = backjump;
        s.cand_cap = usize::MAX;
        let far = Instant::now() + std::time::Duration::from_secs(60);
        let out = s.attempt(0, u64::MAX, 7, far);
        (out, s.nodes)
    }

    /// The 2x2 at the bottom is arc-consistent but has no fill with four
    /// distinct words (AB AC BA BD DA). MRV first picks the four independent
    /// top entries (2 candidates each): chronological backtracking re-proves
    /// the 2x2 impossible under every combination of them, conflict-directed
    /// backjumping sees those choices aren't involved and jumps over them.
    #[test]
    fn backjumping_skips_choices_unrelated_to_the_conflict() {
        // four independent entries (lengths 3..6, two words each) on top
        let p = Puzzle::from_template(
            "...#....#.....#......\n#####################\n..###################\n..###################\n",
        );
        let wl = Wordlist::from_str(
            "AB;60\nAC;60\nBA;60\nBD;60\nDA;60\n\
             XYZ;60\nZYX;60\nWXYZ;60\nZYXW;60\nVWXYZ;60\nZYXWV;60\nUVWXYZ;60\nZYXWVU;60\n",
            0,
        );
        let (o_chrono, n_chrono) = exhaust(&p, &wl, false);
        let (o_cbj, n_cbj) = exhaust(&p, &wl, true);
        assert!(o_chrono == Outcome::Exhausted && o_cbj == Outcome::Exhausted);
        assert!(
            n_cbj * 2 < n_chrono,
            "backjumping must avoid re-proving the 2x2: {n_cbj} vs {n_chrono} nodes"
        );
    }

    /// Soundness: on many random small grids/dictionaries, backjumping must
    /// reach exactly the same verdict (fillable or not) as exhaustive
    /// chronological search — it may skip work, never solutions.
    #[test]
    fn backjumping_never_loses_solutions() {
        let mut rng = Rng::new(42);
        let letters = b"ABCDE";
        let templates = [
            "...\n...\n...\n",
            "...\n.#.\n...\n",
            "....\n.#..\n....\n",
            "..#\n...\n#..\n",
        ];
        let mut solvable = 0;
        for round in 0..120 {
            let t = templates[round % templates.len()];
            let mut src = String::new();
            for len in 2..=4 {
                for _ in 0..(4 + rng.below(8)) {
                    let w: String = (0..len)
                        .map(|_| letters[rng.below(letters.len())] as char)
                        .collect();
                    src.push_str(&format!("{w};{}\n", 50 + rng.below(40)));
                }
            }
            let wl = Wordlist::from_str(&src, 0);
            let p = Puzzle::from_template(t);
            if !wl.missing_lengths(&p).is_empty() {
                continue;
            }
            let (a, _) = exhaust(&p, &wl, false);
            let (b, _) = exhaust(&p, &wl, true);
            assert_eq!(
                a, b,
                "verdicts differ on round {round}: template {t:?} dict {src:?}"
            );
            if a == Outcome::Solved {
                solvable += 1;
            }
        }
        assert!(solvable > 5, "the sample must contain fillable cases too");
    }

    #[test]
    fn backjumping_still_solves() {
        let p = Puzzle::from_template("...\n...\n...\n");
        let wl = dict();
        let mut s = Solver::new(&p, &wl);
        let r = s.solve(&SolveConfig {
            backjump: true,
            ..fast_cfg()
        });
        assert_eq!(r.reason, "solved");
    }

    // ---- punto 4: ordine degli slot ----

    /// Row 0 is "CAB.." with two letters to fill; row 2 is 5 open cells.
    /// Rows don't cross (row 1 and the down runs are length 1).
    fn two_rows() -> (Puzzle, Wordlist) {
        let p = Puzzle::from_template("CAB..\n#####\n.....\n");
        let mut src = String::new();
        for i in 0..40u8 {
            let a = (b'A' + i % 26) as char;
            let b = (b'A' + i / 26) as char;
            src.push_str(&format!("CAB{a}{b};60\nX{a}{b}YZ;60\n"));
        }
        (p, Wordlist::from_str(&src, 0))
    }

    #[test]
    fn mrv_picks_fewest_candidates() {
        let (p, wl) = two_rows();
        let mut s = Solver::new(&p, &wl);
        s.reset(0);
        s.dom_count[0] = 6; // CAB..: 6 candidates, 2 empty cells (3 per cell)
        s.dom_count[1] = 10; // .....: 10 candidates, 5 empty cells (2 per cell)
        s.slot_order = SlotOrder::Mrv;
        assert_eq!(s.select_entry(), Some(0));
    }

    #[test]
    fn per_cell_mrv_weighs_candidates_by_empty_cells() {
        let (p, wl) = two_rows();
        let mut s = Solver::new(&p, &wl);
        s.reset(0);
        s.dom_count[0] = 6;
        s.dom_count[1] = 10;
        s.slot_order = SlotOrder::PerCell;
        assert_eq!(s.select_entry(), Some(1), "2 per empty cell beats 3");
    }

    /// 3x3 open grid with an isolated 2-letter entry: after placing the top
    /// across, "linked" must pick an entry crossing it even though the
    /// isolated one has fewer candidates.
    #[test]
    fn linked_prefers_entries_crossing_the_filled_part() {
        let p = Puzzle::from_template("...#..\n...###\n...###\n");
        let wl = Wordlist::from_str("CAB;80\nORE;70\nTEN;75\nCOT;60\nARE;65\nBEN;55\nXY;50\n", 0);
        let mut s = Solver::new(&p, &wl);
        s.reset(0);
        let top = p.find_entry(0, 0, Dir::Across).unwrap();
        let iso = p.find_entry(0, 4, Dir::Across).unwrap();
        let cab = wl.len_data(3).index_of(&[2, 0, 1]).unwrap();
        let _f = s.assign(top, cab);
        // make the isolated 2-letter entry the most urgent by count
        for ei in 0..p.entries.len() {
            if ei != top && ei != iso {
                s.dom_count[ei] = 5;
            }
        }
        s.dom_count[iso] = 1;
        s.linked = false;
        assert_eq!(s.select_entry(), Some(iso));
        s.linked = true;
        let pick = s.select_entry().unwrap();
        assert!(
            s.crossings[pick].iter().any(|&(_, o, _)| o == top),
            "linked pick must cross the placed word"
        );
    }

    #[test]
    fn slot_order_options() {
        let args: Vec<String> = ["--slot-order", "per-cell", "--linked", "on"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let cfg = SolveConfig::default().with_args(&args).unwrap();
        assert_eq!(cfg.slot_order, SlotOrder::PerCell);
        assert!(cfg.linked);
        assert!(
            cfg.describe().contains("slot-order per-cell") && cfg.describe().contains("linked on")
        );
        let bad: Vec<String> = vec!["--slot-order".into(), "boh".into()];
        assert!(SolveConfig::default().with_args(&bad).is_err());
        let bad: Vec<String> = vec!["--linked".into(), "boh".into()];
        assert!(SolveConfig::default().with_args(&bad).is_err());
    }

    // ---- 2b: cache delle lettere ammesse ----

    /// The cached allowed-letter masks must always equal a from-scratch
    /// recomputation, through random assignments, propagation and undo.
    #[test]
    fn letter_cache_matches_recomputation() {
        let p = Puzzle::from_template("....\n.#..\n....\n..#.\n");
        let mut rng = Rng::new(9);
        let mut src = String::new();
        for len in 2..=4 {
            for _ in 0..60 {
                let w: String = (0..len)
                    .map(|_| (b'A' + rng.below(5) as u8) as char)
                    .collect();
                src.push_str(&format!("{w};60\n"));
            }
        }
        let wl = Wordlist::from_str(&src, 0);
        let mut s = Solver::new(&p, &wl);
        s.propagation = Propagation::Arc;
        s.reset(0);
        let check = |s: &Solver| {
            for e in 0..s.p.entries.len() {
                for pos in 0..s.p.entries[e].len {
                    assert_eq!(
                        s.letter_mask(e, pos),
                        s.letters_at(e, pos),
                        "entry {e} pos {pos}"
                    );
                }
            }
        };
        check(&s);
        assert!(s.propagate_all());
        check(&s);
        for _ in 0..50 {
            let mut frames = Vec::new();
            for _ in 0..3 {
                let Some(e) = s.select_entry() else { break };
                let cands = s.ordered_candidates(e);
                let Some(&w) = cands.get(rng.below(cands.len().max(1))) else {
                    break;
                };
                let mut f = s.assign(e, w);
                let _ = s.propagate(&mut f);
                check(&s);
                frames.push(f);
            }
            while let Some(f) = frames.pop() {
                s.undo(f);
                check(&s);
            }
        }
    }

    // ---- 3b: infattibilità dimostrata ----

    /// A complete (untruncated) exhaustive attempt proves the grid
    /// unfillable at that floor: solve must stop instead of restarting
    /// until the deadline.
    #[test]
    fn proven_infeasible_stops_restarting() {
        let p = Puzzle::from_template("..\n..\n");
        let wl = Wordlist::from_str("AB;60\nAC;60\nBA;60\nBD;60\nDA;60\n", 0);
        let mut s = Solver::new(&p, &wl);
        let r = s.solve(&SolveConfig {
            time_limit_s: 3.0,
            ..Default::default()
        });
        assert_eq!(r.reason, "unsolved");
        assert_eq!(r.restarts, 1, "one complete attempt is a proof");
        assert!(r.elapsed_s < 0.5);
    }

    // ---- punto 5: lookahead sui candidati ----

    /// "?AB" across, its first cell starts the down "??E". X is a frequent
    /// initial in the list, so static ordering prefers XAB — but only XAE
    /// fits the down after it, against five Z?E words after ZAB.
    fn lookahead_case() -> (Puzzle, Wordlist) {
        let p = Puzzle::from_template(".AB\n.##\nE##\n");
        let mut src = String::from("XAB;60\nZAB;60\nQAB;60\nXAE;60\n");
        for w in ["ZAE", "ZBE", "ZCE", "ZDE", "ZFE"] {
            src.push_str(&format!("{w};60\n"));
        }
        for w in [
            "XQQ", "XRR", "XSS", "XTT", "XUU", "XVV", "XWW", "XYY", "XKK", "XJJ",
        ] {
            src.push_str(&format!("{w};60\n"));
        }
        (p, Wordlist::from_str(&src, 0))
    }

    #[test]
    fn lookahead_orders_by_real_crossing_options() {
        let (p, wl) = lookahead_case();
        let across = p.find_entry(0, 0, Dir::Across).unwrap();
        let xab = wl.len_data(3).index_of(&[23, 0, 1]).unwrap();
        let zab = wl.len_data(3).index_of(&[25, 0, 1]).unwrap();
        let mut s = Solver::new(&p, &wl);
        s.reset(0);
        assert!(s.propagate_all());
        s.lookahead = 0;
        assert_eq!(
            s.ordered_candidates(across)[0],
            xab,
            "static order prefers X"
        );
        s.lookahead = 8;
        assert_eq!(s.ordered_candidates(across)[0], zab, "lookahead prefers Z");
    }

    #[test]
    fn lookahead_drops_candidates_that_empty_a_crossing() {
        let (p, wl) = lookahead_case();
        let across = p.find_entry(0, 0, Dir::Across).unwrap();
        let qab = wl.len_data(3).index_of(&[16, 0, 1]).unwrap();
        let mut s = Solver::new(&p, &wl);
        s.propagation = Propagation::Forward; // arc would prune QAB up front
        s.reset(0);
        s.lookahead = 0;
        assert!(s.ordered_candidates(across).contains(&qab));
        s.lookahead = 8;
        assert!(
            !s.ordered_candidates(across).contains(&qab),
            "no Q?E down exists"
        );
    }

    #[test]
    fn lookahead_option() {
        let args: Vec<String> = vec!["--lookahead".into(), "12".into()];
        let cfg = SolveConfig::default().with_args(&args).unwrap();
        assert_eq!(cfg.lookahead, 12);
        assert!(cfg.describe().contains("lookahead 12"));
        let bad: Vec<String> = vec!["--lookahead".into(), "tanti".into()];
        assert!(SolveConfig::default().with_args(&bad).is_err());
    }

    // ---- punto 6: analisi dello schema ----

    #[test]
    fn analysis_reports_impossible_slot() {
        // row 0 "QQ." needs a 3-letter word starting QQ: none exists
        let p = Puzzle::from_template("QQ.\n...\n...\n");
        let wl = dict();
        let a = Solver::new(&p, &wl).analyze();
        assert!(!a.feasible);
        let top = p.find_entry(0, 0, Dir::Across).unwrap();
        assert!(a.impossible.contains(&top));
    }

    #[test]
    fn analysis_finds_forced_letters_by_propagation() {
        // "?AB" across + "??E" down; the only ??E word is XAE, so the shared
        // cell must be X and the cell below it A.
        let p = Puzzle::from_template(".AB\n.##\nE##\n");
        let wl = Wordlist::from_str("XAB;60\nZAB;60\nXAE;60\n", 0);
        let a = Solver::new(&p, &wl).analyze();
        assert!(a.feasible);
        assert!(a.forced.contains(&(0, 0, b'X')), "{:?}", a.forced);
        assert!(a.forced.contains(&(1, 0, b'A')), "{:?}", a.forced);
        let across = p.find_entry(0, 0, Dir::Across).unwrap();
        assert_eq!(a.candidates[across], 1, "ZAB is ruled out");
    }

    #[test]
    fn analysis_of_an_easy_grid() {
        let p = Puzzle::from_template("...\n...\n...\n");
        // every 3-letter string over A, B, C: nothing can be forced
        let mut src = String::new();
        for a in b"ABC" {
            for b in b"ABC" {
                for c in b"ABC" {
                    src.push_str(&format!("{}{}{};60\n", *a as char, *b as char, *c as char));
                }
            }
        }
        let wl = Wordlist::from_str(&src, 0);
        let a = Solver::new(&p, &wl).analyze();
        assert!(a.feasible);
        assert!(a.impossible.is_empty());
        assert!(a.forced.is_empty());
        assert!(a.cell_letters.iter().all(|&m| m.count_ones() > 1));
    }

    // ---- punto 8: sottostringhe ripetute ----

    fn shares_kgram(a: &str, b: &str, k: usize) -> bool {
        a.len() >= k && (0..=a.len() - k).any(|i| b.contains(&a[i..i + k]))
    }

    /// Two independent entries; the best-scored pair ARTE + PARTE shares the
    /// 4-gram ARTE. With the option on, that pair is forbidden.
    #[test]
    fn avoid_substrings_forbids_shared_kgrams() {
        let p = Puzzle::from_template("....#.....\n");
        let wl = Wordlist::from_str("ARTE;90\nOSSO;50\nPARTE;90\nLUCIO;50\n", 0);
        let words = |r: &SolveResult| -> Vec<String> {
            r.fill
                .as_ref()
                .unwrap()
                .iter()
                .map(|(_, w, _)| w.clone())
                .collect()
        };
        let base = fast_cfg();
        let r = Solver::new(&p, &wl).solve(&base);
        let w = words(&r);
        assert!(
            shares_kgram(&w[0], &w[1], 4),
            "without the option ARTE+PARTE wins: {w:?}"
        );
        let r = Solver::new(&p, &wl).solve(&SolveConfig {
            avoid_substrings: 4,
            ..base
        });
        assert_eq!(r.reason, "solved");
        let w = words(&r);
        assert!(
            !shares_kgram(&w[0], &w[1], 4) && !shares_kgram(&w[1], &w[0], 4),
            "{w:?}"
        );
    }

    /// Backjumping must stay sound when candidates are rejected for repeated
    /// substrings: same verdict as chronological search on random cases.
    #[test]
    fn avoid_substrings_keeps_backjumping_sound() {
        let mut rng = Rng::new(77);
        // independent blocks: without the substring rule backjumping jumps
        // freely between them; with it the blocks are coupled, so an
        // incomplete conflict set would jump over relevant choices
        let letters = b"ABCD";
        let templates = [
            "..#..\n..#..\n",
            "...#...\n...#...\n",
            "..#..#..\n..#..#..\n",
            "...#..\n...#..\n",
        ];
        let mut differing_paths = 0;
        for round in 0..400 {
            let t = templates[round % templates.len()];
            let mut src = String::new();
            for len in 2..=5 {
                for _ in 0..(10 + rng.below(25)) {
                    let w: String = (0..len)
                        .map(|_| letters[rng.below(letters.len())] as char)
                        .collect();
                    src.push_str(&format!("{w};60\n"));
                }
            }
            let wl = Wordlist::from_str(&src, 0);
            let p = Puzzle::from_template(t);
            if !wl.missing_lengths(&p).is_empty() {
                continue;
            }
            let run = |bj: bool| {
                let mut s = Solver::new(&p, &wl);
                s.backjump = bj;
                s.avoid_substrings = 2;
                s.cand_cap = usize::MAX;
                let far = Instant::now() + std::time::Duration::from_secs(60);
                (s.attempt(0, u64::MAX, 3, far), s.nodes)
            };
            let (a, na) = run(false);
            let (b, nb) = run(true);
            assert_eq!(a, b, "round {round}: {t:?} {src:?}");
            differing_paths += (na != nb) as usize;
        }
        assert!(
            differing_paths > 20,
            "backjumping must actually kick in: {differing_paths}"
        );
    }

    /// R1 ("..." top-left) has XAB (tried first) and XYZ; the independent
    /// 2x2 R2 can only be filled using AB. With 2-letter substrings banned,
    /// XAB blocks R2: backjumping must see that R2's failure depends on the
    /// R1 choice and try XYZ instead of declaring the grid unfillable.
    #[test]
    fn avoid_substrings_conflict_includes_the_blocking_word() {
        let p = Puzzle::from_template("...#..\n####..\n");
        let wl = Wordlist::from_str("XAB;90\nXYZ;60\nAB;60\nCD;60\nAC;60\nBD;60\n", 0);
        for bj in [false, true] {
            let mut s = Solver::new(&p, &wl);
            s.backjump = bj;
            s.avoid_substrings = 2;
            s.cand_cap = usize::MAX;
            let far = Instant::now() + std::time::Duration::from_secs(60);
            assert_eq!(
                s.attempt(0, u64::MAX, 3, far),
                Outcome::Solved,
                "backjump {bj}"
            );
        }
    }

    #[test]
    fn avoid_substrings_option() {
        let args: Vec<String> = vec!["--avoid-substrings".into(), "4".into()];
        let cfg = SolveConfig::default().with_args(&args).unwrap();
        assert_eq!(cfg.avoid_substrings, 4);
        assert!(cfg.describe().contains("avoid-substrings 4"));
        assert_eq!(SolveConfig::default().avoid_substrings, 0, "off by default");
        let bad: Vec<String> = vec!["--avoid-substrings".into(), "x".into()];
        assert!(SolveConfig::default().with_args(&bad).is_err());
    }

    // ---- punto 9: ricerca parallela ----

    #[test]
    fn cancel_flag_stops_the_search() {
        let p = Puzzle::from_template("...\n...\n...\n");
        let wl = dict();
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let r = Solver::new(&p, &wl).solve(&SolveConfig {
            time_limit_s: 5.0,
            cancel: Some(flag),
            ..Default::default()
        });
        assert_eq!(r.reason, "unsolved", "cancelled before it could start");
        assert!(r.elapsed_s < 0.5);
    }

    #[test]
    fn portfolio_combines_workers() {
        let p = Puzzle::from_template("...\n...\n...\n");
        let wl = dict();
        let cfg = SolveConfig {
            time_limit_s: 0.3,
            ..Default::default()
        };
        let r = solve_portfolio(&p, &wl, &[], &cfg, 3).unwrap();
        assert_eq!(r.reason, "solved");
        assert!(r.restarts >= 3, "restarts are summed over the 3 workers");
        // first-solution mode: the winner stops the others right away
        let fast = SolveConfig {
            time_limit_s: 10.0,
            stop_on_first: true,
            ..Default::default()
        };
        let r = solve_portfolio(&p, &wl, &[], &fast, 4).unwrap();
        assert_eq!(r.reason, "solved");
        assert!(r.elapsed_s < 2.0);
        let bad = vec![Lock {
            row: 9,
            col: 9,
            dir: Dir::Across,
            answer: "CAB".into(),
        }];
        assert!(solve_portfolio(&p, &wl, &bad, &cfg, 2).is_err());
    }

    // ---- punto 10: riparazione delle parole deboli ----

    /// Italian scale (default config: clean floor 50, weak bar 58). The fill
    /// leans on ORE;50 — admissible but weak. Ripping it with its crossings
    /// and refilling the region at the weak bar gives UXU;60/CUT/AXE/BUN:
    /// no weak entry left.
    #[test]
    fn repair_targets_weak_entries() {
        let wl = repair_dict(90);
        let p = Puzzle::from_template("...\n...\n...\n");
        let s = Solver::new(&p, &wl);
        let b = dirty_fill(&p);
        let cfg = SolveConfig {
            time_limit_s: 2.0,
            ..Default::default()
        };
        assert_eq!((cfg.clean_floor, cfg.weak_bar), (50, 58));
        let rep = s.repair(&b, &cfg).expect("weak entry must be repaired");
        assert_eq!(rep.weak_count, 0);
        assert!(rep.fill.iter().all(|(_, w, _)| w != "ORE"));
        assert!(rep.min_score >= 58);
    }

    /// With the default config every fill is "clean" (min >= 50), so the
    /// clean track usually IS the mean-best fill: the repair phase must still
    /// try it, or weak entries would never be repaired.
    #[test]
    fn repair_phase_repairs_the_clean_fill_too() {
        let wl = repair_dict(90);
        let p = Puzzle::from_template("...\n...\n...\n");
        let s = Solver::new(&p, &wl);
        let b = dirty_fill(&p);
        let cfg = SolveConfig {
            time_limit_s: 2.0,
            ..Default::default()
        };
        let best = Some(b.clone());
        let mut clean = Some(b);
        s.repair_phase(&best, &mut clean, &cfg);
        assert_eq!(clean.unwrap().weak_count, 0);
    }

    /// Two independent 3x3 blocks, each leaning on one weak word (ORE and
    /// its letter-substituted twin KLI). With at most 1 weak entry per
    /// repair, the all-at-once repair gives up; repairing one weak entry at a
    /// time (greedy) fixes both.
    #[test]
    fn greedy_repair_fixes_weak_entries_one_at_a_time() {
        let sub = |w: &str| -> String {
            w.chars()
                .map(|c| match c {
                    'A' => 'F',
                    'B' => 'G',
                    'C' => 'H',
                    'E' => 'I',
                    'N' => 'J',
                    'O' => 'K',
                    'R' => 'L',
                    'T' => 'M',
                    'U' => 'P',
                    'X' => 'Q',
                    o => o,
                })
                .collect()
        };
        let block = [
            ("CAB", 80),
            ("TEN", 75),
            ("ORE", 50),
            ("UXU", 60),
            ("COT", 90),
            ("ARE", 90),
            ("BEN", 90),
            ("CUT", 90),
            ("AXE", 90),
            ("BUN", 90),
        ];
        let mut src = String::new();
        for (w, sc) in block {
            src.push_str(&format!("{w};{sc}\n{};{sc}\n", sub(w)));
        }
        let wl = Wordlist::from_str(&src, 0);
        let p = Puzzle::from_template("...#...\n...#...\n...#...\n");
        let e = |r, c, d| p.find_entry(r, c, d).unwrap();
        let mut fill = Vec::new();
        for c0 in [0usize, 4] {
            let m = |w: &str| if c0 == 0 { w.to_string() } else { sub(w) };
            for (r, w, sc) in [(0, "CAB", 80u8), (1, "ORE", 50), (2, "TEN", 75)] {
                fill.push((e(r, c0, Dir::Across), m(w), sc));
            }
            for (c, w) in [(0, "COT"), (1, "ARE"), (2, "BEN")] {
                fill.push((e(0, c0 + c, Dir::Down), m(w), 90u8));
            }
        }
        let total: u32 = fill.iter().map(|(_, _, s)| *s as u32).sum();
        let b = SolvedFill {
            letters: Vec::new(),
            mean_score: total as f64 / fill.len() as f64,
            min_score: 50,
            weak_count: 2,
            fill,
        };
        let s = Solver::new(&p, &wl);
        let cfg = SolveConfig {
            time_limit_s: 2.0,
            repair_max_weak: 1,
            ..Default::default()
        };
        let rep = s
            .repair(&b, &cfg)
            .expect("each weak entry is repairable on its own");
        assert_eq!(rep.weak_count, 0);
    }

    /// Two fills of the open 3x3: CAB/ORE/TEN × COT/ARE/BEN (BEN;55 is
    /// weak) and PAW/IRE/TEN × PIT/ARE/WEN (no weak entry). From BEN, radius
    /// 1 keeps COT and ARE locked, which pins the old fill; radius 2 frees the
    /// whole grid and reaches the clean one.
    #[test]
    fn repair_region_radius_two_frees_pinned_words() {
        let wl = Wordlist::from_str(
            "CAB;80\nORE;70\nTEN;75\nCOT;60\nARE;65\nBEN;55\nPAW;90\nIRE;90\nPIT;90\nWEN;60\n",
            0,
        );
        let p = Puzzle::from_template("...\n...\n...\n");
        let e = |r, c, d| p.find_entry(r, c, d).unwrap();
        let fill = vec![
            (e(0, 0, Dir::Across), "CAB".to_string(), 80u8),
            (e(1, 0, Dir::Across), "ORE".to_string(), 70),
            (e(2, 0, Dir::Across), "TEN".to_string(), 75),
            (e(0, 0, Dir::Down), "COT".to_string(), 60),
            (e(0, 1, Dir::Down), "ARE".to_string(), 65),
            (e(0, 2, Dir::Down), "BEN".to_string(), 55),
        ];
        let b = SolvedFill {
            letters: Vec::new(),
            mean_score: 405.0 / 6.0,
            min_score: 55,
            weak_count: 1,
            fill,
        };
        let s = Solver::new(&p, &wl);
        let cfg = SolveConfig::default();
        let ben = e(0, 2, Dir::Down);
        let r1 = s.repair_region(&b, &cfg, &[ben], 1, 0.5);
        assert!(
            r1.is_none_or(|r| r.weak_count == 1),
            "radius 1 cannot drop BEN"
        );
        let r2 = s
            .repair_region(&b, &cfg, &[ben], 2, 0.5)
            .expect("radius 2 refills");
        assert_eq!(r2.weak_count, 0);
    }

    #[test]
    fn lock_errors_on_bad_spec() {
        let p = Puzzle::from_template("...\n...\n...\n");
        let wl = dict();
        // wrong length
        let bad = vec![Lock {
            row: 0,
            col: 0,
            dir: Dir::Across,
            answer: "TOOLONG".into(),
        }];
        assert!(Solver::with_locks(&p, &wl, &bad).is_err());
        // no entry there
        let bad = vec![Lock {
            row: 9,
            col: 9,
            dir: Dir::Across,
            answer: "CAB".into(),
        }];
        assert!(Solver::with_locks(&p, &wl, &bad).is_err());
    }
}
