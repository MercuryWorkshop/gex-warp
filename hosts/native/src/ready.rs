//! Waking threads parked on descriptors (`perform_block`), natively.
//!
//! A thread blocked in `poll`, `read`, `accept`, `connect` or a full `write`
//! re-checks its descriptors on every turn. It used to sleep 1 ms between
//! turns, so every handoff between processes (a wineserver request and its
//! reply, a pipe between two programs) cost about a millisecond. Now the
//! turn ends early: any syscall that can change descriptor readiness
//! ([`gex_kernel::syscalls::wakes_fd_waiters`]) bumps a host-wide sequence
//! number and wakes the parked threads, which re-check at once. The 1 ms
//! turn stays as the bound for what no syscall announces (host devices,
//! timers, the compositor's own sockets).
//!
//! One sequence for the whole host rather than a queue per descriptor: a
//! wake costs every parked thread one re-check, but only syscalls that touch
//! descriptors wake anyone, and only while someone is parked.
//!
//! The parked threads wait on the sequence's word itself (the host's
//! compare-and-wait, `gex_atomic::park`: `__ulock_wait2` on macOS, a futex
//! on Linux), with no lock of ours: a desktop's hundreds of parked threads,
//! every one woken by each notification, used to queue on one mutex to
//! leave their condition wait, and every notifier with them (the most
//! contended lock of a Mojave desktop run once the process table's were
//! gone). Elsewhere, a mutex and condition variable as before.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering::SeqCst};
use std::time::Duration;

/// The sequence (its low 32 bits: a wait compares a word of 32).
static SEQ: AtomicU32 = AtomicU32::new(0);
static PARKED: AtomicUsize = AtomicUsize::new(0);

/// The current sequence number: take it before checking the descriptors,
/// then pass it to [`wait`].
pub fn seq() -> u64 {
    SEQ.load(SeqCst) as u64
}

/// Something may have become ready: wake every parked thread.
pub fn notify() {
    SEQ.fetch_add(1, SeqCst);
    if PARKED.load(SeqCst) > 0 {
        word::wake_all(&SEQ);
    }
}

/// Park until [`notify`] has run since `seen` was taken, or `max` passes.
/// No notification is lost between the caller's check of its descriptors
/// and the sleep: the parked count is raised before the sequence is looked
/// at again (a notifier that moves it after that sees the count and wakes),
/// and the host's wait compares the word with `seen` as it sleeps.
pub fn wait(seen: u64, max: Duration) {
    PARKED.fetch_add(1, SeqCst);
    if SEQ.load(SeqCst) == seen as u32 {
        word::wait(&SEQ, seen as u32, max);
    }
    PARKED.fetch_sub(1, SeqCst);
}

/// Waiting on a word until it is no longer a value, and waking its waiters.
#[cfg(any(target_os = "macos", target_os = "linux"))]
use gex_atomic::park as word;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod word {
    use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    static LOCK: Mutex<()> = Mutex::new(());
    static CV: Condvar = Condvar::new();

    pub fn wait(w: &AtomicU32, value: u32, max: Duration) {
        let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if w.load(SeqCst) == value {
            let _ = CV.wait_timeout(g, max);
        }
    }

    pub fn wake_all(_: &AtomicU32) {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        CV.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notify_ends_the_wait_early() {
        let s = seq();
        let t = std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            wait(s, Duration::from_secs(5));
            t0.elapsed()
        });
        std::thread::sleep(Duration::from_millis(20));
        notify();
        assert!(t.join().unwrap() < Duration::from_secs(1));
    }

    #[test]
    fn a_notify_before_the_wait_is_not_lost() {
        let s = seq();
        notify();
        let t0 = std::time::Instant::now();
        wait(s, Duration::from_secs(5));
        assert!(t0.elapsed() < Duration::from_secs(1));
    }
}
