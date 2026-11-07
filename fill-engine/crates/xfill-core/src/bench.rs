//! Fixed benchmark suite: deterministic block patterns of typical Italian
//! sizes and densities, solved under a given `SolveConfig`, summarized per
//! pattern family. Used to compare solver variants and choose defaults.

use crate::gen_it::{generate_it, GenMode, ItGenConfig, Symmetry};
use crate::grid::Puzzle;
use crate::solver::{SolveConfig, SolveResult, Solver};
use crate::util::Rng;
use crate::wordlist::Wordlist;

/// One family of patterns in the suite.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub name: &'static str,
    pub rows: usize,
    pub cols: usize,
    pub blacks: usize,
    pub count: usize,
}

/// Typical Italian sizes, from open (few blacks, hard) to dense.
pub const STANDARD: &[Spec] = &[
    Spec {
        name: "9x9/12",
        rows: 9,
        cols: 9,
        blacks: 12,
        count: 5,
    },
    Spec {
        name: "11x11/18",
        rows: 11,
        cols: 11,
        blacks: 18,
        count: 5,
    },
    Spec {
        name: "13x13/24",
        rows: 13,
        cols: 13,
        blacks: 24,
        count: 5,
    },
    Spec {
        name: "13x13/30",
        rows: 13,
        cols: 13,
        blacks: 30,
        count: 5,
    },
    Spec {
        name: "15x15/32",
        rows: 15,
        cols: 15,
        blacks: 32,
        count: 5,
    },
    Spec {
        name: "13x21/40",
        rows: 13,
        cols: 21,
        blacks: 40,
        count: 5,
    },
];

/// Tiny suite for tests and quick checks.
pub const SMOKE: &[Spec] = &[Spec {
    name: "5x5/4",
    rows: 5,
    cols: 5,
    blacks: 4,
    count: 2,
}];

pub fn suite_by_name(name: &str) -> Option<&'static [Spec]> {
    match name {
        "standard" => Some(STANDARD),
        "smoke" => Some(SMOKE),
        _ => None,
    }
}

pub struct Case {
    pub spec: &'static str,
    pub template: String,
}

fn name_seed(name: &str) -> u64 {
    name.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01B3)
    })
}

/// Build the suite: `count` distinct valid patterns per spec, compatible with
/// `wl` (no entry length missing from it). Deterministic for a given seed and
/// word list. Specs that can't produce their patterns contribute fewer cases.
pub fn build_suite(specs: &[Spec], wl: &Wordlist, seed: u64) -> Vec<Case> {
    let mut cases = Vec::new();
    for spec in specs {
        // the suite stays fixed: always the random walk, whatever the
        // generator default becomes
        let cfg = ItGenConfig {
            symmetry: Symmetry::Rotational,
            max_len: spec.rows.max(spec.cols).min(wl.longest()),
            mode: GenMode::Random,
            ..ItGenConfig::new(spec.rows, spec.cols, spec.blacks)
        };
        let mut rng = Rng::new(seed ^ name_seed(spec.name));
        let mut got: Vec<String> = Vec::new();
        let mut tries = 0;
        while got.len() < spec.count && tries < spec.count * 50 {
            tries += 1;
            let Some(t) = generate_it(&cfg, &mut rng, 50) else {
                continue;
            };
            if got.contains(&t) || !wl.missing_lengths(&Puzzle::from_template(&t)).is_empty() {
                continue;
            }
            got.push(t);
        }
        cases.extend(got.into_iter().map(|template| Case {
            spec: spec.name,
            template,
        }));
    }
    cases
}

#[derive(Clone, Debug)]
pub struct CaseResult {
    pub spec: &'static str,
    pub solved: bool,
    pub secs: f64,
    pub nodes: u64,
    pub mean: Option<f64>,
    pub weak: Option<usize>,
}

pub fn run_case(case: &Case, wl: &Wordlist, cfg: &SolveConfig) -> CaseResult {
    let p = Puzzle::from_template(&case.template);
    let r = Solver::new(&p, wl).solve(cfg);
    result_of(case.spec, &r)
}

/// Like [`run_case`], solving the pattern with `threads` threads (see
/// [`crate::solver::solve_portfolio`]).
pub fn run_case_threads(
    case: &Case,
    wl: &Wordlist,
    cfg: &SolveConfig,
    threads: usize,
) -> CaseResult {
    let p = Puzzle::from_template(&case.template);
    let r = crate::solver::solve_portfolio(&p, wl, &[], cfg, threads).expect("no locks");
    result_of(case.spec, &r)
}

/// Quality stats of the fill that would be published: the clean track when
/// present (as `library` does), else the mean-best fill.
fn result_of(spec: &'static str, r: &SolveResult) -> CaseResult {
    let (mean, weak) = match &r.clean {
        Some(c) => (Some(c.mean_score), Some(c.weak_count)),
        None => (r.mean_score, r.weak_count),
    };
    CaseResult {
        spec,
        solved: r.reason == "solved",
        secs: r.elapsed_s,
        nodes: r.nodes,
        mean,
        weak,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpecSummary {
    pub spec: String,
    pub cases: usize,
    pub solved: usize,
    /// Median wall time over SOLVED cases (None if none solved).
    pub median_secs: Option<f64>,
    pub mean_nodes: f64,
    /// Mean answer score averaged over solved cases.
    pub mean_score: Option<f64>,
    /// Weak entries per solved case, on average.
    pub mean_weak: Option<f64>,
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// Per-spec summaries in first-appearance order, plus a final "TOTALE" row.
pub fn summarize(results: &[CaseResult]) -> Vec<SpecSummary> {
    let mut names: Vec<&str> = Vec::new();
    for r in results {
        if !names.contains(&r.spec) {
            names.push(r.spec);
        }
    }
    let one = |name: String, rs: Vec<&CaseResult>| {
        let solved: Vec<&&CaseResult> = rs.iter().filter(|r| r.solved).collect();
        let scores: Vec<f64> = solved.iter().filter_map(|r| r.mean).collect();
        let weaks: Vec<f64> = solved
            .iter()
            .filter_map(|r| r.weak.map(|w| w as f64))
            .collect();
        SpecSummary {
            spec: name,
            cases: rs.len(),
            solved: solved.len(),
            median_secs: median(solved.iter().map(|r| r.secs).collect()),
            mean_nodes: if rs.is_empty() {
                0.0
            } else {
                rs.iter().map(|r| r.nodes as f64).sum::<f64>() / rs.len() as f64
            },
            mean_score: if scores.is_empty() {
                None
            } else {
                Some(scores.iter().sum::<f64>() / scores.len() as f64)
            },
            mean_weak: if weaks.is_empty() {
                None
            } else {
                Some(weaks.iter().sum::<f64>() / weaks.len() as f64)
            },
        }
    };
    let mut out: Vec<SpecSummary> = names
        .iter()
        .map(|n| {
            one(
                n.to_string(),
                results.iter().filter(|r| r.spec == *n).collect(),
            )
        })
        .collect();
    out.push(one("TOTALE".into(), results.iter().collect()));
    out
}

pub fn format_table(rows: &[SpecSummary]) -> String {
    let mut s = format!(
        "{:<10} {:>8} {:>12} {:>14} {:>8} {:>8}\n",
        "schema", "riempiti", "mediana (s)", "nodi medi", "media", "deboli"
    );
    for r in rows {
        s.push_str(&format!(
            "{:<10} {:>8} {:>12} {:>14.0} {:>8} {:>8}\n",
            r.spec,
            format!("{}/{}", r.solved, r.cases),
            r.median_secs.map_or("-".to_string(), |m| format!("{m:.3}")),
            r.mean_nodes,
            r.mean_score.map_or("-".to_string(), |m| format!("{m:.1}")),
            r.mean_weak.map_or("-".to_string(), |m| format!("{m:.1}")),
        ));
    }
    s
}

/// Theme density of one format over several seeds (see [`theme_density`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeRow {
    pub format: String,
    pub runs: usize,
    /// Runs that produced a grid.
    pub ok: usize,
    /// Theme words per produced grid: average, min, max.
    pub theme_avg: f64,
    pub theme_min: usize,
    pub theme_max: usize,
    /// Entries per produced grid, on average.
    pub entries_avg: f64,
    /// Mean word score and weak entries per produced grid, on average.
    pub score_avg: f64,
    pub weak_avg: f64,
    /// Wall time per run, on average (seconds).
    pub secs_avg: f64,
}

/// Generate `base` at each of `formats` (rows, cols) once per seed and
/// measure how many theme words the grids hold.
pub fn theme_density(
    dict: &crate::api::Dictionary,
    base: &crate::api::GenerateRequest,
    formats: &[(usize, usize)],
    seeds: &[u64],
) -> Vec<ThemeRow> {
    let cancel = crate::api::cancel_flag();
    formats
        .iter()
        .map(|&(rows, cols)| {
            let mut ok = Vec::new();
            let mut secs = 0.0;
            for &seed in seeds {
                let req = crate::api::GenerateRequest {
                    rows,
                    cols,
                    seed,
                    clues: false,
                    ..base.clone()
                };
                let t = std::time::Instant::now();
                let r = crate::api::generate(dict, &req, &cancel);
                secs += t.elapsed().as_secs_f64();
                if r.status == crate::api::Status::Ok {
                    ok.push(r);
                }
            }
            let avg = |f: &dyn Fn(&crate::api::GenerateResult) -> f64| {
                if ok.is_empty() {
                    0.0
                } else {
                    ok.iter().map(f).sum::<f64>() / ok.len() as f64
                }
            };
            ThemeRow {
                format: format!("{rows}x{cols}"),
                runs: seeds.len(),
                ok: ok.len(),
                theme_avg: avg(&|r| r.theme_count as f64),
                theme_min: ok.iter().map(|r| r.theme_count).min().unwrap_or(0),
                theme_max: ok.iter().map(|r| r.theme_count).max().unwrap_or(0),
                entries_avg: avg(&|r| r.entries.len() as f64),
                score_avg: avg(&|r| r.score),
                weak_avg: avg(&|r| r.weak_words.len() as f64),
                secs_avg: secs / seeds.len().max(1) as f64,
            }
        })
        .collect()
}

/// Table of [`theme_density`] rows.
pub fn format_theme_table(rows: &[ThemeRow]) -> String {
    let mut s = format!(
        "{:<8} {:>6} {:>10} {:>9} {:>7} {:>7} {:>7} {:>8}\n",
        "formato", "ok", "tema medio", "min-max", "parole", "media", "deboli", "s/run"
    );
    for r in rows {
        s.push_str(&format!(
            "{:<8} {:>6} {:>10.1} {:>9} {:>7.1} {:>7.1} {:>7.1} {:>8.1}\n",
            r.format,
            format!("{}/{}", r.ok, r.runs),
            r.theme_avg,
            format!("{}-{}", r.theme_min, r.theme_max),
            r.entries_avg,
            r.score_avg,
            r.weak_avg,
            r.secs_avg
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every 2..=5-letter string over a small alphabet: easy to fill.
    fn synth() -> Wordlist {
        let alpha = ['A', 'E', 'I', 'O', 'R', 'S'];
        let mut words: Vec<String> = alpha.iter().map(|c| c.to_string()).collect();
        let mut src = String::new();
        for _ in 2..=5 {
            words = words
                .iter()
                .flat_map(|w| alpha.iter().map(move |c| format!("{w}{c}")))
                .collect();
            for w in &words {
                src.push_str(&format!("{w};60\n"));
            }
        }
        Wordlist::from_str(&src, 0)
    }

    #[test]
    fn suite_is_deterministic_and_dictionary_compatible() {
        let wl = synth();
        let a = build_suite(SMOKE, &wl, 7);
        let b = build_suite(SMOKE, &wl, 7);
        assert_eq!(a.len(), 2);
        assert_eq!(
            a.iter().map(|c| &c.template).collect::<Vec<_>>(),
            b.iter().map(|c| &c.template).collect::<Vec<_>>()
        );
        assert_ne!(a[0].template, a[1].template, "distinct patterns");
        for c in &a {
            assert!(wl
                .missing_lengths(&Puzzle::from_template(&c.template))
                .is_empty());
        }
        assert!(suite_by_name("standard").is_some());
        assert!(suite_by_name("nope").is_none());
    }

    #[test]
    fn runs_and_summarizes() {
        let wl = synth();
        let cases = build_suite(SMOKE, &wl, 3);
        let cfg = SolveConfig {
            time_limit_s: 1.0,
            stop_on_first: true,
            ..Default::default()
        };
        let results: Vec<CaseResult> = cases.iter().map(|c| run_case(c, &wl, &cfg)).collect();
        assert!(results.iter().all(|r| r.solved));
        let sum = summarize(&results);
        assert_eq!(sum.len(), 2, "one spec row + TOTALE");
        assert_eq!(sum[1].spec, "TOTALE");
        assert_eq!((sum[1].solved, sum[1].cases), (2, 2));
        assert!(sum[0].median_secs.is_some());
        let table = format_table(&sum);
        assert!(table.contains("5x5/4") && table.contains("2/2"));
    }

    /// The benchmark measures the fill a library would publish: the clean
    /// track (fewest weak entries) when there is one.
    #[test]
    fn case_result_prefers_the_clean_fill() {
        let clean = crate::solver::SolvedFill {
            letters: Vec::new(),
            mean_score: 75.0,
            min_score: 60,
            weak_count: 0,
            theme_count: 0,
            fill: Vec::new(),
        };
        let r = SolveResult {
            letters: Some(Vec::new()),
            nodes: 5,
            restarts: 1,
            elapsed_s: 0.1,
            reason: "solved",
            mean_score: Some(80.0),
            min_score: Some(50),
            weak_count: Some(3),
            fill: Some(Vec::new()),
            clean: Some(clean),
            proven_unfillable: false,
            theme_count: 0,
        };
        let c = result_of("x", &r);
        assert_eq!((c.mean, c.weak), (Some(75.0), Some(0)));
        let r = SolveResult { clean: None, ..r };
        assert_eq!(result_of("x", &r).weak, Some(3));
    }

    #[test]
    fn run_case_with_threads() {
        let wl = synth();
        let cases = build_suite(SMOKE, &wl, 3);
        let cfg = SolveConfig {
            time_limit_s: 1.0,
            stop_on_first: true,
            ..Default::default()
        };
        let r = run_case_threads(&cases[0], &wl, &cfg, 3);
        assert!(r.solved);
    }

    #[test]
    fn summary_handles_unsolved_and_even_medians() {
        let r = |solved, secs| CaseResult {
            spec: "x",
            solved,
            secs,
            nodes: 10,
            mean: if solved { Some(60.0) } else { None },
            weak: if solved { Some(secs as usize) } else { None },
        };
        let s = summarize(&[r(true, 1.0), r(true, 3.0), r(false, 9.0)]);
        assert_eq!(s[0].solved, 2);
        assert_eq!(
            s[0].mean_weak,
            Some(2.0),
            "weak counts 1 and 3 over solved cases"
        );
        assert!(format_table(&s).contains("deboli"));
        assert_eq!(
            s[0].median_secs,
            Some(2.0),
            "even count → average of middle two"
        );
        assert_eq!(s[0].mean_score, Some(60.0));
        let s = summarize(&[r(false, 1.0)]);
        assert_eq!(s[0].median_secs, None);
        assert_eq!(s[0].mean_score, None);
        assert!(format_table(&s).contains('-'));
        assert_eq!(summarize(&[]).last().unwrap().mean_nodes, 0.0);
    }

    #[test]
    fn theme_density_counts_theme_words_per_format() {
        let mut csv = String::new();
        let alpha = ['A', 'E', 'I', 'O', 'R', 'S'];
        let mut words: Vec<String> = alpha.iter().map(|c| c.to_string()).collect();
        for _ in 2..=5 {
            words = words
                .iter()
                .flat_map(|w| alpha.iter().map(move |c| format!("{w}{c}")))
                .collect();
            for w in &words {
                csv.push_str(&format!("{w},voce\n"));
            }
        }
        let dict = crate::api::Dictionary::from_csv_bytes(csv.as_bytes()).unwrap();
        let base = crate::api::GenerateRequest {
            theme_words: vec!["ARIA".into(), "SERA".into(), "ORSO".into(), "OSSA".into()],
            candidates: 4,
            node_budget: 2_000,
            blocks: Some(6),
            ..Default::default()
        };
        let rows = theme_density(&dict, &base, &[(5, 5), (5, 7)], &[1, 2]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].format, "5x5");
        assert_eq!(rows[1].format, "5x7");
        for r in &rows {
            assert_eq!(r.runs, 2);
            assert_eq!(r.ok, 2, "{r:?}");
            assert!(r.theme_avg >= 1.0 && r.theme_min >= 1 && r.theme_max <= 4);
            assert!(r.entries_avg > 0.0 && r.score_avg > 0.0);
        }
        let t = format_theme_table(&rows);
        assert!(t.contains("5x7") && t.contains("tema"), "{t}");
    }
}
