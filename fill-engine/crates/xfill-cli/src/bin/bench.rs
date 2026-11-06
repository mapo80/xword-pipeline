//! Benchmark the solver on a fixed suite of Italian-style patterns.
//!
//! Usage: bench [--wordlist PATH] [--suite standard|smoke] [--time SECS]
//!              [--mode first|quality] [--seed N] [--workers N] [--verbose]
//!
//! `first` measures fill power (time to the first complete fill);
//! `quality` runs the full quality search for `--time` seconds per pattern.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use xfill_core::bench::{
    build_suite, format_table, run_case, suite_by_name, summarize, CaseResult,
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

fn main() {
    let wordlist: String = arg("--wordlist", "../../dataset/Db.csv".to_string());
    let suite_name: String = arg("--suite", "standard".to_string());
    let time: f64 = arg("--time", 3.0);
    let mode: String = arg("--mode", "first".to_string());
    let seed: u64 = arg("--seed", 1);
    let verbose = flag("--verbose");
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
        "suite {suite_name}: {} patterns, mode {mode}, {time}s each, {workers} workers, {}",
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
                let r = run_case(&cases[i], &wl, &c);
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
