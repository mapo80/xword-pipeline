//! Benchmark the solver on a fixed suite of Italian-style patterns.
//!
//! Usage: bench [--wordlist PATH] [--suite standard|smoke] [--time SECS]
//!              [--mode first|quality] [--seed N] [--workers N] [--threads T]
//!              [--verbose]
//!        bench --theme FILE [--wordlist PATH] [--formats RxC,…] [--runs N]
//!              [--request JSON]
//!
//! `first` measures fill power (time to the first complete fill);
//! `quality` runs the full quality search for `--time` seconds per pattern.
//! `--theme` measures theme density instead: full generations (patterns,
//! fill, choice) per format and seed, with the words of FILE as the theme;
//! `--request` overrides fields of the generation request (JSON).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use xfill_core::bench::{
    build_suite, format_table, run_case, run_case_threads, suite_by_name, summarize, CaseResult,
};
use xfill_core::clues::load_clue_csv;
use xfill_core::SolveConfig;

fn arg<T: std::str::FromStr>(name: &str, default: T) -> T {
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        if a == name {
            if let Some(v) = it.next() {
                if let Ok(p) = v.parse() {
                    return p;
                }
            }
        }
    }
    default
}

fn flag(name: &str) -> bool {
    std::env::args().skip(1).any(|a| a == name)
}

/// Theme density mode (`--theme FILE`).
fn theme_mode(wordlist: &str, theme_file: &str) {
    let text = match std::fs::read_to_string(theme_file) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: cannot read theme list {theme_file}: {e}");
            std::process::exit(1);
        }
    };
    let words: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect();
    let formats: Vec<(usize, usize)> = match arg("--formats", "13x13,15x15,12x18,12x22".to_string())
        .split(',')
        .map(|f| {
            let (r, c) = f.trim().split_once('x')?;
            Some((r.parse().ok()?, c.parse().ok()?))
        })
        .collect::<Option<Vec<_>>>()
    {
        Some(f) => f,
        None => {
            eprintln!("error: bad --formats (use ROWSxCOLS,…)");
            std::process::exit(2);
        }
    };
    let runs: u64 = arg("--runs", 3);
    let base: xfill_core::api::GenerateRequest =
        match serde_json::from_str(&arg("--request", "{}".to_string())) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: bad --request: {e}");
                std::process::exit(2);
            }
        };
    let bytes = match std::fs::read(wordlist) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: cannot load wordlist {wordlist}: {e}");
            std::process::exit(1);
        }
    };
    let dict = match xfill_core::api::Dictionary::from_csv_bytes(&bytes) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let found = xfill_core::solver::theme_ids(&dict.words, &words).len();
    eprintln!(
        "theme: {} words ({found} in the dictionary), {runs} runs per format",
        words.len()
    );
    let base = xfill_core::api::GenerateRequest {
        theme_words: words,
        ..base
    };
    let seeds: Vec<u64> = (1..=runs).collect();
    let rows = xfill_core::bench::theme_density(&dict, &base, &formats, &seeds);
    print!("{}", xfill_core::bench::format_theme_table(&rows));
}

fn main() {
    let wordlist: String = arg("--wordlist", "../../dataset/Db.csv".to_string());
    let theme_file: String = arg("--theme", String::new());
    if !theme_file.is_empty() {
        theme_mode(&wordlist, &theme_file);
        return;
    }
    let suite_name: String = arg("--suite", "standard".to_string());
    let time: f64 = arg("--time", 3.0);
    let mode: String = arg("--mode", "first".to_string());
    let seed: u64 = arg("--seed", 1);
    let verbose = flag("--verbose");
    let threads: usize = arg("--threads", 1);
    let workers: usize = arg(
        "--workers",
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
    );
    let Some(specs) = suite_by_name(&suite_name) else {
        eprintln!("error: unknown --suite '{suite_name}' (use standard or smoke)");
        std::process::exit(2);
    };
    let stop_on_first = match mode.as_str() {
        "first" => true,
        "quality" => false,
        _ => {
            eprintln!("error: unknown --mode '{mode}' (use first or quality)");
            std::process::exit(2);
        }
    };
    let (wl, _clues) = match load_clue_csv(&wordlist, 0) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("error: cannot load wordlist {wordlist}: {e}");
            std::process::exit(1);
        }
    };

    let cases = build_suite(specs, &wl, seed);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = match (SolveConfig {
        time_limit_s: time,
        stop_on_first,
        ..Default::default()
    })
    .with_args(&args)
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    eprintln!(
        "suite {suite_name}: {} patterns, mode {mode}, {time}s each, {workers} workers, \
         {threads} threads per pattern, {}",
        cases.len(),
        cfg.describe()
    );

    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, CaseResult)>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..workers.max(1) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= cases.len() {
                    break;
                }
                let mut c = cfg.clone();
                c.seed = seed.wrapping_add(i as u64);
                let r = if threads > 1 {
                    run_case_threads(&cases[i], &wl, &c, threads)
                } else {
                    run_case(&cases[i], &wl, &c)
                };
                if verbose {
                    eprintln!(
                        "  {:<10} #{i:<3} {} {:.3}s nodes={}",
                        r.spec,
                        if r.solved { "ok " } else { "NO " },
                        r.secs,
                        r.nodes
                    );
                }
                results.lock().unwrap().push((i, r));
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|(i, _)| *i);
    let results: Vec<CaseResult> = results.into_iter().map(|(_, r)| r).collect();
    print!("{}", format_table(&summarize(&results)));
}
