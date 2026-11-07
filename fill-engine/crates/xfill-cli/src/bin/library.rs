//! Build a library of filled Italian-style crosswords: generate many random
//! block patterns for the requested size and black-square count, fill them in
//! parallel, keep the best fills, rank by quality and emit a JSON artifact
//! (template, fill, numbered answers + scores).
//!
//! Usage: library [--wordlist PATH] [--size RxC] [--blocks N]
//!                [--symmetry none|rot|mirror] [--max-len N] [--no-adjacent]
//!                [--gen random|search] [--max-two-letter PCT]
//!                [--min-len-count L:N,…] [--require-lengths L,…]
//!                [--max-full-words N] [--max-white-rect A] [--lattice]
//!                [--llm-clues off|missing|all] [--llm-config FILE] [--llm-model M]
//!                [--candidates N] [--time SECS] [--keep-mean F]
//!                [--weak-bar N] [--max-weak N] [--per-shape N] [--top N]
//!                [--seed N] [--workers N] [--out PATH]
//!
//! `--size` is ROWSxCOLS (e.g. `13x21`) or a single N for a square grid.
//! `--blocks` defaults to ~15% of the cells.

use std::time::Instant;
use xfill_core::api::{assign_clues, cancel_flag, fill_patterns, generate_patterns, Plan};
use xfill_core::clues::load_clue_csv_rebus;
use xfill_core::gen_it::{GenMode, ItGenConfig, Symmetry};
use xfill_core::library::write_json;
use xfill_core::wordlist::MAX_LEN;
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

/// "13x21" → (13, 21); "15" → (15, 15).
fn parse_size(s: &str) -> Option<(usize, usize)> {
    let s = s.to_ascii_lowercase();
    match s.split_once('x') {
        Some((r, c)) => Some((r.trim().parse().ok()?, c.trim().parse().ok()?)),
        None => {
            let n = s.trim().parse().ok()?;
            Some((n, n))
        }
    }
}

fn main() {
    let wordlist: String = arg("--wordlist", "../../dataset/Db.csv".to_string());
    let size_s: String = arg("--size", "13x13".to_string());
    let Some((rows, cols)) = parse_size(&size_s).filter(|&(r, c)| r >= 2 && c >= 2) else {
        eprintln!("error: bad --size '{size_s}' (use ROWSxCOLS, e.g. 13x21)");
        std::process::exit(2);
    };
    let blocks: usize = arg("--blocks", rows * cols * 15 / 100);
    let sym_s: String = arg("--symmetry", "rot".to_string());
    let Some(symmetry) = Symmetry::parse(&sym_s) else {
        eprintln!("error: bad --symmetry '{sym_s}' (use none, rot or mirror)");
        std::process::exit(2);
    };
    let max_len_arg: usize = arg("--max-len", rows.max(cols)).min(MAX_LEN);
    let allow_adjacent = !flag("--no-adjacent");
    // benchmark (40 patterns × 3 families): search 53 filled vs random 43
    let gen_s: String = arg("--gen", "search".to_string());
    let Some(gen_mode) = GenMode::parse(&gen_s) else {
        eprintln!("error: bad --gen '{gen_s}' (use random or search)");
        std::process::exit(2);
    };
    let max_two_letter_pct: Option<u8> = std::env::args()
        .skip(1)
        .any(|a| a == "--max-two-letter")
        .then(|| arg("--max-two-letter", 100u8));
    let candidates: usize = arg("--candidates", 200);
    let time: f64 = arg("--time", 2.0);
    let keep_mean: f64 = arg("--keep-mean", 0.0);
    let weak_bar: u8 = arg("--weak-bar", SolveConfig::default().weak_bar);
    let max_weak: usize = arg("--max-weak", usize::MAX);
    let per_shape: usize = arg("--per-shape", xfill_core::library::DEFAULT_PER_SHAPE);
    let top: usize = arg("--top", 10);
    let seed: u64 = arg("--seed", 1);
    let out: String = arg("--out", "../out/libraries/grid-library.json".to_string());
    let pdf_dir: String = arg("--pdf", String::new());
    // LLM clues: off (default), missing (only words without a clue in the
    // database), all (every word); credentials file: URL, then API key.
    let llm_mode: String = arg("--llm-clues", "off".to_string());
    let llm_all = match llm_mode.as_str() {
        "off" | "missing" => false,
        "all" => true,
        _ => {
            eprintln!("error: bad --llm-clues '{llm_mode}' (use off, missing or all)");
            std::process::exit(2);
        }
    };
    let llm = if llm_mode == "off" {
        None
    } else {
        let path: String = arg("--llm-config", "../../.alibaba".to_string());
        let model: String = arg("--llm-model", "qwen3.8-flash".to_string());
        match xfill_core::llm::LlmConfig::load(&path, &model) {
            Ok(cfg) => Some(xfill_core::llm::CurlClient { cfg, max_chars: 40 }),
            Err(e) => {
                eprintln!("error: cannot read LLM config {path}: {e}");
                std::process::exit(1);
            }
        }
    };
    let cli: Vec<String> = std::env::args().skip(1).collect();
    let mut base_cfg = match SolveConfig::default().with_args(&cli) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    let workers: usize = arg(
        "--workers",
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
    );

    eprintln!("solver: {}", base_cfg.describe());
    let t = Instant::now();
    // free rebus strings the solver may place in any cell
    let rebus: Vec<String> = arg("--rebus", String::new())
        .split(',')
        .map(|r| r.trim().to_ascii_uppercase())
        .filter(|r| !r.is_empty())
        .collect();
    base_cfg.free_rebus = rebus.clone();
    let (wl, clues) = match load_clue_csv_rebus(&wordlist, 0, &rebus) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("error: cannot load wordlist {wordlist}: {e}");
            std::process::exit(1);
        }
    };
    // Never ask for runs longer than the dictionary's longest word.
    let max_len = max_len_arg.min(wl.longest());
    let theme_list: String = arg("--theme-list", String::new());
    if !theme_list.is_empty() {
        match xfill_core::solver::load_theme(&theme_list, &wl) {
            Ok((ids, total)) => {
                eprintln!("theme: {} of {total} words in the dictionary", ids.len());
                base_cfg.theme = std::sync::Arc::new(ids);
            }
            Err(e) => {
                eprintln!("error: cannot read theme list {theme_list}: {e}");
                std::process::exit(1);
            }
        }
    }
    let total: usize = wl.by_len.iter().map(|l| l.n).sum();
    eprintln!(
        "wordlist: {total} words loaded in {:.1}s",
        t.elapsed().as_secs_f64()
    );

    // Generator constraints
    let list = |name: &str| -> Result<Vec<usize>, String> {
        let v: String = arg(name, String::new());
        v.split(',')
            .filter(|x| !x.trim().is_empty())
            .map(|x| x.trim().parse().map_err(|_| format!("bad {name} '{v}'")))
            .collect()
    };
    let parsed = (|| -> Result<_, String> {
        let min_len_counts: Vec<(usize, usize)> = {
            let v: String = arg("--min-len-count", String::new());
            v.split(',')
                .filter(|x| !x.trim().is_empty())
                .map(|x| {
                    let (l, n) = x
                        .split_once(':')
                        .ok_or(format!("bad --min-len-count '{v}' (use L:N,…)"))?;
                    Ok((
                        l.trim()
                            .parse()
                            .map_err(|_| format!("bad --min-len-count '{v}'"))?,
                        n.trim()
                            .parse()
                            .map_err(|_| format!("bad --min-len-count '{v}'"))?,
                    ))
                })
                .collect::<Result<_, String>>()?
        };
        Ok((min_len_counts, list("--require-lengths")?))
    })();
    let (min_len_counts, require_lengths) = match parsed {
        Ok(x) => x,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    let max_full_words: Option<usize> =
        flag("--max-full-words").then(|| arg("--max-full-words", usize::MAX));
    let max_white_rect: Option<usize> =
        flag("--max-white-rect").then(|| arg("--max-white-rect", usize::MAX));
    let lattice = flag("--lattice");

    // Pattern generator constraints.
    let gen_cfg = ItGenConfig {
        symmetry,
        max_len,
        allow_adjacent,
        mode: gen_mode,
        max_two_letter_pct,
        min_len_counts,
        max_full_words,
        require_lengths,
        max_white_rect,
        lattice,
        ..ItGenConfig::new(rows, cols, blocks)
    };
    eprintln!(
        "constraints: min-len-count {:?}, require-lengths {:?}, max-full-words {:?}, \
         max-white-rect {:?}{}",
        gen_cfg.min_len_counts,
        gen_cfg.require_lengths,
        gen_cfg.max_full_words,
        gen_cfg.max_white_rect,
        if gen_cfg.lattice { ", lattice" } else { "" }
    );
    // Phases 1-2 (pattern generation, parallel fill) live in the core API;
    // workers stop once the library has plenty of keepers (2x what we'll
    // output).
    let plan = Plan {
        gen: gen_cfg,
        solve: SolveConfig {
            time_limit_s: time,
            weak_bar,
            ..base_cfg.clone()
        },
        candidates,
        gen_tries: 50,
        min_mean: keep_mean,
        max_weak,
        stop_after: top * 2,
        threads: workers,
        seed,
    };
    let cancel = cancel_flag();
    let patterns = generate_patterns(&wl, &plan, &cancel);
    let jobs = patterns.jobs;
    if jobs.is_empty() {
        eprintln!(
            "error: could not generate any valid {rows}x{cols} pattern with {blocks} blacks \
             after {} tries. Try more --blocks, --no-adjacent off, or a larger --max-len.",
            patterns.tries
        );
        std::process::exit(1);
    }
    eprintln!(
        "generated {} candidate {rows}x{cols} patterns ({blocks} blacks, symmetry {sym_s}, \
         gen {gen_s}; {} infeasible skipped); filling with {workers} workers...",
        jobs.len(),
        patterns.infeasible
    );

    let run = fill_patterns(&wl, &plan, &jobs, &cancel, &|d, n| {
        if d.is_multiple_of(50) {
            eprintln!("  filled {d}/{n} ...");
        }
    });
    let dur = run.elapsed_s;
    let stop_at = plan.stop_after;

    let mut kept = xfill_core::library::select_keepers(run.kept, per_shape, base_cfg.theme_weight);
    kept.truncate(top);

    let filled_n = run.filled;
    let done_n = run.done;
    eprintln!(
        "\nfilled {filled_n}/{done_n} in {dur:.1}s; {} grids kept over {} shapes",
        kept.len(),
        xfill_core::library::distinct_shapes(&kept)
    );
    if done_n < jobs.len() {
        eprintln!(
            "(early stop: {stop_at} grids kept after {done_n} of {} candidates)",
            jobs.len()
        );
    }

    // Clue every kept grid from the clue database (seeded, reproducible).
    assign_clues(&mut kept, &clues, seed);
    // then the language model, for missing (or all) clues
    if let Some(client) = &llm {
        for g in &mut kept {
            let mut items: Vec<(String, Option<String>)> = g
                .entries
                .iter()
                .map(|e| (e.answer.clone(), e.clue.clone()))
                .collect();
            match xfill_core::llm::fill_clues(&mut items, client, llm_all) {
                Ok(n) => {
                    eprintln!("llm: {n} clue(s) written by {}", client.cfg.model);
                    for (e, (_, c)) in g.entries.iter_mut().zip(items) {
                        e.clue = c;
                    }
                }
                Err(err) => eprintln!("warning: LLM clues skipped: {err}"),
            }
        }
    }

    write_json(&out, &kept, &wordlist, blocks).expect("write library file");
    eprintln!("wrote {} grids to {out}", kept.len());

    if !pdf_dir.is_empty() {
        if let Err(e) = std::fs::create_dir_all(&pdf_dir) {
            eprintln!("error: cannot create {pdf_dir}: {e}");
            std::process::exit(1);
        }
        for (i, g) in kept.iter().enumerate() {
            let title = format!("Cruciverba n. {}", i + 1);
            let subtitle = format!(
                "Schema {rows}×{cols} · {} caselle nere · {} parole",
                g.blocks,
                g.entries.len()
            );
            let pz = xfill_core::library::to_pdf_puzzle(g, &title, &subtitle);
            let path = std::path::Path::new(&pdf_dir).join(format!("cruciverba-{:02}.pdf", i + 1));
            std::fs::write(&path, xfill_core::pdf::render_pdf(&pz)).expect("write pdf");
            eprintln!("pdf: {}", path.display());
        }
    }
    if let Some(best) = kept.first() {
        eprintln!(
            "best: mean={:.1} weak={} (bar {weak_bar}) blocks={}",
            best.mean, best.weak, best.blocks
        );
    }
}
