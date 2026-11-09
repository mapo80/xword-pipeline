//! Full generation benchmark (api::generate): wall time, CPU time, heap peak, allocations per phase.
//! usage: gen <Db.csv> [threads] [seeds] [formats e.g. 13x13,15x15]

use std::time::Instant;
use xfill_core::api::{cancel_flag, generate, Dictionary, GenerateRequest};

#[global_allocator]
static A: xfill_perf::Counting = xfill_perf::Counting;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = &a[1];
    let threads: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
    let seeds: u64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);
    let formats = a
        .get(4)
        .cloned()
        .unwrap_or("13x13,15x15,12x18,12x22".into());

    let bytes = std::fs::read(path).unwrap();

    xfill_perf::reset_peak();
    let (t0, c0, n0) = (Instant::now(), xfill_perf::cpu_s(), xfill_perf::allocs());
    let dict = Dictionary::from_csv_bytes(&bytes).unwrap();
    drop(bytes);
    println!(
        "LOAD wall {:.0} ms cpu {:.0} ms | heap live {:.1} MB peak {:.1} MB (est heap_bytes {:.1} MB) | allocs {} | words {} clues {}",
        t0.elapsed().as_secs_f64() * 1e3,
        (xfill_perf::cpu_s() - c0) * 1e3,
        xfill_perf::mb(xfill_perf::live()),
        xfill_perf::mb(xfill_perf::peak()),
        xfill_perf::mb(dict.heap_bytes()),
        xfill_perf::allocs() - n0,
        dict.word_count(),
        dict.clues.len()
    );
    let wl_b = dict.words.heap_bytes();
    let mut per_len = String::new();
    for l in &dict.words.by_len {
        if l.n > 0 {
            per_len += &format!("{}:{} ", l.len, l.n);
        }
    }
    println!(
        "  wordlist {:.2} MB clues {:.2} MB",
        xfill_perf::mb(wl_b),
        xfill_perf::mb(dict.clues.heap_bytes())
    );
    println!("  per length {per_len}");

    let dict_live = xfill_perf::live();
    for f in formats.split(',') {
        let (r, c) = f.split_once('x').unwrap();
        let (r, c): (usize, usize) = (r.parse().unwrap(), c.parse().unwrap());
        let mut walls = vec![];
        let mut cpus = vec![];
        let mut peaks = vec![];
        let mut allocs = vec![];
        let mut abytes = vec![];
        let mut nodes = vec![];
        let mut oks = 0;
        let mut scores = vec![];
        for seed in 1..=seeds {
            let req = GenerateRequest {
                rows: r,
                cols: c,
                seed,
                threads,
                ..Default::default()
            };
            xfill_perf::reset_peak();
            let (t0, c0, n0, b0) = (
                Instant::now(),
                xfill_perf::cpu_s(),
                xfill_perf::allocs(),
                xfill_perf::alloc_bytes(),
            );
            let res = generate(&dict, &req, &cancel_flag());
            walls.push(t0.elapsed().as_secs_f64() * 1e3);
            cpus.push((xfill_perf::cpu_s() - c0) * 1e3);
            peaks.push(xfill_perf::mb(xfill_perf::peak() - dict_live));
            allocs.push((xfill_perf::allocs() - n0) as f64);
            abytes.push(xfill_perf::mb(xfill_perf::alloc_bytes() - b0));
            nodes.push(res.nodes as f64);
            if !res.entries.is_empty() {
                oks += 1;
                scores.push(res.score);
            }
            if std::env::var("PERF_FP").is_ok() {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                res.grid.hash(&mut h);
                for e in &res.entries {
                    e.answer.hash(&mut h);
                    e.clue.hash(&mut h);
                }
                println!(
                    "FP {f} seed {seed} nodes {} attempts {} filled {} score {:.2} hash {:016x}",
                    res.nodes,
                    res.attempts,
                    res.filled,
                    res.score,
                    h.finish()
                );
            }
            if std::env::var("PERF_VERBOSE").is_ok() {
                println!("    seed {seed}: {:?} {} ms nodes {} attempts {} filled {} patterns {} score {:.2}", res.status, res.duration_ms, res.nodes, res.attempts, res.filled, res.patterns, res.score);
            }
        }
        let avg = |v: &Vec<f64>| v.iter().sum::<f64>() / v.len() as f64;
        println!(
            "GEN {f} thr {threads}: ok {oks}/{seeds} | wall {:.0} ms (min {:.0} max {:.0}) | cpu {:.0} ms | heap peak +{:.2} MB | allocs {:.0} ({:.0} MB churn) | nodes {:.0} | score {:.2} | ns/node {:.0}",
            avg(&walls), walls.iter().cloned().fold(f64::MAX, f64::min), walls.iter().cloned().fold(0.0, f64::max),
            avg(&cpus), avg(&peaks), avg(&allocs), avg(&abytes), avg(&nodes), avg(&scores), avg(&cpus) * 1e6 / avg(&nodes).max(1.0)
        );
    }
    // PERF_PAUSE=secs: wait before exiting (inspect with vmmap / footprint)
    if let Some(t) = std::env::var("PERF_PAUSE")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    {
        eprintln!("pid {}", std::process::id());
        std::thread::sleep(std::time::Duration::from_secs(t));
    }
    println!("MAX RSS {:.1} MB", xfill_perf::max_rss_mb());
}
