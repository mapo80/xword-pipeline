//! Section analysis: the open part of a grid split into independent
//! sections, each judged fillable / impossible / uncertain by filling it
//! alone within a time budget; verdicts are cached by section content.

use crate::grid::{Dir, Puzzle};
use crate::solver::{SolveConfig, Solver};
use crate::wordlist::Wordlist;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// A fill of the section was found.
    Fillable,
    /// The search proved the section can't be filled.
    Impossible,
    /// Neither within the time budget.
    Uncertain,
}

/// A group of open entries linked through empty cells.
pub struct Section {
    pub entries: Vec<usize>,
    pub verdict: Verdict,
    /// The verdict came from the cache.
    pub cached: bool,
}

/// Verdicts by section content (entries' positions and letters).
#[derive(Default)]
pub struct SectionCache {
    map: HashMap<u64, Verdict>,
    pub hits: usize,
}

/// Split the open part of `p` into sections and judge each one by filling
/// it alone (`cfg` sets the time per section). Sections are independent
/// except for the no-duplicate rule, which this per-section view ignores.
pub fn analyze_sections(
    p: &Puzzle,
    wl: &Wordlist,
    cfg: &SolveConfig,
    cache: &mut SectionCache,
) -> Vec<Section> {
    let n = p.entries.len();
    let open: Vec<bool> = p
        .entries
        .iter()
        .map(|e| e.cells.iter().any(|&c| p.prefilled[c].is_none()))
        .collect();
    // union-find over open entries sharing an empty cell
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while parent[r] != r {
            r = parent[r];
        }
        let mut y = x;
        while parent[y] != r {
            let next = parent[y];
            parent[y] = r;
            y = next;
        }
        r
    }
    for (cid, ents) in p.cell_entries.iter().enumerate() {
        if p.prefilled[cid].is_some() {
            continue;
        }
        for w in ents.windows(2) {
            let (a, b) = (find(&mut parent, w[0].0), find(&mut parent, w[1].0));
            parent[a] = b;
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for e in (0..n).filter(|&e| open[e]) {
        let root = find(&mut parent, e);
        groups.entry(root).or_default().push(e);
    }
    let mut sections: Vec<Vec<usize>> = groups.into_values().collect();
    sections.sort();

    let cell_at: HashMap<(usize, usize), usize> = p
        .cell_rc
        .iter()
        .enumerate()
        .map(|(cid, &rc)| (rc, cid))
        .collect();
    let text = |cid: usize| p.prefilled[cid].map_or(".".to_string(), |b| p.symbol_text(b));

    sections
        .into_iter()
        .map(|entries| {
            let key = section_key(p, &entries, &text);
            if let Some(&verdict) = cache.map.get(&key) {
                cache.hits += 1;
                return Section {
                    entries,
                    verdict,
                    cached: true,
                };
            }
            // the section alone: its cells as they are, everything else black
            let mut keep = vec![false; p.n_cells];
            for &e in &entries {
                for &c in &p.entries[e].cells {
                    keep[c] = true;
                }
            }
            let mut sub = String::new();
            for r in 0..p.rows {
                for c in 0..p.cols {
                    match cell_at.get(&(r, c)) {
                        Some(&cid) if keep[cid] => sub.push_str(&text(cid)),
                        _ => sub.push('#'),
                    }
                }
                sub.push('\n');
            }
            let sp = Puzzle::from_template_with_rebus(&sub, &p.rebus);
            let r = Solver::new(&sp, wl).solve(cfg);
            let verdict = if r.reason == "solved" {
                Verdict::Fillable
            } else if r.proven_unfillable {
                Verdict::Impossible
            } else {
                Verdict::Uncertain
            };
            cache.map.insert(key, verdict);
            Section {
                entries,
                verdict,
                cached: false,
            }
        })
        .collect()
}

fn section_key(p: &Puzzle, entries: &[usize], text: &dyn Fn(usize) -> String) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for &e in entries {
        let en = &p.entries[e];
        (en.row, en.col, en.dir == Dir::Across).hash(&mut h);
        for &c in &en.cells {
            text(c).hash(&mut h);
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Puzzle;
    use crate::solver::SolveConfig;
    use crate::wordlist::Wordlist;

    fn cfg() -> SolveConfig {
        SolveConfig {
            time_limit_s: 1.0,
            stop_on_first: true,
            ..Default::default()
        }
    }

    /// Left: an open 2x2 fillable with AB/CD/AC/BD. Right: a 2x2 whose
    /// prefilled Z has no word. Separated by a black column.
    fn two_sections() -> (Puzzle, Wordlist) {
        (
            Puzzle::from_template("..#Z.\n..#..\n"),
            Wordlist::from_str("AB;60\nCD;60\nAC;60\nBD;60\n", 0),
        )
    }

    #[test]
    fn sections_get_their_own_verdicts() {
        let (p, wl) = two_sections();
        let mut cache = SectionCache::default();
        let secs = analyze_sections(&p, &wl, &cfg(), &mut cache);
        assert_eq!(secs.len(), 2);
        let left = secs
            .iter()
            .find(|s| {
                s.entries
                    .contains(&p.find_entry(0, 0, crate::grid::Dir::Across).unwrap())
            })
            .unwrap();
        let right = secs
            .iter()
            .find(|s| {
                s.entries
                    .contains(&p.find_entry(0, 3, crate::grid::Dir::Across).unwrap())
            })
            .unwrap();
        assert_eq!(left.verdict, Verdict::Fillable);
        assert_eq!(right.verdict, Verdict::Impossible);
        assert!(secs.iter().all(|s| !s.cached));
    }

    #[test]
    fn verdicts_are_cached_by_content() {
        let (p, wl) = two_sections();
        let mut cache = SectionCache::default();
        let first = analyze_sections(&p, &wl, &cfg(), &mut cache);
        let again = analyze_sections(&p, &wl, &cfg(), &mut cache);
        assert_eq!(cache.hits, 2);
        assert!(again.iter().all(|s| s.cached));
        let v = |x: &Vec<Section>| x.iter().map(|s| s.verdict).collect::<Vec<_>>();
        assert_eq!(v(&first), v(&again));
        // change the right section only: the left one is still cached
        let p2 = Puzzle::from_template("..#A.\n..#..\n");
        let third = analyze_sections(&p2, &wl, &cfg(), &mut cache);
        assert_eq!(third.iter().filter(|s| s.cached).count(), 1);
    }

    #[test]
    fn complete_entries_form_no_section() {
        let p = Puzzle::from_template("AB\nCD\n");
        let wl = Wordlist::from_str("AB;60\nCD;60\nAC;60\nBD;60\n", 0);
        let secs = analyze_sections(&p, &wl, &cfg(), &mut SectionCache::default());
        assert!(secs.is_empty());
    }
}
