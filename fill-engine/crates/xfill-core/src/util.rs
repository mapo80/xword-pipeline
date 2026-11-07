//! Small shared utilities.

/// splitmix64 PRNG — deterministic, dependency-free.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    #[inline]
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// Lower the calling thread's scheduling priority so a long background
/// search leaves the machine responsive and uses less power (macOS: QoS
/// class "utility"; Linux: nice 10). Returns false where unsupported.
pub fn lower_thread_priority() -> bool {
    os::lower()
}

/// Is the calling thread running at the lowered priority?
pub fn current_priority_is_low() -> bool {
    os::is_low()
}

#[cfg(target_os = "macos")]
mod os {
    const QOS_CLASS_UTILITY: u32 = 0x11;
    extern "C" {
        fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        fn qos_class_self() -> u32;
    }
    pub fn lower() -> bool {
        // SAFETY: plain libSystem call on the current thread, no pointers.
        unsafe { pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0) == 0 }
    }
    pub fn is_low() -> bool {
        // SAFETY: as above; utility (0x11) and background (0x09) are low.
        unsafe { qos_class_self() <= QOS_CLASS_UTILITY }
    }
}

#[cfg(target_os = "linux")]
mod os {
    const PRIO_PROCESS: i32 = 0;
    extern "C" {
        fn setpriority(which: i32, who: u32, prio: i32) -> i32;
        fn getpriority(which: i32, who: u32) -> i32;
        fn gettid() -> i32;
    }
    pub fn lower() -> bool {
        // SAFETY: libc calls on the current thread id, no pointers.
        unsafe { setpriority(PRIO_PROCESS, gettid() as u32, 10) == 0 }
    }
    pub fn is_low() -> bool {
        // SAFETY: as above.
        unsafe { getpriority(PRIO_PROCESS, gettid() as u32) > 0 }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod os {
    pub fn lower() -> bool {
        false
    }
    pub fn is_low() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowering_priority_takes_effect() {
        let h = std::thread::spawn(|| {
            let ok = lower_thread_priority();
            (ok, current_priority_is_low())
        });
        let (ok, low) = h.join().unwrap();
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert!(ok, "supported platform");
            assert!(low, "the thread now runs at low priority");
        } else {
            assert!(!ok);
        }
    }
}
