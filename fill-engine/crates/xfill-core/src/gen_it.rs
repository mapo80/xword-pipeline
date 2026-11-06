//! Italian-style block-pattern generator: rectangular `rows × cols`, a target
//! number of black squares, optional symmetry.
//!
//! Rules (Italian convention, unlike `gen`'s NYT rules):
//!   * entries are runs of >= 2 letters; a 1-letter run is allowed and simply
//!     unclued, but every white cell must belong to at least one entry
//!     (no cell isolated in both directions);
//!   * all white cells form one orthogonally connected region;
//!   * no 2×2 square of blacks;
//!   * no run longer than `max_len` (keeps entries inside the word list).
//!
//! Construction is randomized and incremental: each step blackens a white cell
//! (plus its symmetric partners) drawn with probability growing with the
//! length of the runs through it, so long runs get broken first and blacks
//! spread out. A step that would violate a rule is rolled back. The caller
//! retries with fresh randomness when a pattern can't be completed — and,
//! one level up, when a completed pattern can't be filled.

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

#[derive(Clone, Debug)]
pub struct ItGenConfig {
    pub rows: usize,
    pub cols: usize,
    pub blacks: usize,
    pub symmetry: Symmetry,
    pub max_len: usize,
    /// Allow two blacks to touch orthogonally (still never a 2×2 block).
    pub allow_adjacent: bool,
}

struct Pattern {
    rows: usize,
    cols: usize,
    black: Vec<bool>,
}

impl Pattern {
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
    let mut p = Pattern {
        rows: cfg.rows,
        cols: cfg.cols,
        black: vec![false; cfg.rows * cfg.cols],
    };
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
        let ok = cells.iter().all(|&(a, b)| !p.black_square_at(a, b))
            && (cfg.allow_adjacent || cells.iter().all(|&(a, b)| !p.has_black_neighbor(a, b)))
            && {
                let runs = p.runs();
                (0..n).all(|i| p.black[i] || runs[i].0 >= 2 || runs[i].1 >= 2)
            }
            && p.whites_connected();
        if ok {
            placed += cells.len();
        } else {
            for &(a, b) in &cells {
                let i = p.idx(a, b);
                p.black[i] = false;
            }
            stalls += 1;
        }
    }
    if placed + 1 < cfg.blacks {
        return None;
    }
    let runs = p.runs();
    if runs
        .iter()
        .any(|&(h, v)| h > cfg.max_len || v > cfg.max_len)
    {
        return None;
    }
    Some(p.to_template())
}

/// Generate a valid pattern, trying up to `max_tries` random walks.
pub fn generate_it(cfg: &ItGenConfig, rng: &mut Rng, max_tries: usize) -> Option<String> {
    (0..max_tries).find_map(|_| try_once(cfg, rng))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Puzzle;

    fn cfg(rows: usize, cols: usize, blacks: usize, symmetry: Symmetry) -> ItGenConfig {
        ItGenConfig {
            rows,
            cols,
            blacks,
            symmetry,
            max_len: 13,
            allow_adjacent: true,
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
}
