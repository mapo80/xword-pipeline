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
    let auto = xfill_core::solver::auto_threads(cpus, usize::MAX);
    assert!(
        stderr.contains(&format!("solving with {auto} worker")),
        "automatic thread count by default: {stderr}"
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
fn bench_threads_per_case() {
    let d = TempDir::new("bench-threads");
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
            "--threads",
            "3",
            "--parallel",
            "split",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("3 threads per pattern"), "{stderr}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("2/2"));
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

#[test]
fn xfill_exhaustive_reports_proof() {
    let d = TempDir::new("exhaustive");
    let dict = d.path("d.csv");
    std::fs::write(&dict, "AB,a\nAC,b\nBA,c\nBD,d\nDA,e\n").unwrap();
    let tpl = d.path("t.txt");
    std::fs::write(&tpl, "..\n..\n").unwrap();
    let out = run(
        XFILL,
        &[
            s(&tpl),
            "--wordlist",
            s(&dict),
            "--exhaustive",
            "--workers",
            "1",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success());
    assert!(stdout.contains("proven unfillable"), "{stdout}");
}

#[test]
fn theme_list_with_theme_optimization() {
    let d = TempDir::new("theme");
    let dict = d.path("d.csv");
    std::fs::write(&dict, "AAA,a\nAAA,b\nAAA,c\nBBB,d\nBBB,e\nCCC,f\n").unwrap();
    let theme = d.path("theme.txt");
    std::fs::write(&theme, "# tema\nccc\nzzz\n").unwrap();
    let tpl = d.path("t.txt");
    std::fs::write(&tpl, "...\n").unwrap();
    let out = run(
        XFILL,
        &[
            s(&tpl),
            "--wordlist",
            s(&dict),
            "--optimize",
            "theme",
            "--theme-list",
            s(&theme),
            "--first",
            "--workers",
            "1",
        ],
    );
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(out.status.success(), "{stderr}");
    assert!(
        stdout.lines().any(|l| l == "CCC"),
        "theme word placed: {stdout}"
    );
    assert!(
        stderr.contains("theme: 1 of 2 words in the dictionary"),
        "{stderr}"
    );
    assert!(stdout.contains("theme words used: 1"), "{stdout}");
    let out = run(
        XFILL,
        &[s(&tpl), "--wordlist", s(&dict), "--theme-list", "/nope.txt"],
    );
    assert!(!out.status.success());

    let out = run(
        LIBRARY,
        &[
            "--wordlist",
            s(&dict),
            "--size",
            "3x3",
            "--blocks",
            "6",
            "--candidates",
            "2",
            "--time",
            "0.2",
            "--top",
            "1",
            "--out",
            s(&d.path("l.json")),
            "--theme-list",
            s(&theme),
            "--optimize",
            "theme",
        ],
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("theme: 1 of 2"));
}

#[test]
fn xfill_best_lists_ranked_fills() {
    let d = TempDir::new("best");
    let dict = write_dict(&d, 3);
    let tpl = d.path("t.txt");
    std::fs::write(&tpl, "...\n.#.\n...\n").unwrap();
    let out = run(
        XFILL,
        &[
            s(&tpl),
            "--wordlist",
            s(&dict),
            "--best",
            "3",
            "--time",
            "3",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for k in 1..=3 {
        assert!(stdout.contains(&format!("#{k} mean ")), "{stdout}");
    }
    assert!(stdout.contains("(optimal)") || stdout.contains("(best found in time)"));
    let means: Vec<f64> = stdout
        .lines()
        .filter_map(|l| l.split("mean ").nth(1))
        .filter_map(|m| m.split_whitespace().next()?.parse().ok())
        .collect();
    assert!(means.windows(2).all(|w| w[0] >= w[1]), "ranked: {means:?}");
    let out = run(XFILL, &[s(&tpl), "--wordlist", s(&dict), "--best", "x"]);
    assert!(!out.status.success());
}

#[test]
fn xfill_rebus_cells_and_free_rebus() {
    let d = TempDir::new("rebus");
    let dict = d.path("d.csv");
    std::fs::write(&dict, "CREDO,Atto di fede\nREX,Re latino\n").unwrap();
    let tpl = d.path("t.txt");
    std::fs::write(&tpl, "C[RE]DO\n#.##\n").unwrap();
    let out = run(
        XFILL,
        &[s(&tpl), "--wordlist", s(&dict), "--first", "--workers", "1"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("C[RE]DO"), "{stdout}");
    assert!(stdout.contains("#X##"), "{stdout}");
    let t2 = d.path("t2.txt");
    std::fs::write(&t2, "....\n").unwrap();
    let out = run(
        XFILL,
        &[s(&t2), "--wordlist", s(&dict), "--first", "--workers", "1"],
    );
    assert!(!out.status.success(), "no plain 4-letter word");
    let out = run(
        XFILL,
        &[s(&t2), "--wordlist", s(&dict), "--first", "--rebus", "re"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("C[RE]DO"), "{stdout}");
}

#[test]
fn xfill_analyze_reports_sections() {
    let d = TempDir::new("sections");
    let dict = d.path("d.csv");
    std::fs::write(&dict, "AB,a\nCD,b\nAC,c\nBD,d\n").unwrap();
    let tpl = d.path("t.txt");
    std::fs::write(&tpl, "..#Z.\n..#..\n").unwrap();
    let out = run(
        XFILL,
        &[s(&tpl), "--wordlist", s(&dict), "--analyze", "--time", "1"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("sections: 2"), "{stdout}");
    assert!(
        stdout.contains(": fillable") && stdout.contains(": impossible"),
        "{stdout}"
    );
}

#[test]
fn xfill_suggest_lists_candidates_and_forced_words() {
    let d = TempDir::new("suggest");
    let dict = d.path("d.csv");
    std::fs::write(&dict, "XAB,a\nYAB,b\nYAB,b2\nXZ,c\nYZ,d\nYZ,e\nYZ,f\n").unwrap();
    let tpl = d.path("t.txt");
    std::fs::write(&tpl, ".AB\nZ##\n").unwrap();
    let out = run(
        XFILL,
        &[
            s(&tpl),
            "--wordlist",
            s(&dict),
            "--suggest",
            "0,0,A",
            "--suggest-check",
            "1",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let first = stdout.lines().find(|l| l.contains("score")).unwrap_or("");
    assert!(first.contains("YAB"), "ranked first: {stdout}");
    assert!(stdout.contains("fillable"), "{stdout}");
    let out = run(
        XFILL,
        &[s(&tpl), "--wordlist", s(&dict), "--suggest", "9,9,A"],
    );
    assert!(!out.status.success());
    let out = run(
        XFILL,
        &[s(&tpl), "--wordlist", s(&dict), "--suggest", "0,0"],
    );
    assert!(!out.status.success());
}

const PAROLE: &str = env!("CARGO_BIN_EXE_parole");

#[test]
fn parole_pattern_and_anagram() {
    let d = TempDir::new("parole");
    let dict = d.path("d.csv");
    std::fs::write(
        &dict,
        "CASA,Abitazione\nCOSA,Oggetto\nROMA,Capitale\nAMOR,Amore latino\nRAMO,Parte d'albero\n",
    )
    .unwrap();
    let out = run(
        PAROLE,
        &["pattern", "c?sa", "--wordlist", s(&dict), "--clues"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("CASA") && stdout.contains("COSA"),
        "{stdout}"
    );
    assert!(stdout.contains("Abitazione"), "clues shown: {stdout}");
    let out = run(PAROLE, &["anagram", "roma", "--wordlist", s(&dict)]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    for w in ["ROMA", "AMOR", "RAMO"] {
        assert!(stdout.contains(w), "{stdout}");
    }
    let out = run(
        PAROLE,
        &[
            "anagram",
            "casaroma",
            "--words",
            "2",
            "--wordlist",
            s(&dict),
        ],
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("CASA ROMA"));
    let out = run(PAROLE, &["pattern", "[ab", "--wordlist", s(&dict)]);
    assert!(!out.status.success());
    let out = run(PAROLE, &["boh"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn library_generator_constraint_options() {
    let d = TempDir::new("gen-opts");
    let dict = write_dict(&d, 5);
    let json = d.path("l.json");
    let out = run(
        LIBRARY,
        &[
            "--wordlist",
            s(&dict),
            "--size",
            "7x7",
            "--blocks",
            "10",
            "--candidates",
            "3",
            "--time",
            "0.3",
            "--top",
            "1",
            "--out",
            s(&json),
            "--max-len",
            "5",
            "--min-len-count",
            "5:2",
            "--max-full-words",
            "0",
            "--require-lengths",
            "4,3",
            "--max-white-rect",
            "20",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("constraints:"), "{stderr}");
    let text = std::fs::read_to_string(&json).unwrap();
    let lens: Vec<usize> = text
        .split("\"len\": ")
        .skip(1)
        .filter_map(|x| x.split(',').next()?.parse().ok())
        .collect();
    assert!(lens.iter().filter(|&&l| l == 5).count() >= 2, "{lens:?}");
    let out = run(LIBRARY, &["--wordlist", s(&dict), "--min-len-count", "x"]);
    assert_eq!(out.status.code(), Some(2));
    let out = run(
        LIBRARY,
        &[
            "--wordlist",
            s(&dict),
            "--size",
            "7x7",
            "--blocks",
            "14",
            "--lattice",
            "--candidates",
            "2",
            "--time",
            "0.3",
            "--top",
            "1",
            "--out",
            s(&json),
        ],
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("lattice"));
}

#[test]
fn library_llm_clue_options() {
    let d = TempDir::new("llm");
    let dict = write_dict(&d, 4);
    let out = run(
        LIBRARY,
        &[
            "--wordlist",
            s(&dict),
            "--llm-clues",
            "missing",
            "--llm-config",
            "/nonexistent/llm.txt",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("cannot read LLM config"), "{stderr}");
    let out = run(
        LIBRARY,
        &["--wordlist", s(&dict), "--llm-clues", "talvolta"],
    );
    assert_eq!(out.status.code(), Some(2));
}
