//! Dictionary load benchmark: loads the CSV N times, prints the best time,
//! the live heap of the dictionary and the load peak.
//! usage: load <Db.csv> [times=20]
use std::time::Instant;
use xfill_core::api::Dictionary;

#[global_allocator]
static A: xfill_perf::Counting = xfill_perf::Counting;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let times: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let bytes = std::fs::read(&a[1]).expect("csv");
    let (mut best, mut live, mut peak) = (f64::MAX, 0, 0);
    for _ in 0..times {
        let base = xfill_perf::live();
        xfill_perf::reset_peak();
        let t = Instant::now();
        let d = Dictionary::from_csv_bytes(&bytes).expect("dictionary");
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
        live = xfill_perf::live() - base;
        peak = xfill_perf::peak() - base;
        drop(d);
    }
    println!(
        "LOAD best {best:.1} ms | dictionary {:.2} MiB | load peak {:.2} MiB (without the CSV bytes)",
        xfill_perf::mb(live),
        xfill_perf::mb(peak)
    );
}
