//! Differential check: every generated grid must pass `validate`.
//! Usage: cargo run --release --example validate_sweep -- DICT.csv THEME.txt SEEDS
use xfill_core::api::*;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dict = Dictionary::from_csv_bytes(&std::fs::read(&a[0]).unwrap()).unwrap();
    let theme: Vec<String> = std::fs::read_to_string(&a[1])
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    let seeds: u64 = a[2].parse().unwrap();
    let (mut ok, mut bad) = (0, 0);
    for (rows, cols) in [(13, 13), (15, 15), (12, 18), (12, 22)] {
        for themed in [false, true] {
            for seed in 1..=seeds {
                let req = GenerateRequest {
                    rows,
                    cols,
                    seed,
                    candidates: 16,
                    node_budget: 5_000,
                    theme_words: if themed { theme.clone() } else { Vec::new() },
                    ..Default::default()
                };
                let r = generate(&dict, &req, &cancel_flag());
                if r.status != Status::Ok {
                    println!(
                        "{rows}x{cols} theme={themed} seed={seed}: {:?} {:?}",
                        r.status, r.error
                    );
                    continue;
                }
                let json = serde_json::to_string(&r).unwrap();
                let v = validate(&serde_json::from_str(&json).unwrap());
                if v.valid {
                    ok += 1;
                } else {
                    bad += 1;
                    let codes: Vec<String> = v
                        .errors
                        .iter()
                        .map(|e| format!("{}: {}", e.code, e.message))
                        .collect();
                    println!("{rows}x{cols} theme={themed} seed={seed}: INVALID {codes:?}");
                }
            }
        }
    }
    println!("valid {ok}, invalid {bad}");
}
