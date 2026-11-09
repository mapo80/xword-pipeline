//! Measuring helpers shared by the harnesses: a counting allocator (live,
//! peak and allocation count), process CPU time and peak RSS.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

/// System allocator that counts live bytes, the peak and the allocations.
pub struct Counting;

static CUR: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if !p.is_null() {
            let c = CUR.fetch_add(l.size(), Relaxed) + l.size();
            PEAK.fetch_max(c, Relaxed);
            ALLOCS.fetch_add(1, Relaxed);
            ALLOC_BYTES.fetch_add(l.size(), Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l);
        CUR.fetch_sub(l.size(), Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = System.realloc(p, l, n);
        if !q.is_null() {
            if n > l.size() {
                let c = CUR.fetch_add(n - l.size(), Relaxed) + n - l.size();
                PEAK.fetch_max(c, Relaxed);
            } else {
                CUR.fetch_sub(l.size() - n, Relaxed);
            }
            ALLOCS.fetch_add(1, Relaxed);
            ALLOC_BYTES.fetch_add(n, Relaxed);
        }
        q
    }
}

/// Live heap bytes.
pub fn live() -> usize {
    CUR.load(Relaxed)
}
/// Heap peak since the last [`reset_peak`].
pub fn peak() -> usize {
    PEAK.load(Relaxed)
}
/// Restart the peak from the live bytes.
pub fn reset_peak() {
    PEAK.store(CUR.load(Relaxed), Relaxed);
}
/// Allocations so far.
pub fn allocs() -> usize {
    ALLOCS.load(Relaxed)
}
/// Bytes allocated so far (churn).
pub fn alloc_bytes() -> usize {
    ALLOC_BYTES.load(Relaxed)
}

#[cfg(unix)]
mod rusage {
    #[repr(C)]
    struct Rusage {
        utime: [i64; 2],
        stime: [i64; 2],
        maxrss: i64,
        rest: [i64; 13],
    }
    extern "C" {
        fn getrusage(who: i32, r: *mut Rusage) -> i32;
    }
    fn get() -> Rusage {
        // SAFETY: plain C struct filled by the OS (64-bit Unix rusage layout).
        let mut r: Rusage = unsafe { std::mem::zeroed() };
        unsafe { getrusage(0, &mut r) };
        r
    }
    pub fn cpu_s() -> f64 {
        let r = get();
        // timeval: seconds, then microseconds (32 bits on macOS)
        let tv = |t: [i64; 2]| t[0] as f64 + (t[1] as i32) as f64 / 1e6;
        tv(r.utime) + tv(r.stime)
    }
    pub fn max_rss_mb() -> f64 {
        let b = get().maxrss as f64;
        if cfg!(target_os = "macos") {
            b / 1048576.0 // bytes
        } else {
            b / 1024.0 // kilobytes
        }
    }
}

/// Process CPU time (user + system), seconds (0 where not measured).
pub fn cpu_s() -> f64 {
    #[cfg(unix)]
    return rusage::cpu_s();
    #[cfg(not(unix))]
    return 0.0;
}

/// Peak resident set size, MiB (0 where not measured).
pub fn max_rss_mb() -> f64 {
    #[cfg(unix)]
    return rusage::max_rss_mb();
    #[cfg(not(unix))]
    return 0.0;
}

/// Bytes → MB.
pub fn mb(b: usize) -> f64 {
    b as f64 / 1048576.0
}
