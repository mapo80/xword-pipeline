//! Grid library: the JSON artifact written by the `library` binary (template,
//! fill, numbered answers with scores and clues) and the selection of the
//! best fills to keep.

use crate::grid::{Dir, Puzzle};
use crate::solver::{SolveResult, SolvedFill};
use std::collections::{HashMap, HashSet};

/// Default for `select_keepers`' per-shape cap: at most this many different
/// fills of one block pattern are kept. 0 lifts the cap.
pub const DEFAULT_PER_SHAPE: usize = 4;

pub struct LibEntry {
    pub num: u32,
    pub dir: char, // 'A' | 'D'
    pub row: usize,
    pub col: usize,
    pub len: usize,
    pub answer: String,
    pub score: u8,
    /// Clue text, filled in from the clue database after selection.
    pub clue: Option<String>,
    /// The answer is a theme word (see [`mark_theme`]).
    pub is_theme: bool,
}

pub struct LibGrid {
    pub blocks: usize,
    pub mean: f64,
    pub min: u8,
    /// Searched entries under the solver's weak bar (see
    /// `SolveConfig::weak_bar`).
    pub weak: usize,
    /// Entries whose answer is a theme word.
    pub theme: usize,
    pub template: Vec<String>,
    pub fill: Vec<String>,
    pub entries: Vec<LibEntry>,
}

/// Build a library record from a solve result, preferring the solver's clean
/// track (fewest weak entries, then mean) over the mean-best fill. Returns
/// None if the result isn't a solved fill.
pub fn build_lib_grid(p: &Puzzle, r: &SolveResult) -> Option<LibGrid> {
    if let Some(c) = &r.clean {
        return Some(from_fill(p, c));
    }
    let f = SolvedFill {
        letters: r.letters.clone()?,
        mean_score: r.mean_score?,
        min_score: r.min_score?,
        weak_count: r.weak_count.unwrap_or(0),
        theme_count: 0,
        fill: r.fill.clone()?,
    };
    Some(from_fill(p, &f))
}

fn from_fill(p: &Puzzle, f: &SolvedFill) -> LibGrid {
    let nums = p.number_entries();
    let mut entries: Vec<LibEntry> = f
        .fill
        .iter()
        .map(|(ei, ans, sc)| {
            let e = &p.entries[*ei];
            LibEntry {
                num: nums[*ei],
                dir: if e.dir == Dir::Across { 'A' } else { 'D' },
                row: e.row,
                col: e.col,
                len: e.len,
                answer: ans.clone(),
                score: *sc,
                clue: None,
                is_theme: false,
            }
        })
        .collect();
    entries.sort_by_key(|g| (g.num, g.dir));
    LibGrid {
        blocks: p.block_count(),
        mean: f.mean_score,
        min: f.min_score,
        weak: f.weak_count,
        theme: f.theme_count,
        template: p.render(None).lines().map(str::to_string).collect(),
        fill: p
            .render(Some(&f.letters))
            .lines()
            .map(str::to_string)
            .collect(),
        entries,
    }
}

/// Flag the entries of `g` whose answer is a theme word (`theme` holds
/// (length, word index) in `wl`) and recount `g.theme`.
pub fn mark_theme(
    g: &mut LibGrid,
    wl: &crate::wordlist::Wordlist,
    theme: &std::collections::HashSet<(usize, usize)>,
) {
    for e in &mut g.entries {
        let letters: Vec<u8> = e.answer.bytes().map(|b| b.wrapping_sub(b'A')).collect();
        e.is_theme = !theme.is_empty()
            && letters.len() == e.len
            && e.len < wl.by_len.len()
            && wl
                .len_data(e.len)
                .index_of(&letters)
                .is_some_and(|w| theme.contains(&(e.len, w)));
    }
    g.theme = g.entries.iter().filter(|e| e.is_theme).count();
}

/// Order kept grids for output and collapse duplicates: highest
/// `theme_weight` × theme words − weak entries first (without a weight:
/// fewest weak entries), mean breaking ties — the solver's own clean-track
/// ordering — then
/// drop exact-duplicate FILLS, then keep at most `per_shape` fills of any one
/// template (0 = unlimited). Callers truncate to their `top` afterwards.
///
/// The sort happens BEFORE the dedup so the survivors of each shape are its
/// best fills, not whichever workers finished first. That ordering matters
/// far more than it sounds: small symmetric grids have tiny shape spaces (a
/// 300-candidate 5×5 run contains five distinct templates; 7×7 about 23), so
/// the old template-level dedup collapsed every run to a handful of grids and
/// discarded the best fill of each. Different fills of one shape are
/// different puzzles (minis reuse a few shapes daily), so they are kept, up
/// to the cap; exact-duplicate fills are never worth keeping twice.
pub fn select_keepers(mut kept: Vec<LibGrid>, per_shape: usize, theme_weight: f64) -> Vec<LibGrid> {
    let key = |g: &LibGrid| theme_weight * g.theme as f64 - g.weak as f64;
    kept.sort_by(|a, b| {
        key(b)
            .partial_cmp(&key(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                b.mean
                    .partial_cmp(&a.mean)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    let mut seen_fill: HashSet<String> = HashSet::new();
    let mut per_tpl: HashMap<String, usize> = HashMap::new();
    kept.retain(|g| {
        if !seen_fill.insert(g.fill.join("\n")) {
            return false;
        }
        if per_shape == 0 {
            return true;
        }
        let n = per_tpl.entry(g.template.join("\n")).or_insert(0);
        if *n >= per_shape {
            return false;
        }
        *n += 1;
        true
    });
    kept
}

/// Number of distinct templates among `grids` — surfaced by the screeners so
/// a shape-space collapse (five 5×5 shapes) is visible, not silent.
pub fn distinct_shapes(grids: &[LibGrid]) -> usize {
    grids
        .iter()
        .map(|g| g.template.join("\n"))
        .collect::<HashSet<_>>()
        .len()
}

/// Build the printable form of a library grid (clues taken from entries;
/// entries without a clue print an empty line).
pub fn to_pdf_puzzle(g: &LibGrid, title: &str, subtitle: &str) -> crate::pdf::PdfPuzzle {
    use crate::pdf::PdfClue;
    let mut numbers: Vec<(usize, usize, u32)> =
        g.entries.iter().map(|e| (e.row, e.col, e.num)).collect();
    numbers.sort_unstable();
    numbers.dedup();
    let list = |dir: char| -> Vec<PdfClue> {
        let mut v: Vec<PdfClue> = g
            .entries
            .iter()
            .filter(|e| e.dir == dir)
            .map(|e| PdfClue {
                num: e.num,
                text: e.clue.clone().unwrap_or_default(),
            })
            .collect();
        v.sort_by_key(|c| c.num);
        v
    };
    crate::pdf::PdfPuzzle {
        title: title.to_string(),
        subtitle: subtitle.to_string(),
        fill: g.fill.clone(),
        numbers,
        across: list('A'),
        down: list('D'),
    }
}

/// JSON string escaping (quotes, backslash, control characters).
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn json_arr(rows: &[String]) -> String {
    rows.iter()
        .map(|r| format!("\"{}\"", esc(r)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Serialize a library to JSON.
pub fn write_json(
    path: &str,
    grids: &[LibGrid],
    wordlist: &str,
    target_blocks: usize,
) -> std::io::Result<()> {
    let mut s = String::new();
    s.push_str("{\n");
    s.push_str(&format!("  \"wordlist\": \"{}\",\n", esc(wordlist)));
    s.push_str(&format!("  \"target_blocks\": {target_blocks},\n"));
    s.push_str(&format!("  \"count\": {},\n", grids.len()));
    s.push_str("  \"grids\": [\n");
    for (gi, g) in grids.iter().enumerate() {
        s.push_str("    {\n");
        s.push_str(&format!("      \"id\": {gi},\n"));
        s.push_str(&format!("      \"blocks\": {},\n", g.blocks));
        s.push_str(&format!("      \"mean_score\": {:.2},\n", g.mean));
        s.push_str(&format!("      \"min_score\": {},\n", g.min));
        s.push_str(&format!("      \"weak\": {},\n", g.weak));
        s.push_str(&format!(
            "      \"template\": [{}],\n",
            json_arr(&g.template)
        ));
        s.push_str(&format!("      \"fill\": [{}],\n", json_arr(&g.fill)));
        s.push_str("      \"entries\": [\n");
        for (ei, e) in g.entries.iter().enumerate() {
            s.push_str(&format!(
                "        {{\"num\": {}, \"dir\": \"{}\", \"row\": {}, \"col\": {}, \"len\": {}, \"answer\": \"{}\", \"score\": {}, \"clue\": {}}}{}\n",
                e.num, e.dir, e.row, e.col, e.len, esc(&e.answer), e.score,
                e.clue.as_ref().map_or("null".to_string(), |c| format!("\"{}\"", esc(c))),
                if ei + 1 < g.entries.len() { "," } else { "" }
            ));
        }
        s.push_str("      ]\n");
        s.push_str(if gi + 1 < grids.len() {
            "    },\n"
        } else {
            "    }\n"
        });
    }
    s.push_str("  ]\n}\n");
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(entries: Vec<LibEntry>) -> LibGrid {
        LibGrid {
            blocks: 0,
            mean: 0.0,
            min: 0,
            weak: 0,
            theme: 0,
            template: Vec::new(),
            fill: Vec::new(),
            entries,
        }
    }

    /// A kept grid with just the fields `select_keepers` reads: shape, fill
    /// (one row per letter-string, for brevity), weak count, mean.
    fn shaped(tpl: &str, fill: &str, weak: usize, mean: f64) -> LibGrid {
        let mut g = grid(Vec::new());
        g.template = vec![tpl.to_string()];
        g.fill = vec![fill.to_string()];
        g.weak = weak;
        g.mean = mean;
        g
    }

    #[test]
    fn select_keepers_keeps_best_fills_per_shape_not_first_finished() {
        // Completion order deliberately puts the WORST fill of shape A first.
        let kept = vec![
            shaped("A", "a-worst", 2, 80.0),
            shaped("B", "b-only", 0, 82.0),
            shaped("A", "a-best", 0, 88.0),
            shaped("A", "a-mid", 0, 85.0),
            shaped("A", "a-third", 1, 90.0),
        ];
        let out = select_keepers(kept, 2, 0.0);
        let fills: Vec<&str> = out.iter().map(|g| g.fill[0].as_str()).collect();
        // Fewest-weak-then-mean order, and shape A capped at its two best.
        assert_eq!(fills, vec!["a-best", "a-mid", "b-only"]);
    }

    /// With a theme weight, a grid with more theme words can outrank one with
    /// fewer weak entries.
    #[test]
    fn select_keepers_weighs_theme_words() {
        let themed = |fill: &str, weak, mean, theme| {
            let mut g = shaped(fill, fill, weak, mean);
            g.theme = theme;
            g
        };
        let kept = || vec![themed("plain", 0, 80.0, 1), themed("themed", 2, 75.0, 6)];
        let first = |w| select_keepers(kept(), 0, w)[0].fill[0].clone();
        assert_eq!(first(0.0), "plain");
        assert_eq!(first(1.0), "themed", "1×5 more theme words > 2 more weak");
    }

    #[test]
    fn mark_theme_flags_theme_answers() {
        let wl = crate::wordlist::Wordlist::from_str("CASA;60\nROMA;70\n", 0);
        let roma = wl.len_data(4).index_of(&[17, 14, 12, 0]).unwrap();
        let entry = |answer: &str| LibEntry {
            num: 1,
            dir: 'A',
            row: 0,
            col: 0,
            len: answer.len(),
            answer: answer.to_string(),
            score: 60,
            clue: None,
            is_theme: false,
        };
        let mut g = grid(vec![entry("CASA"), entry("ROMA"), entry("XY")]);
        mark_theme(&mut g, &wl, &[(4usize, roma)].into_iter().collect());
        let flags: Vec<bool> = g.entries.iter().map(|e| e.is_theme).collect();
        assert_eq!(flags, vec![false, true, false]);
        assert_eq!(g.theme, 1);
    }

    #[test]
    fn select_keepers_drops_exact_duplicate_fills_and_uncaps_at_zero() {
        let kept = || {
            vec![
                shaped("A", "same", 0, 90.0),
                shaped("A", "same", 0, 90.0), // identical fill from another seed
                shaped("A", "other", 0, 89.0),
                shaped("A", "third", 0, 88.0),
            ]
        };
        let out = select_keepers(kept(), 0, 0.0);
        assert_eq!(out.len(), 3, "duplicate fill collapses; no per-shape cap");
        assert_eq!(
            select_keepers(kept(), 1, 0.0).len(),
            1,
            "cap 1 keeps the best only"
        );
        assert_eq!(distinct_shapes(&out), 1);
    }
}
