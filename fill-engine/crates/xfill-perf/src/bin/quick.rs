//! Quick solver benchmark (seconds, not minutes): fixed patterns, fixed
//! node budget, deterministic results.
//! usage: quick <Db.csv> [node_budget=3000] [patterns_per_format=4] [threads=1]
//! Prints one FP line per pattern (compare across versions: must be equal)
//! and a summary: CPU per work unit, allocations, heap peak.
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::Instant;
use xfill_core::api::{cancel_flag, generate_patterns, plan_for, Dictionary, GenerateRequest};
use xfill_core::{Puzzle, SolveConfig, Solver};

#[global_allocator]
static A: xfill_perf::Counting = xfill_perf::Counting;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let budget: u64 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(3000);
    let per: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(4);
    let threads: usize = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(1);
    let bytes = std::fs::read(&a[1]).unwrap();
    let t = Instant::now();
    let dict = Dictionary::from_csv_bytes(&bytes).unwrap();
    drop(bytes);
    let load_ms = t.elapsed().as_secs_f64() * 1e3;
    let dict_heap = xfill_perf::live();

    // fixed patterns: 4 per format, from the API generator (seed 7)
    let t = Instant::now();
    let mut jobs: Vec<(String, String, SolveConfig)> = Vec::new();
    for f in ["9x9", "13x13", "15x15", "12x18", "12x22"] {
        let (r, c) = f.split_once('x').unwrap();
        let req = GenerateRequest {
            rows: r.parse().unwrap(),
            cols: c.parse().unwrap(),
            seed: 7,
            candidates: per,
            ..Default::default()
        };
        let (plan, _) = plan_for(&dict.words, &req).unwrap();
        let pats = generate_patterns(&dict.words, &plan, &cancel_flag());
        for (i, (tpl, seed)) in pats.jobs.into_iter().enumerate() {
            let cfg = SolveConfig {
                seed,
                node_limit: budget,
                time_limit_s: 600.0,
                ..plan.solve.clone()
            };
            jobs.push((format!("{f}#{i}"), tpl, cfg));
        }
    }
    let gen_ms = t.elapsed().as_secs_f64() * 1e3;

    let (c0, n0, w0) = (xfill_perf::cpu_s(), xfill_perf::allocs(), Instant::now());
    xfill_perf::reset_peak();
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<(usize, String)>> = Mutex::new(Vec::new());
    let work = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Relaxed);
                if i >= jobs.len() {
                    break;
                }
                let (name, tpl, cfg) = &jobs[i];
                let p = Puzzle::from_template_with_rebus(tpl, &dict.words.rebus);
                let r = Solver::new(&p, &dict.words).solve(cfg);
                work.fetch_add(r.nodes as usize, Relaxed);
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                tpl.hash(&mut h);
                r.fill.hash(&mut h);
                r.clean.as_ref().map(|c| c.fill.clone()).hash(&mut h);
                let line = format!(
                    "FP {name} nodes {} restarts {} mean {:?} weak {:?} hash {:016x}",
                    r.nodes,
                    r.restarts,
                    r.mean_score.map(|m| (m * 100.0).round() / 100.0),
                    r.weak_count,
                    h.finish()
                );
                out.lock().unwrap().push((i, line));
            });
        }
    });
    let cpu = xfill_perf::cpu_s() - c0;
    let wall = w0.elapsed().as_secs_f64();
    let mut out = out.into_inner().unwrap();
    out.sort();
    for (_, l) in &out {
        println!("{l}");
    }
    let nodes = work.load(Relaxed) as f64;
    println!(
        "SUMMARY load {load_ms:.0} ms | dict heap {:.2} MB | patterns {} in {gen_ms:.0} ms | solve wall {:.2} s cpu {:.2} s | nodes {nodes:.0} | {:.1} us/node | allocs {} | solve heap peak +{:.2} MB",
        dict_heap as f64 / 1048576.0, jobs.len(), wall, cpu, cpu * 1e6 / nodes.max(1.0),
        xfill_perf::allocs() - n0, (xfill_perf::peak() - dict_heap) as f64 / 1048576.0
    );
}
