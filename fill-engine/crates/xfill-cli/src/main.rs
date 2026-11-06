//! xfill CLI — load a wordlist + grid template, solve, print.
//!
//! Usage: xfill <template> [--wordlist PATH] [--min-score N] [--time SECS] [--nodes N]

use std::process::ExitCode;
use std::time::Instant;
use xfill_core::clues::load_clue_csv;
use xfill_core::grid::Dir;
use xfill_core::{Lock, Puzzle, SolveConfig, Solver};

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
    analyze: bool,
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
        tiers: SolveConfig::default().tiers,
        first: false,
        boxed: false,
        workers: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
        locks: Vec::new(),
        analyze: false,
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
            "--analyze" => a.analyze = true,
            // solver variants: validated by SolveConfig::with_args in main
            f if xfill_core::solver::SOLVER_FLAGS.contains(&f) => {
                it.next();
            }
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
        return Err("usage: xfill <template> [--wordlist PATH] [--min-score N] [--time SECS] [--seed N] [--tiers 50,58,63] [--first] [--boxed] [--workers N] [--lock ROW,COL,DIR,ANSWER ...] [--analyze]".into());
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
    let wl = match load_clue_csv(&args.wordlist, args.min_score) {
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

    let cli: Vec<String> = std::env::args().skip(1).collect();
    let cfg = match (SolveConfig {
        time_limit_s: args.time_limit,
        tiers: args.tiers,
        seed: args.seed,
        stop_on_first: args.first,
        ..Default::default()
    })
    .with_args(&cli)
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("solver: {}", cfg.describe());

    // Validate theme locks once up front (clear error if a spec is wrong).
    if let Err(e) = Solver::with_locks(&p, &wl, &args.locks) {
        eprintln!("lock error: {e}");
        return ExitCode::FAILURE;
    }
    // Slots whose length has no dictionary words can't be filled (locked
    // answers are exempt: they need not be in the dictionary).
    let locked_lens: Vec<usize> = args
        .locks
        .iter()
        .filter_map(|l| p.find_entry(l.row, l.col, l.dir))
        .map(|ei| p.entries[ei].len)
        .collect();
    for (len, n) in wl.missing_lengths(&p) {
        let unlocked = n - locked_lens.iter().filter(|&&l| l == len).count();
        if unlocked > 0 {
            eprintln!("error: no dictionary words of length {len} (needed by {unlocked} slot(s))");
            return ExitCode::FAILURE;
        }
    }
    if !args.locks.is_empty() {
        eprintln!(
            "locked {} theme entr{}",
            args.locks.len(),
            if args.locks.len() == 1 { "y" } else { "ies" }
        );
    }

    if args.analyze {
        return print_analysis(
            &p,
            &Solver::with_locks(&p, &wl, &args.locks).unwrap().analyze(),
        );
    }

    // Parallel portfolio of independent searches (distinct seeds); with
    // --first the first worker to solve stops the others.
    eprintln!(
        "solving with {} worker{}",
        args.workers,
        if args.workers == 1 { "" } else { "s" }
    );
    let r = match xfill_core::solve_portfolio(&p, &wl, &args.locks, &cfg, args.workers) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("lock error: {e}");
            return ExitCode::FAILURE;
        }
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
    if let (Some(mean), Some(min), Some(fill)) = (r.mean_score, r.min_score, r.fill.as_ref()) {
        println!(
            "mean score: {:.1}  min: {}  weak(<{}): {}/{}",
            mean,
            min,
            cfg.weak_bar,
            r.weak_count.unwrap_or(0),
            fill.len()
        );
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

/// Print what the grid allows without searching; success iff feasible.
fn print_analysis(p: &Puzzle, a: &xfill_core::Analysis) -> ExitCode {
    println!(
        "analysis: {}",
        if a.feasible {
            "feasible"
        } else {
            "NOT feasible"
        }
    );
    for &e in &a.impossible {
        let en = &p.entries[e];
        let d = if en.dir == xfill_core::grid::Dir::Across {
            "A"
        } else {
            "D"
        };
        println!("impossible: {d}{} r{}c{}", en.len, en.row, en.col);
    }
    if !a.forced.is_empty() {
        let f: Vec<String> = a
            .forced
            .iter()
            .map(|&(r, c, l)| format!("r{r}c{c}={}", l as char))
            .collect();
        println!("forced: {}", f.join(" "));
    }
    // per cell: the letter if only one is possible, else how many (+ = 10+)
    println!("letters still possible per cell:");
    let mut grid = vec![vec!['#'; p.cols]; p.rows];
    for (cid, &m) in a.cell_letters.iter().enumerate() {
        let (r, c) = p.cell_rc[cid];
        grid[r][c] = match m.count_ones() {
            0 => '0',
            1 => (b'A' + m.trailing_zeros() as u8) as char,
            n if n < 10 => char::from_digit(n, 10).unwrap(),
            _ => '+',
        };
    }
    for row in grid {
        println!("{}", row.into_iter().collect::<String>());
    }
    if a.feasible {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
