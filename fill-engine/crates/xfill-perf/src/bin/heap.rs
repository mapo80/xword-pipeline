//! Heap profile (dhat) of one dictionary load + one generation, single
//! thread. Writes `dhat-heap.json` (open it in
//! https://nnethercote.github.io/dh_view/dh_view.html) and prints the totals.
//! usage: [HEAP_PHASE=gen] heap <Db.csv> [RxC=13x13] [seed=1] [threads=1]
use xfill_core::api::{cancel_flag, generate, Dictionary, GenerateRequest};

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = a.get(2).cloned().unwrap_or("13x13".into());
    let seed: u64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
    let threads: usize = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(1);
    let (r, c) = f.split_once('x').unwrap();
    let bytes = std::fs::read(&a[1]).unwrap();
    // HEAP_PHASE=gen: profile the generation only (dictionary loaded before)
    let gen_only = std::env::var("HEAP_PHASE").as_deref() == Ok("gen");
    let pre = if gen_only {
        Some(Dictionary::from_csv_bytes(&bytes).unwrap())
    } else {
        None
    };
    let _prof = dhat::Profiler::new_heap();
    let dict = match pre {
        Some(d) => d,
        None => Dictionary::from_csv_bytes(&bytes).unwrap(),
    };
    let s = dhat::HeapStats::get();
    eprintln!(
        "after load: live {} B in {} blocks, peak {} B",
        s.curr_bytes, s.curr_blocks, s.max_bytes
    );
    let req = GenerateRequest {
        rows: r.parse().unwrap(),
        cols: c.parse().unwrap(),
        seed,
        threads,
        ..Default::default()
    };
    let res = generate(&dict, &req, &cancel_flag());
    let s = dhat::HeapStats::get();
    eprintln!(
        "after generate: {:?} {} ms | live {} B, peak {} B, total {} B in {} allocations",
        res.status, res.duration_ms, s.curr_bytes, s.max_bytes, s.total_bytes, s.total_blocks
    );
}
