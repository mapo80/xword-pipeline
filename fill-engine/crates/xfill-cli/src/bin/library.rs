//! Build a curated daily-grid LIBRARY (themeless): generate many valid grids,
//! fill them in parallel, keep only the clean fills (high mean, few/no weak
//! entries), deduplicate, rank by quality, and emit a JSON artifact (template,
//! fill, numbered answers + scores). The vetted source the daily-puzzle pipeline
//! (clue writing, scheduling) draws from.
//!
//! Usage: library [--wordlist PATH] [--size N] [--blocks N] [--block-jitter N]
//!                [--candidates N] [--time SECS] [--keep-mean F] [--max-iffy N]
//!                [--weak-bar N] [--max-weak N] [--min-words N] [--per-shape N]
//!                [--top N] [--seed N] [--bank PATH] [--bank-fraction F]
//!                [--workers N] [--out PATH]
//!
//! `--size` is the grid dimension (default 15; e.g. 5 or 10 for minis). The
//! default block count scales with the grid area when `--blocks` is omitted.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;
use xfill_core::library::{build_lib_grid, write_json, LibGrid};
use xfill_core::util::Rng;
use xfill_core::{gen, Puzzle, SolveConfig, Solver, Wordlist};

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

fn main() {
    let wordlist: String = arg("--wordlist", "data/xwordlist.dict".to_string());
    let size: usize = arg("--size", 15);
    // Default block count scales with grid area (~16%): 15→36, 10→16, 5→4.
    // An explicitly pinned --blocks stays exact; auto varies per candidate
    // (see gen::jittered_blocks) — override either way with --block-jitter.
    let blocks_arg: i64 = arg("--blocks", -1);
    let pinned = blocks_arg >= 0;
    let blocks: usize = if pinned {
        blocks_arg as usize
    } else {
        size * size * 16 / 100
    };
    let block_jitter: usize = arg(
        "--block-jitter",
        if pinned {
            0
        } else {
            gen::default_block_jitter(size)
        },
    );
    let candidates: usize = arg("--candidates", 200);
    let time: f64 = arg("--time", 2.0);
    let keep_mean: f64 = arg("--keep-mean", 72.0);
    let max_iffy: usize = arg("--max-iffy", 3);
    let top: usize = arg("--top", 25);
    let seed: u64 = arg("--seed", 1);
    // Reject candidate templates below this word count at generation time —
    // higher word counts fill clean far more reliably (see gen::word_floor).
    // -1 (default) = auto per size; 0 = off; explicit N = that floor.
    let min_words_arg: i64 = arg("--min-words", -1);
    let min_words: usize = if min_words_arg >= 0 {
        min_words_arg as usize
    } else {
        gen::word_floor(size)
    };
    // Entries scoring below the weak bar are the gluey tail an editor counts
    // (see SolveConfig::weak_bar). -1 = pipeline default; 0 = off. Keepers are
    // ordered fewest-weak-first, and --max-weak (default: no gate) drops
    // fills with a longer tail.
    let weak_bar_arg: i64 = arg("--weak-bar", -1);
    let weak_bar: u8 = if weak_bar_arg >= 0 {
        weak_bar_arg as u8
    } else {
        xfill_core::solver::DEFAULT_WEAK_BAR
    };
    let max_weak: usize = arg("--max-weak", usize::MAX);
    // Keep at most this many fills of any one template (0 = unlimited). Small
    // symmetric grids have tiny shape spaces — five 5x5 templates in total —
    // so several fills per shape ARE the variety; see library::select_keepers.
    let per_shape: usize = arg("--per-shape", xfill_core::library::DEFAULT_PER_SHAPE);
    let out: String = arg("--out", "../out/libraries/grid-library.json".to_string());
    // Template bank: draw this fraction of candidates from clean-proven
    // shapes (see xfill_core::bank; build with the tpl-bank bin). -1 = auto
    // (`bank::default_bank_fraction`: 0.7 full-size, 0 below); 0 = fresh only.
    let bank_path: String = arg("--bank", format!("data/tpl-bank-{size}.txt"));
    let bank_fraction_arg: f64 = arg("--bank-fraction", -1.0);
    let workers: usize = arg(
        "--workers",
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
    );

    let t = Instant::now();
    let wl = Wordlist::load(&wordlist, 40).expect("load wordlist");
    eprintln!("wordlist loaded in {:.1}s", t.elapsed().as_secs_f64());

    // Phase 1: generate templates + fixed per-grid seeds (deterministic).
    // Banked (proven) templates first — under the early stop, publication
    // runs spend their budget on shapes that can actually clean-fill.
    let mut rng = Rng::new(seed);
    let mut jobs: Vec<(String, u64)> = Vec::with_capacity(candidates);
    let bank_templates = xfill_core::bank::parse_bank(
        &std::fs::read_to_string(&bank_path).unwrap_or_default(),
        size,
    );
    let bank_fraction = if bank_fraction_arg >= 0.0 {
        bank_fraction_arg.min(1.0)
    } else {
        xfill_core::bank::default_bank_fraction(size)
    };
    let n_banked = ((candidates as f64) * bank_fraction).round() as usize;
    let mut banked_used = 0usize;
    for tpl in xfill_core::bank::sample(&bank_templates, n_banked, &mut rng) {
        let p = Puzzle::from_template(&tpl);
        if p.orphan_cells() > 0 || p.entries.len() < min_words {
            continue; // bank line predates a stricter floor — skip, don't trust
        }
        jobs.push((tpl, rng.next_u64()));
        banked_used += 1;
    }
    let banked_set: HashSet<String> = jobs.iter().map(|(t, _)| t.clone()).collect();
    // Bound the search so an infeasible size/blocks combo fails fast instead of
    // spinning forever (generate() returns None when it can't satisfy the spec).
    let mut tries = 0usize;
    let max_tries = candidates.saturating_mul(200).max(20_000);
    let mut eff_min_words = min_words;
    while jobs.len() < candidates && tries < max_tries {
        tries += 1;
        // Relax the word floor once if it's starving generation (an open,
        // low-block build): it improves fill quality but must never hard-fail.
        if eff_min_words > 0 && tries * 5 >= max_tries * 3 {
            eprintln!(
                "  word floor {eff_min_words} yielded only {}/{candidates} templates; relaxing",
                jobs.len()
            );
            eff_min_words = 0;
        }
        let jseed = rng.next_u64();
        let b = gen::jittered_blocks(blocks, block_jitter, &mut rng);
        let Some(tpl) = gen::generate(size, b, &mut rng, 4000) else {
            continue;
        };
        let p = Puzzle::from_template(&tpl);
        if p.orphan_cells() > 0 || p.entries.len() < eff_min_words {
            continue;
        }
        if banked_set.contains(&tpl) {
            continue; // don't double-fill a shape the bank already queued
        }
        jobs.push((tpl, jseed));
    }
    if jobs.is_empty() {
        eprintln!(
            "error: could not generate any valid {size}x{size} grids with {blocks} blocks \
             after {tries} tries. Try fewer --blocks or a different --size."
        );
        std::process::exit(1);
    }
    eprintln!(
        "generated {} candidate {size}x{size} grids{}{}; filling with {workers} workers...",
        jobs.len(),
        if banked_used > 0 {
            format!(" ({banked_used} banked)")
        } else {
            String::new()
        },
        if min_words > 0 {
            format!(" (word floor {min_words})")
        } else {
            String::new()
        }
    );

    // Phase 2: parallel fill, keep only clean fills. Workers stop early once
    // the library has plenty of clean grids (2x what we'll keep).
    let stop_at = top * 2;
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let kept: Mutex<Vec<LibGrid>> = Mutex::new(Vec::new());
    let kept_count = AtomicUsize::new(0);
    let filled = AtomicUsize::new(0);
    let dup_rejects = AtomicUsize::new(0);
    let dup_rescues = AtomicUsize::new(0);
    let dup_examples: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let dup_checker = xfill_core::dup::DupChecker::new(&wl);
    let no_theme: HashSet<usize> = HashSet::new();
    let t0 = Instant::now();

    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                if kept_count.load(Ordering::Relaxed) >= stop_at {
                    break;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= jobs.len() {
                    break;
                }
                let (tpl, jseed) = &jobs[i];
                let p = Puzzle::from_template(tpl);
                let mut solver = Solver::new(&p, &wl);
                let mut cfg = SolveConfig {
                    time_limit_s: time,
                    tiers: vec![40, 50, 55, 60, 70],
                    seed: *jseed,
                    weak_bar,
                    ..Default::default()
                };
                // Solve → gate → dup-check, retrying up to twice with the
                // most disposable dup member banned: about half of the
                // gate-passing fills die to root-duplicate answers, and a
                // short ban-and-refill rescues most of them.
                let mut bans = 0usize;
                let kept_grid = loop {
                    let r = solver.solve(&cfg);
                    if bans == 0 {
                        let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                        if d.is_multiple_of(50) {
                            eprintln!("  filled {d}/{} ...", jobs.len());
                        }
                    }
                    // Prefer the clean fill (nothing below the solver's clean
                    // floor) whenever it passes the keep gates on its own —
                    // same yield, strictly fewer weak entries.
                    let clean_pick = r.clean.as_ref().filter(|c| {
                        c.mean_score >= keep_mean
                            && c.iffy_count <= max_iffy
                            && c.weak_count <= max_weak
                    });
                    let g = match clean_pick {
                        Some(c) => Some(xfill_core::library::build_lib_grid_from(&p, c, &no_theme)),
                        None => build_lib_grid(&p, &r, &no_theme),
                    };
                    let Some(g) = g else {
                        break None;
                    };
                    if bans == 0 {
                        filled.fetch_add(1, Ordering::Relaxed);
                    }
                    if g.mean < keep_mean || g.iffy > max_iffy || g.weak > max_weak {
                        break None;
                    }
                    // Root-duplicate gate (TEN/TENTH, EVEN/UNEVENLY, shared
                    // embedded words): editors flag these as high-severity
                    // dups, so don't keep such grids.
                    let answers: Vec<(String, bool)> = g
                        .entries
                        .iter()
                        .map(|e| (e.answer.clone(), e.theme))
                        .collect();
                    let Some((a, b)) = dup_checker.find_dup(&answers) else {
                        break Some(g);
                    };
                    let banned_one = bans < 2
                        && xfill_core::library::dup_ban_targets(&g, &a, &b)
                            .iter()
                            .any(|t| solver.ban_answer(t));
                    if !banned_one {
                        dup_rejects.fetch_add(1, Ordering::Relaxed);
                        let mut ex = dup_examples.lock().unwrap();
                        if ex.len() < 5 {
                            ex.push(format!("{a}/{b}"));
                        }
                        break None;
                    }
                    bans += 1;
                    // Refills are cheap — the template provably fills — so
                    // half the original budget recovers it almost always.
                    cfg.time_limit_s = time * 0.5;
                    cfg.seed = cfg.seed.wrapping_add(1);
                };
                let Some(g) = kept_grid else {
                    continue;
                };
                if bans > 0 {
                    dup_rescues.fetch_add(1, Ordering::Relaxed);
                }
                kept.lock().unwrap().push(g);
                kept_count.fetch_add(1, Ordering::Relaxed);
            });
        }
    });
    let dur = t0.elapsed().as_secs_f64();

    // Fewest-weak-then-mean order, exact-duplicate fills dropped, at most
    // `per_shape` fills of any one template — sorting BEFORE the dedup is what
    // keeps each shape's best fill rather than its first-finished one.
    let mut kept = xfill_core::library::select_keepers(kept.into_inner().unwrap(), per_shape);
    kept.truncate(top);

    let filled_n = filled.load(Ordering::Relaxed);
    let done_n = done.load(Ordering::Relaxed);
    eprintln!(
        "\nfilled {filled_n}/{done_n} in {dur:.1}s; {} clean grids kept over {} shapes (mean>={keep_mean}, iffy<={max_iffy})",
        kept.len(),
        xfill_core::library::distinct_shapes(&kept)
    );
    if done_n < jobs.len() {
        eprintln!(
            "(early stop: {stop_at} clean grids kept after {done_n} of {} candidates)",
            jobs.len()
        );
    }
    let dup_n = dup_rejects.load(Ordering::Relaxed);
    if dup_n > 0 {
        let ex = dup_examples.into_inner().unwrap();
        eprintln!(
            "({dup_n} clean fill(s) rejected for root-duplicate answers: {})",
            ex.join(", ")
        );
    }
    let rescue_n = dup_rescues.load(Ordering::Relaxed);
    if rescue_n > 0 {
        eprintln!("({rescue_n} dup-hit fill(s) rescued by ban-and-refill)");
    }

    write_json(&out, &kept, &wordlist, blocks, &[]).expect("write library file");
    eprintln!("wrote {} grids to {out}", kept.len());
    if let Some(best) = kept.first() {
        eprintln!(
            "best: mean={:.1} iffy={} weak={} (bar {weak_bar}) blocks={}",
            best.mean, best.iffy, best.weak, best.blocks
        );
    }
}
