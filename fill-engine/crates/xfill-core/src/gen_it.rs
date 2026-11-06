//! Italian-style block-pattern generator: rectangular `rows × cols`, a target
//! number of black squares, optional symmetry.
//!
//! Rules (Italian convention):
//!   * entries are runs of >= 2 letters; a 1-letter run is allowed and simply
//!     unclued, but every white cell must belong to at least one entry
//!     (no cell isolated in both directions);
//!   * all white cells form one orthogonally connected region;
//!   * no 2×2 square of blacks;
//!   * no run longer than `max_len` (keeps entries inside the word list).
//!
//! Optionally: at most a given percentage of 2-letter entries.
//!
//! Two construction modes (see [`GenMode`]):
//!   * `Random`: a randomized walk that blackens a white cell (plus its
//!     symmetric partners) drawn with probability growing with the length of
//!     the runs through it; a step that breaks a rule is rolled back, a stuck
//!     walk is abandoned.
//!   * `Search`: depth-first search over symmetry orbits with the same
//!     preference order, backtracking on dead ends, a memo of visited black
//!     sets and a lower bound on the blacks still needed to break over-long
//!     runs. Every rule except the 2-letter limit only gets harder as blacks
//!     are added, so pruning on them is exact: with no node budget the search
//!     finds a pattern whenever one exists.

use crate::util::Rng;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Symmetry {
    None,
    /// 180° rotation (classic).
    Rotational,
    /// Left–right mirror.
    Mirror,
}

impl Symmetry {
    pub fn parse(s: &str) -> Option<Symmetry> {
        match s.to_ascii_lowercase().as_str() {
            "none" | "nessuna" => Some(Symmetry::None),
            "rot" | "rotational" | "rotazionale" => Some(Symmetry::Rotational),
            "mirror" | "speculare" => Some(Symmetry::Mirror),
            _ => None,
        }
    }
}

/// How patterns are built (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenMode {
    Random,
    Search,
}

impl GenMode {
    pub fn parse(s: &str) -> Option<GenMode> {
        match s {
            "random" => Some(GenMode::Random),
            "search" => Some(GenMode::Search),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ItGenConfig {
    pub rows: usize,
    pub cols: usize,
    pub blacks: usize,
    pub symmetry: Symmetry,
    pub max_len: usize,
    /// Allow two blacks to touch orthogonally (still never a 2×2 block).
    pub allow_adjacent: bool,
    pub mode: GenMode,
    /// At most this percentage of the entries may have 2 letters.
    pub max_two_letter_pct: Option<u8>,
    /// Search mode: node budget per try (0 = unlimited).
    pub search_budget: usize,
}

impl ItGenConfig {
    /// Defaults: rotational symmetry, runs up to the longer side, touching
    /// blacks allowed, random mode, no 2-letter limit.
    pub fn new(rows: usize, cols: usize, blacks: usize) -> ItGenConfig {
        ItGenConfig {
            rows,
            cols,
            blacks,
            symmetry: Symmetry::Rotational,
            max_len: rows.max(cols),
            allow_adjacent: true,
            mode: GenMode::Random,
            max_two_letter_pct: None,
            search_budget: 20_000,
        }
    }
}

struct Pattern {
    rows: usize,
    cols: usize,
    black: Vec<bool>,
}

impl Pattern {
    fn empty(rows: usize, cols: usize) -> Pattern {
        Pattern {
            rows,
            cols,
            black: vec![false; rows * cols],
        }
    }

    #[inline]
    fn idx(&self, r: usize, c: usize) -> usize {
        r * self.cols + c
    }

    #[inline]
    fn is_black(&self, r: usize, c: usize) -> bool {
        self.black[self.idx(r, c)]
    }

    /// (horizontal run length, vertical run length) through each white cell;
    /// (0, 0) for blacks.
    fn runs(&self) -> Vec<(usize, usize)> {
        let (rows, cols) = (self.rows, self.cols);
        let mut out = vec![(0usize, 0usize); rows * cols];
        for r in 0..rows {
            let mut c = 0;
            while c < cols {
                if self.is_black(r, c) {
                    c += 1;
                    continue;
                }
                let s = c;
                while c < cols && !self.is_black(r, c) {
                    c += 1;
                }
                for k in s..c {
                    out[r * cols + k].0 = c - s;
                }
            }
        }
        for c in 0..cols {
            let mut r = 0;
            while r < rows {
                if self.is_black(r, c) {
                    r += 1;
                    continue;
                }
                let s = r;
                while r < rows && !self.is_black(r, c) {
                    r += 1;
                }
                for k in s..r {
                    out[k * cols + c].1 = r - s;
                }
            }
        }
        out
    }

    fn whites_connected(&self) -> bool {
        let n = self.rows * self.cols;
        let Some(start) = (0..n).find(|&i| !self.black[i]) else {
            return false;
        };
        let total = self.black.iter().filter(|b| !**b).count();
        let mut seen = vec![false; n];
        let mut stack = vec![start];
        seen[start] = true;
        let mut count = 0;
        while let Some(i) = stack.pop() {
            count += 1;
            let (r, c) = (i / self.cols, i % self.cols);
            let mut push = |j: usize| {
                if !self.black[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if r > 0 {
                push(i - self.cols);
            }
            if r + 1 < self.rows {
                push(i + self.cols);
            }
            if c > 0 {
                push(i - 1);
            }
            if c + 1 < self.cols {
                push(i + 1);
            }
        }
        count == total
    }

    /// Any 2×2 all-black square touching (r, c)?
    fn black_square_at(&self, r: usize, c: usize) -> bool {
        // top-left corners of the four 2×2 windows containing (r, c)
        let rs = [r.checked_sub(1), Some(r)];
        let cs = [c.checked_sub(1), Some(c)];
        for tr in rs.iter().flatten() {
            for tc in cs.iter().flatten() {
                let (tr, tc) = (*tr, *tc);
                if tr + 1 >= self.rows || tc + 1 >= self.cols {
                    continue;
                }
                if self.is_black(tr, tc)
                    && self.is_black(tr + 1, tc)
                    && self.is_black(tr, tc + 1)
                    && self.is_black(tr + 1, tc + 1)
                {
                    return true;
                }
            }
        }
        false
    }

    fn has_black_neighbor(&self, r: usize, c: usize) -> bool {
        (r > 0 && self.is_black(r - 1, c))
            || (r + 1 < self.rows && self.is_black(r + 1, c))
            || (c > 0 && self.is_black(r, c - 1))
            || (c + 1 < self.cols && self.is_black(r, c + 1))
    }

    fn to_template(&self) -> String {
        let mut s = String::with_capacity(self.rows * (self.cols + 1));
        for r in 0..self.rows {
            for c in 0..self.cols {
                s.push(if self.is_black(r, c) { '#' } else { '.' });
            }
            s.push('\n');
        }
        s
    }
}

fn orbit(cfg: &ItGenConfig, r: usize, c: usize) -> Vec<(usize, usize)> {
    let mut v = vec![(r, c)];
    let partner = match cfg.symmetry {
        Symmetry::None => None,
        Symmetry::Rotational => Some((cfg.rows - 1 - r, cfg.cols - 1 - c)),
        Symmetry::Mirror => Some((r, cfg.cols - 1 - c)),
    };
    if let Some(p) = partner {
        if p != (r, c) {
            v.push(p);
        }
    }
    v
}

/// Every symmetry orbit once (cells in row-major order of first appearance).
fn orbit_list(cfg: &ItGenConfig) -> Vec<Vec<(usize, usize)>> {
    let mut seen = vec![false; cfg.rows * cfg.cols];
    let mut out = Vec::new();
    for r in 0..cfg.rows {
        for c in 0..cfg.cols {
            if seen[r * cfg.cols + c] {
                continue;
            }
            let o = orbit(cfg, r, c);
            for &(a, b) in &o {
                seen[a * cfg.cols + b] = true;
            }
            out.push(o);
        }
    }
    out
}

/// Lower bound on the blacks still needed so that no run exceeds `max_len`:
/// a run of length L needs ceil((L - max_len) / (max_len + 1)) blacks; one
/// black splits one row run and one column run, hence the max of the sums.
fn min_blacks_needed(p: &Pattern, max_len: usize) -> usize {
    let need = |l: usize| {
        if l > max_len {
            (l - max_len).div_ceil(max_len + 1)
        } else {
            0
        }
    };
    let (mut rows, mut cols) = (0, 0);
    for r in 0..p.rows {
        let mut run = 0;
        for c in 0..=p.cols {
            if c < p.cols && !p.is_black(r, c) {
                run += 1;
            } else {
                rows += need(run);
                run = 0;
            }
        }
    }
    for c in 0..p.cols {
        let mut run = 0;
        for r in 0..=p.rows {
            if r < p.rows && !p.is_black(r, c) {
                run += 1;
            } else {
                cols += need(run);
                run = 0;
            }
        }
    }
    rows.max(cols)
}

/// The rules that only get harder as blacks are added, checked after
/// placing `cells`: no 2×2 block, touching blacks if forbidden, no isolated
/// white cell, whites connected.
fn local_ok(cfg: &ItGenConfig, p: &Pattern, cells: &[(usize, usize)]) -> bool {
    cells.iter().all(|&(a, b)| !p.black_square_at(a, b))
        && (cfg.allow_adjacent || cells.iter().all(|&(a, b)| !p.has_black_neighbor(a, b)))
        && {
            let runs = p.runs();
            (0..p.black.len()).all(|i| p.black[i] || runs[i].0 >= 2 || runs[i].1 >= 2)
        }
        && p.whites_connected()
}

/// Full validity of a finished pattern (every rule, whole grid).
fn complete_ok(cfg: &ItGenConfig, p: &Pattern) -> bool {
    let all: Vec<(usize, usize)> = (0..p.rows)
        .flat_map(|r| (0..p.cols).map(move |c| (r, c)))
        .filter(|&(r, c)| p.is_black(r, c))
        .collect();
    if !local_ok(cfg, p, &all) {
        return false;
    }
    let runs = p.runs();
    if runs
        .iter()
        .any(|&(h, v)| h > cfg.max_len || v > cfg.max_len)
    {
        return false;
    }
    match cfg.max_two_letter_pct {
        None => true,
        Some(pct) => {
            let (mut entries, mut two) = (0usize, 0usize);
            for r in 0..p.rows {
                for c in 0..p.cols {
                    if p.is_black(r, c) {
                        continue;
                    }
                    let (h, v) = runs[p.idx(r, c)];
                    // count each run once, at its first cell
                    if h >= 2 && (c == 0 || p.is_black(r, c - 1)) {
                        entries += 1;
                        two += (h == 2) as usize;
                    }
                    if v >= 2 && (r == 0 || p.is_black(r - 1, c)) {
                        entries += 1;
                        two += (v == 2) as usize;
                    }
                }
            }
            two * 100 <= pct as usize * entries
        }
    }
}

fn unit(rng: &mut Rng) -> f64 {
    (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64
}

/// Validate a full pattern against the Italian rules (used for user-supplied
/// templates too). Returns a human-readable reason on failure.
pub fn check_template(template: &str, max_len: usize) -> Result<(), String> {
    let rows: Vec<Vec<char>> = template
        .lines()
        .map(|l| l.trim().replace(' ', ""))
        .filter(|l| !l.is_empty())
        .map(|l| l.chars().collect())
        .collect();
    if rows.is_empty() {
        return Err("empty template".into());
    }
    let cols = rows[0].len();
    if rows.iter().any(|r| r.len() != cols) {
        return Err("rows have different lengths".into());
    }
    let p = Pattern {
        rows: rows.len(),
        cols,
        black: rows.iter().flatten().map(|&ch| ch == '#').collect(),
    };
    if !p.whites_connected() {
        return Err("white cells are not all connected".into());
    }
    let runs = p.runs();
    for r in 0..p.rows {
        for c in 0..p.cols {
            if p.is_black(r, c) {
                continue;
            }
            let (h, v) = runs[p.idx(r, c)];
            if h < 2 && v < 2 {
                return Err(format!("cell r{r}c{c} belongs to no word (isolated)"));
            }
            if h > max_len || v > max_len {
                return Err(format!(
                    "run through r{r}c{c} is longer than {max_len} letters"
                ));
            }
        }
    }
    Ok(())
}

/// One attempt at building a pattern. `None` if this random walk got stuck.
fn try_once(cfg: &ItGenConfig, rng: &mut Rng) -> Option<String> {
    let mut p = Pattern::empty(cfg.rows, cfg.cols);
    let mut placed = 0usize;
    let mut stalls = 0usize;
    let n = cfg.rows * cfg.cols;
    while placed < cfg.blacks && stalls < 4 * n {
        let runs = p.runs();
        // weight: long runs first; discourage (or forbid) touching blacks
        let mut weights = vec![0f64; n];
        let mut total = 0.0;
        for r in 0..cfg.rows {
            for c in 0..cfg.cols {
                if p.is_black(r, c) {
                    continue;
                }
                let (h, v) = runs[p.idx(r, c)];
                let mut w = (h.max(v) as f64).powi(3) + 0.5 * ((h + v) as f64);
                if p.has_black_neighbor(r, c) {
                    if !cfg.allow_adjacent {
                        continue;
                    }
                    w *= 0.15;
                }
                weights[p.idx(r, c)] = w;
                total += w;
            }
        }
        if total <= 0.0 {
            return None;
        }
        let mut x = unit(rng) * total;
        let mut pick = 0;
        for (i, &w) in weights.iter().enumerate() {
            if w <= 0.0 {
                continue;
            }
            pick = i;
            if x < w {
                break;
            }
            x -= w;
        }
        let (r, c) = (pick / cfg.cols, pick % cfg.cols);
        let cells = orbit(cfg, r, c);
        if placed + cells.len() > cfg.blacks + 1 || cells.iter().any(|&(a, b)| p.is_black(a, b)) {
            stalls += 1;
            continue;
        }
        for &(a, b) in &cells {
            let i = p.idx(a, b);
            p.black[i] = true;
        }
        if local_ok(cfg, &p, &cells) {
            placed += cells.len();
        } else {
            for &(a, b) in &cells {
                let i = p.idx(a, b);
                p.black[i] = false;
            }
            stalls += 1;
        }
    }
    if placed + 1 < cfg.blacks || !complete_ok(cfg, &p) {
        return None;
    }
    Some(p.to_template())
}

/// Depth-first search over symmetry orbits (see the module docs).
struct Search<'a> {
    cfg: &'a ItGenConfig,
    rng: &'a mut Rng,
    orbits: Vec<Vec<(usize, usize)>>,
    visited: std::collections::HashSet<Vec<bool>>,
    nodes: usize,
}

/// `Err(())` = node budget exhausted.
type Found = Result<Option<String>, ()>;

impl Search<'_> {
    fn dfs(&mut self, p: &mut Pattern, placed: usize) -> Found {
        self.nodes += 1;
        if self.cfg.search_budget > 0 && self.nodes > self.cfg.search_budget {
            return Err(());
        }
        let blacks = self.cfg.blacks;
        if placed + 1 >= blacks && complete_ok(self.cfg, p) {
            return Ok(Some(p.to_template()));
        }
        if placed + min_blacks_needed(p, self.cfg.max_len) > blacks + 1 {
            return Ok(None);
        }
        // candidate orbits, longest runs first (randomized tie-breaking)
        let runs = p.runs();
        let mut cands: Vec<(f64, usize)> = Vec::new();
        for (k, o) in self.orbits.iter().enumerate() {
            let (r, c) = o[0];
            if p.is_black(r, c) || placed + o.len() > blacks + 1 {
                continue;
            }
            let (h, v) = runs[p.idx(r, c)];
            let mut w = (h.max(v) as f64).powi(3) + 0.5 * ((h + v) as f64);
            if p.has_black_neighbor(r, c) {
                w *= 0.15;
            }
            cands.push((w * (0.5 + unit(self.rng)), k));
        }
        cands.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        for (_, k) in cands {
            let cells = self.orbits[k].clone();
            for &(a, b) in &cells {
                let i = p.idx(a, b);
                p.black[i] = true;
            }
            let fresh = local_ok(self.cfg, p, &cells) && self.visited.insert(p.black.clone());
            let r = if fresh {
                self.dfs(p, placed + cells.len())
            } else {
                Ok(None)
            };
            for &(a, b) in &cells {
                let i = p.idx(a, b);
                p.black[i] = false;
            }
            match r {
                Ok(None) => {}
                other => return other,
            }
        }
        Ok(None)
    }
}

fn search_once(cfg: &ItGenConfig, rng: &mut Rng) -> Found {
    let mut s = Search {
        cfg,
        rng,
        orbits: orbit_list(cfg),
        visited: std::collections::HashSet::new(),
        nodes: 0,
    };
    let mut p = Pattern::empty(cfg.rows, cfg.cols);
    s.dfs(&mut p, 0)
}

/// Generate a valid pattern with up to `max_tries` tries (random walks, or
/// budgeted searches with fresh randomness). A search that completes
/// without a pattern proves none exists and stops early.
pub fn generate_it(cfg: &ItGenConfig, rng: &mut Rng, max_tries: usize) -> Option<String> {
    match cfg.mode {
        GenMode::Random => (0..max_tries).find_map(|_| try_once(cfg, rng)),
        GenMode::Search => {
            for _ in 0..max_tries {
                match search_once(cfg, rng) {
                    Ok(found) => return found,
                    Err(()) => continue, // budget: retry with new randomness
                }
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Puzzle;

    fn cfg(rows: usize, cols: usize, blacks: usize, symmetry: Symmetry) -> ItGenConfig {
        ItGenConfig {
            symmetry,
            max_len: 13,
            ..ItGenConfig::new(rows, cols, blacks)
        }
    }

    #[test]
    fn generates_valid_rectangular_patterns() {
        for (seed, sym) in [
            (1u64, Symmetry::None),
            (2, Symmetry::Rotational),
            (3, Symmetry::Mirror),
        ] {
            let c = cfg(9, 13, 18, sym);
            let mut rng = Rng::new(seed);
            let t = generate_it(&c, &mut rng, 200).expect("pattern");
            check_template(&t, c.max_len).expect("valid");
            let blacks = t.chars().filter(|&ch| ch == '#').count();
            assert!(blacks.abs_diff(18) <= 1, "blacks={blacks}");
            let p = Puzzle::from_template(&t);
            assert_eq!(p.orphan_cells(), 0);
            assert_eq!(p.rows, 9);
            assert_eq!(p.cols, 13);
        }
    }

    #[test]
    fn rotational_patterns_are_symmetric() {
        let c = cfg(11, 11, 20, Symmetry::Rotational);
        let mut rng = Rng::new(7);
        let t = generate_it(&c, &mut rng, 200).unwrap();
        let g: Vec<Vec<char>> = t.lines().map(|l| l.chars().collect()).collect();
        for r in 0..11 {
            for col in 0..11 {
                assert_eq!(g[r][col], g[10 - r][10 - col]);
            }
        }
    }

    #[test]
    fn check_template_rejects_isolated_and_long_runs() {
        assert!(check_template(".#.\n###\n.#.\n", 5).is_err()); // disconnected/isolated
        assert!(check_template("......\n......\n", 5).is_err()); // run of 6 > 5
        assert!(check_template("..#\n...\n#..\n", 5).is_ok());
    }

    // ---- punto 7: generatore a ricerca ----

    /// Does a valid pattern exist? Enumerate every subset of symmetry orbits.
    fn exists_by_brute_force(c: &ItGenConfig) -> bool {
        let orbits = orbit_list(c);
        let n = orbits.len();
        assert!(n <= 20, "brute force is for small grids");
        (0u32..(1 << n)).any(|mask| {
            let mut p = Pattern::empty(c.rows, c.cols);
            let mut placed = 0;
            for (i, o) in orbits.iter().enumerate() {
                if mask & (1 << i) != 0 {
                    for &(r, col) in o {
                        let k = p.idx(r, col);
                        p.black[k] = true;
                    }
                    placed += o.len();
                }
            }
            placed + 1 >= c.blacks && placed <= c.blacks + 1 && complete_ok(c, &p)
        })
    }

    #[test]
    fn search_finds_a_pattern_iff_one_exists() {
        let cases = [
            (4, 4, 2, 3, Symmetry::Rotational, true),
            (5, 5, 4, 3, Symmetry::Rotational, false),
            (5, 5, 6, 3, Symmetry::Rotational, true),
            (5, 5, 2, 3, Symmetry::Rotational, true),
            (5, 5, 5, 4, Symmetry::Mirror, true),
            (4, 5, 3, 4, Symmetry::Mirror, true),
            (4, 5, 2, 2, Symmetry::Mirror, true),
        ];
        for (rows, cols, blacks, max_len, symmetry, allow_adjacent) in cases {
            let c = ItGenConfig {
                symmetry,
                max_len,
                allow_adjacent,
                mode: GenMode::Search,
                search_budget: 0, // exhaustive
                ..ItGenConfig::new(rows, cols, blacks)
            };
            let exists = exists_by_brute_force(&c);
            let got = generate_it(&c, &mut Rng::new(5), 1);
            assert_eq!(
                got.is_some(),
                exists,
                "{rows}x{cols} blacks {blacks} max_len {max_len} {symmetry:?} adj {allow_adjacent}"
            );
            if let Some(t) = got {
                check_template(&t, max_len).expect("valid");
            }
        }
    }

    #[test]
    fn search_respects_two_letter_limit() {
        let c = ItGenConfig {
            mode: GenMode::Search,
            max_two_letter_pct: Some(5),
            ..ItGenConfig::new(9, 9, 12)
        };
        let t = generate_it(&c, &mut Rng::new(3), 20).expect("pattern");
        let p = Puzzle::from_template(&t);
        let two = p.entries.iter().filter(|e| e.len == 2).count();
        assert!(
            two * 100 <= 5 * p.entries.len(),
            "{two} of {}",
            p.entries.len()
        );
    }

    #[test]
    fn lower_bound_on_blacks_needed() {
        let p = Pattern::empty(5, 5);
        assert_eq!(
            min_blacks_needed(&p, 3),
            5,
            "each row and column of 5 needs one"
        );
        let p = Pattern::empty(1, 8);
        assert_eq!(min_blacks_needed(&p, 3), 2, "8 = 3 + 1 + 3 + 1");
        let p = Pattern::empty(3, 3);
        assert_eq!(min_blacks_needed(&p, 3), 0);
    }

    #[test]
    fn gen_mode_parse() {
        assert_eq!(GenMode::parse("search"), Some(GenMode::Search));
        assert_eq!(GenMode::parse("random"), Some(GenMode::Random));
        assert_eq!(GenMode::parse("x"), None);
    }
}
