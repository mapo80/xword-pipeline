//! Dictionary file benchmark: build from CSV, save, open (mapped), and the
//! same generations on the built and the opened dictionary.
//! usage: xfd <dictionary.csv> [out.xfd]
use std::time::Instant;
use xfill_core::api::{cancel_flag, generate, Dictionary, GenerateRequest};

#[global_allocator]
static A: xfill_perf::Counting = xfill_perf::Counting;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let out = a.get(2).cloned().unwrap_or_else(|| {
        std::env::temp_dir()
            .join("xfd-bench.xfd")
            .display()
            .to_string()
    });
    let out = std::path::Path::new(&out);
    let bytes = std::fs::read(&a[1]).expect("csv");
    let base = xfill_perf::live();
    xfill_perf::reset_peak();
    let t = Instant::now();
    let built = Dictionary::from_csv_bytes(&bytes).expect("dictionary");
    let build_ms = ms(t);
    let build_peak = xfill_perf::mb(xfill_perf::peak() - base);
    drop(bytes);
    let t = Instant::now();
    built.save(out).expect("save");
    let save_ms = ms(t);
    let size = std::fs::metadata(out).unwrap().len();
    // open: best of 20 (warm page cache, as after the first start)
    let mut open_ms = f64::MAX;
    for _ in 0..20 {
        let t = Instant::now();
        let d = Dictionary::open(out).expect("open");
        open_ms = open_ms.min(ms(t));
        drop(d);
    }
    let before = xfill_perf::live();
    let opened = Dictionary::open(out).expect("open");
    let opened_heap = xfill_perf::live() - before;
    println!(
        "words {} | CSV → dictionary {build_ms:.1} ms (heap peak {build_peak:.2} MiB) | save {save_ms:.1} ms | file {:.2} MiB | open {open_ms:.2} ms, heap {:.1} KiB, mapped {:.2} MiB",
        built.word_count(),
        size as f64 / 1048576.0,
        opened_heap as f64 / 1024.0,
        opened.mapped_bytes() as f64 / 1048576.0
    );
    for (r, c) in [(13, 13), (15, 15), (12, 22)] {
        let (mut tb, mut to, mut same) = (0.0, 0.0, true);
        for seed in 1..=3 {
            let req = GenerateRequest {
                rows: r,
                cols: c,
                seed,
                threads: 1,
                ..Default::default()
            };
            let t = Instant::now();
            let x = generate(&built, &req, &cancel_flag());
            tb += ms(t);
            let t = Instant::now();
            let y = generate(&opened, &req, &cancel_flag());
            to += ms(t);
            same &= x.grid == y.grid
                && serde_json::to_string(&x.entries).unwrap()
                    == serde_json::to_string(&y.entries).unwrap();
        }
        println!(
            "{r}x{c} (3 seeds, 1 thread): built {:.0} ms | opened {:.0} ms | identical {same}",
            tb / 3.0,
            to / 3.0
        );
    }
}
