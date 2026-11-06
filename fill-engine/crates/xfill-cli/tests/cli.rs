//! End-to-end tests of the `xfill` and `library` binaries against a small
//! synthetic dictionary (every 2..=5-letter string over A, B, C, E, I, O),
//! so they don't depend on the real dataset.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let p = std::env::temp_dir().join(format!("xfill-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    fn path(&self, f: &str) -> PathBuf {
        self.0.join(f)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// CSV with every word over a 6-letter alphabet, lengths 2..=max_len; each
/// word gets one or two clues so scores vary.
fn write_dict(dir: &TempDir, max_len: usize) -> PathBuf {
    let alpha = ['A', 'B', 'C', 'E', 'I', 'O'];
    let mut words: Vec<String> = alpha.iter().map(|c| c.to_string()).collect();
    let mut all = Vec::new();
    for _ in 2..=max_len {
        words = words
            .iter()
            .flat_map(|w| alpha.iter().map(move |c| format!("{w}{c}")))
            .collect();
        all.extend(words.iter().cloned());
    }
    let mut csv = String::new();
    for (i, w) in all.iter().enumerate() {
        csv.push_str(&format!("{w},Definizione di {w}\n"));
        if i % 3 == 0 {
            csv.push_str(&format!("{w},Altra definizione di {w}\n"));
        }
    }
    let p = dir.path("dict.csv");
    std::fs::write(&p, csv).unwrap();
    p
}

fn run(bin: &str, args: &[&str]) -> Output {
    Command::new(bin).args(args).output().expect("binary runs")
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

const XFILL: &str = env!("CARGO_BIN_EXE_xfill");
const LIBRARY: &str = env!("CARGO_BIN_EXE_library");

#[test]
fn xfill_fills_a_template_from_csv() {
    let d = TempDir::new("fill");
    let dict = write_dict(&d, 4);
    let tpl = d.path("t.txt");
    std::fs::write(&tpl, "....\n.#..\n....\n").unwrap();
    let out = run(
        XFILL,
        &[s(&tpl), "--wordlist", s(&dict), "--time", "2", "--boxed"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("reason: solved"), "{stdout}");
    assert!(stdout.contains('┌'), "boxed render");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    assert!(
        stderr.contains(&format!("solving with {cpus} worker")),
        "one worker per CPU by default: {stderr}"
    );
}

#[test]
fn xfill_reports_slot_lengths_missing_from_dictionary() {
    let d = TempDir::new("missing-len");
    let dict = write_dict(&d, 3);
    let tpl = d.path("t.txt");
    std::fs::write(&tpl, "......\n").unwrap(); // a 6-letter slot, dict stops at 3
    let out = run(XFILL, &[s(&tpl), "--wordlist", s(&dict), "--time", "1"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(!stderr.contains("panicked"), "must not panic: {stderr}");
    assert!(
        stderr.contains("no dictionary words of length 6"),
        "names the missing length: {stderr}"
    );
}

#[test]
fn xfill_rejects_bad_lock_and_missing_template() {
    let d = TempDir::new("errors");
    let dict = write_dict(&d, 4);
    let tpl = d.path("t.txt");
    std::fs::write(&tpl, "....\n....\n").unwrap();
    let out = run(
        XFILL,
        &[s(&tpl), "--wordlist", s(&dict), "--lock", "0,0,A,TOOLONG"],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("lock error"));

    let out = run(XFILL, &[s(&d.path("nope.txt")), "--wordlist", s(&dict)]);
    assert!(!out.status.success());

    let out = run(XFILL, &[]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage"));
}

#[test]
fn xfill_parallel_workers_and_lock() {
    let d = TempDir::new("workers");
    let dict = write_dict(&d, 4);
    let tpl = d.path("t.txt");
    std::fs::write(&tpl, "....\n....\n....\n").unwrap();
    let out = run(
        XFILL,
        &[
            s(&tpl),
            "--wordlist",
            s(&dict),
            "--workers",
            "3",
            "--time",
            "1",
            "--lock",
            "0,0,A,ABBA",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.lines().any(|l| l.starts_with("ABBA")), "{stdout}");
}

#[test]
fn library_writes_json_with_clues_and_pdfs() {
    let d = TempDir::new("library");
    let dict = write_dict(&d, 5);
    let json = d.path("lib.json");
    let pdf_dir = d.path("pdf");
    let out = run(
        LIBRARY,
        &[
            "--wordlist",
            s(&dict),
            "--size",
            "5x6",
            // 5 rows of 6 with words of at most 5 letters: every row needs a
            // black, and rotational symmetry pairs them → at least 6.
            "--blocks",
            "6",
            "--candidates",
            "8",
            "--time",
            "0.5",
            "--top",
            "2",
            "--seed",
            "4",
            "--out",
            s(&json),
            "--pdf",
            s(&pdf_dir),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("gen search"),
        "search is the default generator"
    );
    let text = std::fs::read_to_string(&json).unwrap();
    assert!(text.contains("\"clue\": \"Definizione di") || text.contains("\"clue\": \"Altra"));
    let pdf = std::fs::read(pdf_dir.join("cruciverba-01.pdf")).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
}

#[test]
fn library_rejects_bad_size_and_symmetry() {
    let out = run(LIBRARY, &["--size", "x9"]);
    assert_eq!(out.status.code(), Some(2));
    let out = run(LIBRARY, &["--symmetry", "diagonale"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn library_fails_cleanly_on_missing_wordlist() {
    let out = run(LIBRARY, &["--wordlist", "/nonexistent/dict.csv"]);
    assert!(!out.status.success());
    assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
}

const BENCH: &str = env!("CARGO_BIN_EXE_bench");

#[test]
fn bench_runs_smoke_suite_and_prints_summary() {
    let d = TempDir::new("bench");
    let dict = write_dict(&d, 5);
    for mode in ["first", "quality"] {
        let out = run(
            BENCH,
            &[
                "--wordlist",
                s(&dict),
                "--suite",
                "smoke",
                "--time",
                "0.3",
                "--mode",
                mode,
            ],
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(stdout.contains("TOTALE"), "{stdout}");
        assert!(stdout.contains("2/2"), "{stdout}");
    }
}

#[test]
fn bench_accepts_solver_variants() {
    let d = TempDir::new("bench-variants");
    let dict = write_dict(&d, 5);
    let out = run(
        BENCH,
        &[
            "--wordlist",
            s(&dict),
            "--suite",
            "smoke",
            "--time",
            "0.3",
            "--propagation",
            "arc",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("propagation arc"));
    let out = run(BENCH, &["--wordlist", s(&dict), "--propagation", "boh"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn bench_rejects_unknown_suite_and_mode() {
    let d = TempDir::new("bench-bad");
    let dict = write_dict(&d, 3);
    let out = run(BENCH, &["--wordlist", s(&dict), "--suite", "boh"]);
    assert_eq!(out.status.code(), Some(2));
    let out = run(BENCH, &["--wordlist", s(&dict), "--mode", "boh"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn xfill_analyze_reports_forced_letters_and_impossible_slots() {
    let d = TempDir::new("analyze");
    let dict = d.path("d.csv");
    std::fs::write(&dict, "XAB,a\nZAB,b\nXAE,c\n").unwrap();
    let tpl = d.path("t.txt");
    std::fs::write(&tpl, ".AB\n.##\nE##\n").unwrap();
    let out = run(XFILL, &[s(&tpl), "--wordlist", s(&dict), "--analyze"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("feasible"), "{stdout}");
    assert!(
        stdout.contains("r0c0=X") && stdout.contains("r1c0=A"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("reason:"),
        "analysis must not solve: {stdout}"
    );

    std::fs::write(&tpl, "QQB\n.##\nE##\n").unwrap();
    let out = run(XFILL, &[s(&tpl), "--wordlist", s(&dict), "--analyze"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success());
    assert!(
        stdout.contains("NOT feasible") && stdout.contains("A3 r0c0"),
        "{stdout}"
    );
}

#[test]
fn library_search_generator_and_two_letter_limit() {
    let d = TempDir::new("library-search");
    let dict = write_dict(&d, 5);
    let json = d.path("lib.json");
    let out = run(
        LIBRARY,
        &[
            "--wordlist",
            s(&dict),
            "--size",
            "5x6",
            "--blocks",
            "6",
            "--candidates",
            "6",
            "--time",
            "0.5",
            "--top",
            "2",
            "--gen",
            "search",
            "--max-two-letter",
            "40",
            "--out",
            s(&json),
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("gen search"), "{stderr}");
    assert!(
        stderr.contains("infeasible"),
        "reports the screening: {stderr}"
    );
    let out = run(LIBRARY, &["--gen", "boh"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn xfill_and_library_accept_solver_options() {
    let d = TempDir::new("solver-opts");
    let dict = write_dict(&d, 4);
    let tpl = d.path("t.txt");
    std::fs::write(&tpl, "....\n.#..\n....\n").unwrap();
    let out = run(
        XFILL,
        &[
            s(&tpl),
            "--wordlist",
            s(&dict),
            "--time",
            "1",
            "--propagation",
            "forward",
            "--backjump",
            "off",
            "--lookahead",
            "4",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("propagation forward, backjump off"),
        "{stderr}"
    );
    assert!(stderr.contains("lookahead 4"), "{stderr}");
    let out = run(
        XFILL,
        &[s(&tpl), "--wordlist", s(&dict), "--propagation", "boh"],
    );
    assert!(!out.status.success());

    let out = run(
        LIBRARY,
        &[
            "--wordlist",
            s(&dict),
            "--size",
            "4x4",
            "--blocks",
            "2",
            "--candidates",
            "3",
            "--time",
            "0.3",
            "--top",
            "1",
            "--out",
            s(&d.path("l.json")),
            "--repair-max-weak",
            "0",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("repair-max-weak 0"), "{stderr}");
    let out = run(LIBRARY, &["--wordlist", s(&dict), "--backjump", "boh"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn xfill_other_options_and_errors() {
    let d = TempDir::new("xfill-opts");
    let dict = write_dict(&d, 4);
    let tpl = d.path("t.txt");
    std::fs::write(&tpl, "....\n.#..\n....\n").unwrap();
    let ok = run(
        XFILL,
        &[
            s(&tpl),
            "--wordlist",
            s(&dict),
            "--min-score",
            "0",
            "--seed",
            "7",
            "--tiers",
            "50,58",
            "--first",
            "--lock",
            "0,0,D,ABC",
        ],
    );
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    for bad in [
        vec!["--lock", "0,0,A"],
        vec!["--lock", "0,0,X,ABCD"],
        vec!["--seed", "x"],
        vec!["--tiers", "a,b"],
        vec!["--min-score", "x"],
        vec!["extra-argument"],
    ] {
        let mut args = vec![s(&tpl), "--wordlist", s(&dict)];
        args.extend(bad.iter().copied());
        let out = run(XFILL, &args);
        assert!(!out.status.success(), "{bad:?} must be rejected");
    }
    let out = run(XFILL, &[s(&tpl), "--wordlist", "/nonexistent.csv"]);
    assert!(!out.status.success());
    // an unfillable grid (only one 4-letter word for two slots): exit failure
    let one = d.path("one.csv");
    std::fs::write(&one, "ABCD,x\n").unwrap();
    let t2 = d.path("t2.txt");
    std::fs::write(&t2, "....\n####\n....\n").unwrap();
    let out = run(XFILL, &[s(&t2), "--wordlist", s(&one), "--time", "0.3"]);
    assert!(!out.status.success());
    // analysis of an open grid: cells with many possible letters
    let out = run(XFILL, &[s(&tpl), "--wordlist", s(&dict), "--analyze"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains('6'),
        "6 letters possible per cell: {stdout}"
    );
}
