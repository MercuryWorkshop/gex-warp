//! Waiting on a word until it is no longer a value, and waking its waiters:
//! the host's compare-and-wait (XNU's `__ulock_wait2`, a Linux futex), for
//! the native host's parked threads (gex-native's ready.rs), which wait on
//! a sequence with no lock of their own.

use std::sync::atomic::AtomicU32;
use std::time::Duration;

#[cfg(target_os = "macos")]
mod sys {
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    // XNU's compare-and-wait (what libc++'s atomic wait uses): since 11.
    const UL_COMPARE_AND_WAIT: u32 = 1;
    const ULF_WAKE_ALL: u32 = 0x100;
    const ULF_NO_ERRNO: u32 = 0x0100_0000;
    extern "C" {
        fn __ulock_wait2(operation: u32, addr: *mut std::ffi::c_void, value: u64, timeout_ns: u64, value2: u64) -> i32;
        fn __ulock_wake(operation: u32, addr: *mut std::ffi::c_void, wake_value: u64) -> i32;
    }

    pub fn wait(w: &AtomicU32, value: u32, max: Duration) {
        // (0 would be no timeout at all.)
        let ns = (max.as_nanos().min(u64::MAX as u128) as u64).max(1);
        // SAFETY: the address is a live atomic word's, which the call only reads.
        unsafe { __ulock_wait2(UL_COMPARE_AND_WAIT | ULF_NO_ERRNO, w.as_ptr().cast(), value as u64, ns, 0) };
    }

    pub fn wake_all(w: &AtomicU32) {
        // SAFETY: as `wait`'s; a word nobody waits on is ENOENT, ignored.
        unsafe { __ulock_wake(UL_COMPARE_AND_WAIT | ULF_WAKE_ALL | ULF_NO_ERRNO, w.as_ptr().cast(), 0) };
    }
}

#[cfg(target_os = "linux")]
mod sys {
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    pub fn wait(w: &AtomicU32, value: u32, max: Duration) {
        let ts = libc::timespec { tv_sec: max.as_secs().min(i64::MAX as u64) as libc::time_t, tv_nsec: max.subsec_nanos() as libc::c_long };
        // SAFETY: a private futex wait on a live atomic word, with a relative timeout.
        unsafe { libc::syscall(libc::SYS_futex, w.as_ptr(), libc::FUTEX_WAIT | libc::FUTEX_PRIVATE_FLAG, value, &ts as *const libc::timespec) };
    }

    pub fn wake_all(w: &AtomicU32) {
        // SAFETY: as `wait`'s.
        unsafe { libc::syscall(libc::SYS_futex, w.as_ptr(), libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG, i32::MAX) };
    }
}

/// Sleep while `w` is `value`, for `max` at most (or until woken; a wake
/// may also come for no reason).
pub fn wait(w: &AtomicU32, value: u32, max: Duration) {
    sys::wait(w, value, max)
}

/// Wake everyone waiting on `w`.
pub fn wake_all(w: &AtomicU32) {
    sys::wake_all(w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering::SeqCst;
    use std::sync::Arc;

    #[test]
    fn a_wake_ends_the_wait() {
        let w = Arc::new(AtomicU32::new(0));
        let w2 = Arc::clone(&w);
        let t = std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            while w2.load(SeqCst) == 0 {
                wait(&w2, 0, Duration::from_secs(5));
            }
            t0.elapsed()
        });
        std::thread::sleep(Duration::from_millis(20));
        w.store(1, SeqCst);
        wake_all(&w);
        assert!(t.join().unwrap() < Duration::from_secs(2));
    }

    #[test]
    fn a_changed_word_is_not_waited_on() {
        let w = AtomicU32::new(1);
        let t0 = std::time::Instant::now();
        wait(&w, 0, Duration::from_secs(5));
        assert!(t0.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn the_wait_ends_at_its_time() {
        let w = AtomicU32::new(0);
        let t0 = std::time::Instant::now();
        wait(&w, 0, Duration::from_millis(30));
        assert!(t0.elapsed() < Duration::from_secs(2));
    }
}
