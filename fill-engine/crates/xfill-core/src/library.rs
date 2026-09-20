//! Shared grid-library artifact: the vetted JSON the daily-puzzle pipeline (clue
//! writing, scheduling) consumes. Used by both the `library` (themeless) and
//! `theme` (themed) binaries so they emit an identical schema.

use crate::grid::{Dir, Puzzle};
use crate::solver::{SolveResult, SolvedFill};
use std::collections::{HashMap, HashSet};

/// Pipeline default for `select_keepers`' per-shape cap. Four fills of one
/// template is enough variety that a 5×5 run (whose symmetric shape space is
/// FIVE templates in total) still produces a full top-20, while at 9×9 and up
/// shapes are plentiful (~190 distinct per 300 candidates) so the cap rarely
/// binds. Callers surface this as a tunable; 0 lifts the cap.
pub const DEFAULT_PER_SHAPE: usize = 4;

pub struct LibEntry {
    pub num: u32,
    pub dir: char, // 'A' | 'D'
    pub row: usize,
    pub col: usize,
    pub len: usize,
    pub answer: String,
    pub score: u8,
    pub theme: bool,
}

pub struct LibGrid {
    pub blocks: usize,
    pub mean: f64,
    pub min: u8,
    pub iffy: usize,
    /// Searched entries under the solver's weak bar (see
    /// `SolveConfig::weak_bar`) — the gluey tail an editor counts.
    pub weak: usize,
    pub themed: bool,
    pub template: Vec<String>,
    pub fill: Vec<String>,
    pub entries: Vec<LibEntry>,
}

/// Build a library record from a solve result's primary (best-by-mean) fill.
/// `theme_ids` are the entry ids that were locked theme answers (empty for
/// themeless). Returns None if the result isn't a solved fill.
pub fn build_lib_grid(p: &Puzzle, r: &SolveResult, theme_ids: &HashSet<usize>) -> Option<LibGrid> {
    let (mean, min, iffy) = match (r.mean_score, r.min_score, r.iffy_count) {
        (Some(m), Some(mn), Some(i)) => (m, mn, i),
        _ => return None,
    };
    let weak = r.weak_count.unwrap_or(0);
    let (letters, fill) = match (r.letters.as_deref(), r.fill.as_ref()) {
        (Some(l), Some(f)) => (l, f),
        _ => return None,
    };
    Some(build_from_parts(
        p, letters, mean, min, iffy, weak, fill, theme_ids,
    ))
}

/// Build a library record from a specific fill (e.g. the solver's `clean`
/// alternative when it passes the caller's keep gates).
pub fn build_lib_grid_from(p: &Puzzle, f: &SolvedFill, theme_ids: &HashSet<usize>) -> LibGrid {
    build_from_parts(
        p,
        &f.letters,
        f.mean_score,
        f.min_score,
        f.iffy_count,
        f.weak_count,
        &f.fill,
        theme_ids,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_from_parts(
    p: &Puzzle,
    letters: &[Option<u8>],
    mean: f64,
    min: u8,
    iffy: usize,
    weak: usize,
    fill: &[(usize, String, u8)],
    theme_ids: &HashSet<usize>,
) -> LibGrid {
    let nums = p.number_entries();
    let mut entries: Vec<LibEntry> = fill
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
                theme: theme_ids.contains(ei),
            }
        })
        .collect();
    entries.sort_by_key(|g| (g.num, g.dir));
    LibGrid {
        blocks: p.block_count(),
        mean,
        min,
        iffy,
        weak,
        themed: !theme_ids.is_empty(),
        template: p.render(None).lines().map(str::to_string).collect(),
        fill: p
            .render(Some(letters))
            .lines()
            .map(str::to_string)
            .collect(),
        entries,
    }
}

/// Order kept grids for output and collapse duplicates: fewest weak entries
/// first (mean breaks ties — the solver's own clean-track ordering), then
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
pub fn select_keepers(mut kept: Vec<LibGrid>, per_shape: usize) -> Vec<LibGrid> {
    kept.sort_by(|a, b| {
        a.weak.cmp(&b.weak).then(
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

/// Which member(s) of a root-duplicate pair to try banning for a refill
/// retry, most-disposable first: non-theme members only (theme answers are
/// locked and can't be banned), lower score first, shorter on ties. Empty if
/// neither member is a searchable fill entry.
pub fn dup_ban_targets<'a>(g: &'a LibGrid, a: &str, b: &str) -> Vec<&'a str> {
    let mut cands: Vec<&LibEntry> = g
        .entries
        .iter()
        .filter(|e| !e.theme && (e.answer == a || e.answer == b))
        .collect();
    cands.sort_by_key(|e| (e.score, e.answer.len()));
    cands.into_iter().map(|e| e.answer.as_str()).collect()
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn json_arr(rows: &[String]) -> String {
    rows.iter()
        .map(|r| format!("\"{}\"", esc(r)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Serialize a library to JSON. `themes` lists the theme answers (empty for a
/// themeless library) and is recorded in the metadata.
pub fn write_json(
    path: &str,
    grids: &[LibGrid],
    wordlist: &str,
    target_blocks: usize,
    themes: &[String],
) -> std::io::Result<()> {
    let mut s = String::new();
    s.push_str("{\n");
    s.push_str(&format!("  \"wordlist\": \"{}\",\n", esc(wordlist)));
    s.push_str(&format!("  \"target_blocks\": {target_blocks},\n"));
    s.push_str(&format!("  \"themed\": {},\n", !themes.is_empty()));
    s.push_str(&format!("  \"themes\": [{}],\n", json_arr(themes)));
    s.push_str(&format!("  \"count\": {},\n", grids.len()));
    s.push_str("  \"grids\": [\n");
    for (gi, g) in grids.iter().enumerate() {
        s.push_str("    {\n");
        s.push_str(&format!("      \"id\": {gi},\n"));
        s.push_str(&format!("      \"blocks\": {},\n", g.blocks));
        s.push_str(&format!("      \"themed\": {},\n", g.themed));
        s.push_str(&format!("      \"mean_score\": {:.2},\n", g.mean));
        s.push_str(&format!("      \"min_score\": {},\n", g.min));
        s.push_str(&format!("      \"iffy\": {},\n", g.iffy));
        s.push_str(&format!("      \"weak\": {},\n", g.weak));
        s.push_str(&format!(
            "      \"template\": [{}],\n",
            json_arr(&g.template)
        ));
        s.push_str(&format!("      \"fill\": [{}],\n", json_arr(&g.fill)));
        s.push_str("      \"entries\": [\n");
        for (ei, e) in g.entries.iter().enumerate() {
            s.push_str(&format!(
                "        {{\"num\": {}, \"dir\": \"{}\", \"row\": {}, \"col\": {}, \"len\": {}, \"answer\": \"{}\", \"score\": {}, \"theme\": {}}}{}\n",
                e.num, e.dir, e.row, e.col, e.len, esc(&e.answer), e.score, e.theme,
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

    fn entry(answer: &str, score: u8, theme: bool) -> LibEntry {
        LibEntry {
            num: 1,
            dir: 'A',
            row: 0,
            col: 0,
            len: answer.len(),
            answer: answer.to_string(),
            score,
            theme,
        }
    }

    fn grid(entries: Vec<LibEntry>) -> LibGrid {
        LibGrid {
            blocks: 0,
            mean: 0.0,
            min: 0,
            iffy: 0,
            weak: 0,
            themed: false,
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
        let out = select_keepers(kept, 2);
        let fills: Vec<&str> = out.iter().map(|g| g.fill[0].as_str()).collect();
        // Fewest-weak-then-mean order, and shape A capped at its two best.
        assert_eq!(fills, vec!["a-best", "a-mid", "b-only"]);
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
        let out = select_keepers(kept(), 0);
        assert_eq!(out.len(), 3, "duplicate fill collapses; no per-shape cap");
        assert_eq!(
            select_keepers(kept(), 1).len(),
            1,
            "cap 1 keeps the best only"
        );
        assert_eq!(distinct_shapes(&out), 1);
    }

    #[test]
    fn dup_ban_targets_prefers_disposable_member() {
        let g = grid(vec![
            entry("ARENA", 60, false),
            entry("RENA", 50, false),
            entry("SLAMDUNK", 90, true),
        ]);
        // Lower score first, so the junkier member is banned before the keeper.
        assert_eq!(dup_ban_targets(&g, "ARENA", "RENA"), vec!["RENA", "ARENA"]);
        // A theme member is locked → only the fill member is bannable.
        assert_eq!(dup_ban_targets(&g, "SLAMDUNK", "ARENA"), vec!["ARENA"]);
        // Equal scores tie-break shorter-first.
        let g = grid(vec![entry("ILLS", 50, false), entry("ILL", 50, false)]);
        assert_eq!(dup_ban_targets(&g, "ILL", "ILLS"), vec!["ILL", "ILLS"]);
    }
}
