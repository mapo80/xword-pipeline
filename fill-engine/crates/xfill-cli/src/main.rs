//! xfill CLI — load a wordlist + grid template, solve, print.
//!
//! Usage: xfill <template> [--wordlist PATH] [--min-score N] [--time SECS] [--nodes N]

use std::process::ExitCode;
use std::time::Instant;
use xfill_core::clues::load_wordlist;
use xfill_core::grid::Dir;
use xfill_core::{Lock, Puzzle, SolveConfig, SolveResult, Solver};

struct Args {
    template: String,
    wordlist: String,
    min_score: u8,
    time_limit: f64,
    seed: u64,
    tiers: Vec<u8>,
    first: bool,
    boxed: bool,
    workers: usize,
    locks: Vec<Lock>,
}

/// Parse a `--lock` value "ROW,COL,DIR,ANSWER" (DIR is A/Across or D/Down).
fn parse_lock(s: &str) -> Result<Lock, String> {
    let parts: Vec<&str> = s.splitn(4, ',').collect();
    if parts.len() != 4 {
        return Err(format!("--lock expects ROW,COL,DIR,ANSWER, got '{s}'"));
    }
    let row = parts[0].trim().parse().map_err(|_| "bad lock row")?;
    let col = parts[1].trim().parse().map_err(|_| "bad lock col")?;
    let dir = match parts[2].trim().to_ascii_uppercase().as_str() {
        "A" | "ACROSS" => Dir::Across,
        "D" | "DOWN" => Dir::Down,
        other => return Err(format!("bad lock dir '{other}' (use A or D)")),
    };
    Ok(Lock {
        row,
        col,
        dir,
        answer: parts[3].trim().to_string(),
    })
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        template: String::new(),
        wordlist: "../../dataset/Db.csv".to_string(),
        min_score: 0,
        time_limit: 30.0,
        seed: 0,
        tiers: vec![50, 58, 63],
        first: false,
        boxed: false,
        workers: 1,
        locks: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    let mut got_template = false;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--wordlist" => a.wordlist = it.next().ok_or("--wordlist needs a value")?,
            "--min-score" => {
                a.min_score = it
                    .next()
                    .ok_or("--min-score needs a value")?
                    .parse()
                    .map_err(|_| "bad --min-score")?
            }
            "--time" => {
                a.time_limit = it
                    .next()
                    .ok_or("--time needs a value")?
                    .parse()
                    .map_err(|_| "bad --time")?
            }
            "--seed" => {
                a.seed = it
                    .next()
                    .ok_or("--seed needs a value")?
                    .parse()
                    .map_err(|_| "bad --seed")?
            }
            "--tiers" => {
                a.tiers = it
                    .next()
                    .ok_or("--tiers needs a value")?
                    .split(',')
                    .map(|s| s.trim().parse::<u8>().map_err(|_| "bad --tiers"))
                    .collect::<Result<Vec<_>, _>>()?;
            }
            "--first" => a.first = true,
            "--boxed" => a.boxed = true,
            "--workers" => {
                a.workers = it
                    .next()
                    .ok_or("--workers needs a value")?
                    .parse()
                    .map_err(|_| "bad --workers")?
            }
            "--lock" => a
                .locks
                .push(parse_lock(&it.next().ok_or("--lock needs a value")?)?),
            other => {
                if got_template {
                    return Err(format!("unexpected argument: {other}"));
                }
                a.template = other.to_string();
                got_template = true;
            }
        }
    }
    if !got_template {
        return Err("usage: xfill <template> [--wordlist PATH] [--min-score N] [--time SECS] [--seed N] [--tiers 50,58,63] [--first] [--boxed] [--workers N] [--lock ROW,COL,DIR,ANSWER ...]".into());
    }
    Ok(a)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let t = Instant::now();
    let wl = match load_wordlist(&args.wordlist, args.min_score) {
        Ok((w, _clues)) => w,
        Err(e) => {
            eprintln!("failed to load wordlist {}: {e}", args.wordlist);
            return ExitCode::FAILURE;
        }
    };
    let total: usize = (0..=xfill_core::wordlist::MAX_LEN)
        .map(|l| wl.by_len[l].n)
        .sum();
    eprintln!(
        "wordlist: {total} entries (min_score={}) loaded in {:.2}s",
        args.min_score,
        t.elapsed().as_secs_f64()
    );

    let text = match std::fs::read_to_string(&args.template) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("failed to read template {}: {e}", args.template);
            return ExitCode::FAILURE;
        }
    };
    let p = Puzzle::from_template(&text);
    eprintln!(
        "grid: {}x{} blocks={} cells={} entries={} orphans={}",
        p.rows,
        p.cols,
        p.block_count(),
        p.n_cells,
        p.entries.len(),
        p.orphan_cells()
    );

    let cfg = SolveConfig {
        time_limit_s: args.time_limit,
        tiers: args.tiers,
        seed: args.seed,
        stop_on_first: args.first,
        // Italian scores start at 50 (one clue in the database): every word
        // is admissible; single-clue words are the "weak" tail.
        clean_floor: 50,
        weak_bar: 58,
        ..Default::default()
    };

    // Validate theme locks once up front (clear error if a spec is wrong).
    if let Err(e) = Solver::with_locks(&p, &wl, &args.locks) {
        eprintln!("lock error: {e}");
        return ExitCode::FAILURE;
    }
    if !args.locks.is_empty() {
        eprintln!(
            "locked {} theme entr{}",
            args.locks.len(),
            if args.locks.len() == 1 { "y" } else { "ies" }
        );
    }

    // Single grid, optional parallel best-of: each worker runs an independent
    // restart search with a distinct seed; we keep the best fill by mean score
    // and report the combined node/restart totals.
    let locks = &args.locks;
    let r = if args.workers <= 1 {
        Solver::with_locks(&p, &wl, locks).unwrap().solve(&cfg)
    } else {
        let p = &p;
        let wl = &wl;
        let cfg = &cfg;
        std::thread::scope(|s| {
            let handles: Vec<_> = (0..args.workers)
                .map(|k| {
                    let mut cfg_k = cfg.clone();
                    cfg_k.seed = cfg
                        .seed
                        .wrapping_add((k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                    s.spawn(move || Solver::with_locks(p, wl, locks).unwrap().solve(&cfg_k))
                })
                .collect();
            let mut total_nodes = 0u64;
            let mut total_restarts = 0u64;
            let mut best: Option<SolveResult> = None;
            for h in handles {
                let mut res = h.join().unwrap();
                total_nodes += res.nodes;
                total_restarts += res.restarts;
                let take = match &best {
                    None => true,
                    Some(b) => {
                        res.reason == "solved"
                            && b.mean_score
                                .is_none_or(|bm| res.mean_score.is_some_and(|m| m > bm))
                    }
                };
                // The best clean alternative may live in a losing worker —
                // merge it so the combined result carries both maxima.
                if take {
                    if let Some(bc) = best.take().and_then(|b| b.clean) {
                        if res
                            .clean
                            .as_ref()
                            .is_none_or(|rc| bc.mean_score > rc.mean_score)
                        {
                            res.clean = Some(bc);
                        }
                    }
                    best = Some(res);
                } else if let (Some(rc), Some(b)) = (res.clean, best.as_mut()) {
                    if b.clean
                        .as_ref()
                        .is_none_or(|bc| rc.mean_score > bc.mean_score)
                    {
                        b.clean = Some(rc);
                    }
                }
            }
            let mut r = best.unwrap();
            r.nodes = total_nodes;
            r.restarts = total_restarts;
            r
        })
    };

    println!();
    if args.boxed {
        println!("{}", p.render_boxed(r.letters.as_deref()));
    } else {
        println!("{}", p.render(r.letters.as_deref()));
    }
    println!();
    let rate = r.nodes as f64 / r.elapsed_s.max(1e-9);
    println!(
        "reason: {}  nodes: {}  restarts: {}  elapsed: {:.3}s  ({:.0} nodes/s)",
        r.reason, r.nodes, r.restarts, r.elapsed_s, rate
    );
    if let (Some(mean), Some(min), Some(iffy), Some(fill)) =
        (r.mean_score, r.min_score, r.iffy_count, r.fill.as_ref())
    {
        println!(
            "mean score: {:.1}  min: {}  iffy(<50): {}/{}",
            mean,
            min,
            iffy,
            fill.len()
        );
        if let Some(c) = r.clean.as_ref() {
            println!(
                "clean alternative (min {} >= floor {}): mean {:.1}",
                c.min_score, cfg.clean_floor, c.mean_score
            );
        }
        let mut weak: Vec<_> = fill.clone();
        weak.sort_by_key(|(_, _, s)| *s);
        println!("\nweakest entries:");
        for (ei, word, sc) in weak.iter().take(8) {
            let e = &p.entries[*ei];
            let dir = match e.dir {
                xfill_core::grid::Dir::Across => "A",
                xfill_core::grid::Dir::Down => "D",
            };
            println!(
                "  {:16} score={:3}  ({}{} r{}c{})",
                word, sc, dir, e.len, e.row, e.col
            );
        }
        return ExitCode::SUCCESS;
    }
    ExitCode::FAILURE
}
