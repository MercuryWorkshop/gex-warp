//! Restartable ranges: stretches of a task's code that a thread may be
//! taken out of and sent back to their start.
//!
//! The Objective-C runtime reads a class's method cache without a lock, in
//! `objc_msgSend`. When it replaces a cache it must know that no thread is
//! still reading the old one before it frees it, and it has the kernel
//! see to that: it registers the ranges of code that read a cache
//! (`task_restartable_ranges_register`: where each is, how long, and where
//! a thread found in it is to go on instead), and asks, before it frees
//! anything, that every thread in one be sent there
//! (`task_restartable_ranges_synchronize`).
//!
//! XNU sets each thread's program counter as it next returns to user mode.
//! Here a thread's program counter is exact only where translated code
//! says it (a system call, and the poll a loop makes every so many times
//! round, [`Ranges::point`]), so a thread answers a request there: at a
//! poll it is sent to the range's recovery address if it is in one, and a
//! system call is in no range. The one that asked waits until every other
//! thread has answered since, or is in a system call
//! ([`Ranges::unanswered`]), and is made to ask again until then.
use crate::signal::{ThreadSig, ThreadSigs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The longest a range or the way to its recovery address may be
/// (`TASK_RESTARTABLE_OFFSET_MAX`).
const OFFSET_MAX: u64 = 4096;

/// One range: where it starts and ends, and where a thread in it goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: u64,
    pub end: u64,
    pub recovery: u64,
}

/// A task's restartable ranges, and how many times it has asked that its
/// threads be restarted: one for the process's kernel and its host's
/// threads, which ask it without the kernel lock.
#[derive(Default)]
pub struct Ranges {
    /// Whether there are any: all a thread reads of this at a poll when
    /// there are none.
    on: AtomicBool,
    /// Whether any were ever registered, which a task does once.
    registered: AtomicBool,
    asked: AtomicU64,
    list: Mutex<Vec<Range>>,
}

impl Ranges {
    /// The ranges as `task_restartable_ranges_register` is given them,
    /// (location, length, recovery offset, flags) each: `Err` with XNU's
    /// answer for ones it would refuse, or for a second registration.
    pub fn register(&self, given: &[(u64, u16, u16, u32)]) -> Result<(), i32> {
        const KERN_INVALID_ARGUMENT: i32 = 4;
        const KERN_NOT_SUPPORTED: i32 = 46;
        let mut list = Vec::with_capacity(given.len());
        for &(start, len, recovery, flags) in given {
            // (code as gex knows it: a Darwin process's pc less its window)
            let start = gex_abi::canon(start);
            if len as u64 > OFFSET_MAX || recovery as u64 > OFFSET_MAX || flags != 0 {
                return Err(KERN_INVALID_ARGUMENT);
            }
            let (Some(end), Some(recovery)) = (start.checked_add(len as u64), start.checked_add(recovery as u64)) else { return Err(KERN_INVALID_ARGUMENT) };
            list.push(Range { start, end, recovery });
        }
        list.sort_by_key(|r| r.start);
        if list.windows(2).any(|w| w[0].end > w[1].start) {
            return Err(KERN_INVALID_ARGUMENT);
        }
        if self.registered.swap(true, Ordering::AcqRel) {
            return Err(KERN_NOT_SUPPORTED);
        }
        self.on.store(!list.is_empty(), Ordering::Release);
        *self.list.lock().unwrap_or_else(|e| e.into_inner()) = list;
        Ok(())
    }

    /// Whether the task has ranges a thread could be in.
    pub fn on(&self) -> bool {
        self.on.load(Ordering::Acquire)
    }

    /// A forked child's: the same ranges of the same code, and nothing
    /// asked yet.
    pub fn copy(&self) -> Ranges {
        let list = self.list.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Ranges { on: AtomicBool::new(!list.is_empty()), registered: AtomicBool::new(self.registered.load(Ordering::Acquire)), asked: AtomicU64::new(0), list: Mutex::new(list) }
    }

    /// The ranges as text, for a forked child in another memory to have
    /// ([`Ranges::adopt`]): `start:end:recovery` in hex, comma-separated;
    /// `-` for a task that registered none; empty for one that never
    /// registered.
    pub fn carry(&self) -> String {
        if !self.registered.load(Ordering::Acquire) {
            return String::new();
        }
        let list = self.list.lock().unwrap_or_else(|e| e.into_inner());
        if list.is_empty() {
            return "-".to_string();
        }
        list.iter().map(|r| format!("{:x}:{:x}:{:x}", r.start, r.end, r.recovery)).collect::<Vec<_>>().join(",")
    }

    /// The other half of [`Ranges::carry`]. Text that is not its is no
    /// ranges.
    pub fn adopt(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let list: Option<Vec<Range>> = if text == "-" {
            Some(Vec::new())
        } else {
            text.split(',')
                .map(|r| {
                    let mut p = r.split(':').map(|n| u64::from_str_radix(n, 16).ok());
                    Some(Range { start: p.next()??, end: p.next()??, recovery: p.next()?? })
                })
                .collect()
        };
        let Some(list) = list else { return };
        self.registered.store(true, Ordering::Release);
        self.on.store(!list.is_empty(), Ordering::Release);
        *self.list.lock().unwrap_or_else(|e| e.into_inner()) = list;
    }

    /// A new program in the task's place has none.
    pub fn clear(&self) {
        self.on.store(false, Ordering::Release);
        self.registered.store(false, Ordering::Release);
        self.list.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// Thread `t` is at a point in its own code where its program counter
    /// is exactly `pc`, its registers what the program left them: where
    /// it is to go on instead, if the task has asked that its threads be
    /// restarted since the thread last answered and it is in a range.
    pub fn point(&self, t: &ThreadSig, pc: u64) -> Option<u64> {
        if !self.on() {
            return None;
        }
        t.restart_left_call();
        if !t.restart_answer(self.asked.load(Ordering::Acquire)) {
            return None;
        }
        let list = self.list.lock().unwrap_or_else(|e| e.into_inner());
        list.iter().find(|r| r.start <= pc && pc < r.end).map(|r| r.recovery)
    }

    /// Thread `t` makes a system call: it is in no range, which answers
    /// whatever was asked, and stays so until the call ends
    /// ([`Ranges::leave_call`]).
    pub fn enter_call(&self, t: &ThreadSig) {
        if self.on() {
            t.restart_in_call();
            t.restart_answer(self.asked.load(Ordering::Acquire));
        }
    }

    /// Thread `t`'s system call has ended, and it goes back to its code.
    pub fn leave_call(&self, t: &ThreadSig) {
        if self.on() {
            t.restart_left_call();
        }
    }

    /// One more request that the task's threads be restarted: its number.
    pub fn ask(&self) -> u64 {
        self.asked.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Whether a thread of `threads` other than `me` may still be in a
    /// range as it was before request `asked` was made: one there still
    /// is (`live`, by its id) that has not answered it and is not in a
    /// system call.
    pub fn unanswered(&self, threads: &ThreadSigs, me: &Arc<ThreadSig>, asked: u64, live: impl Fn(i32) -> bool) -> bool {
        threads.pairs().iter().any(|(tid, t)| !Arc::ptr_eq(t, me) && !t.restart_answered(asked) && live(*tid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread() -> Arc<ThreadSig> {
        Arc::new(ThreadSig::new_thread(0))
    }

    #[test]
    fn ranges_are_registered_once_and_checked() {
        let r = Ranges::default();
        assert_eq!(r.register(&[(0x1000, 8, 5000, 0)]), Err(4), "a recovery address too far");
        assert_eq!(r.register(&[(0x1000, 8, 8, 1)]), Err(4), "flags");
        assert_eq!(r.register(&[(0x1000, 16, 16, 0), (0x1008, 8, 8, 0)]), Err(4), "two that overlap");
        assert!(!r.on());
        assert_eq!(r.register(&[(0x2000, 8, 12, 0), (0x1000, 16, 16, 0)]), Ok(()));
        assert!(r.on());
        assert_eq!(r.register(&[(0x3000, 8, 8, 0)]), Err(46), "a second time");
    }

    #[test]
    fn a_thread_in_a_range_is_sent_to_its_recovery_once_asked() {
        let r = Ranges::default();
        r.register(&[(0x1000, 16, 24, 0)]).unwrap();
        let (me, other) = (thread(), thread());
        let threads = ThreadSigs::default();
        threads.insert(1, Arc::clone(&me));
        threads.insert(2, Arc::clone(&other));
        assert_eq!(r.point(&other, 0x1004), None, "nothing asked");
        let asked = r.ask();
        assert!(r.unanswered(&threads, &me, asked, |_| true), "the other has not answered");
        assert_eq!(r.point(&other, 0x1004), Some(0x1018));
        assert!(!r.unanswered(&threads, &me, asked, |_| true));
        assert_eq!(r.point(&other, 0x1004), None, "answered already");
        let asked = r.ask();
        assert_eq!(r.point(&other, 0x1010), None, "past the range's end");
        assert!(!r.unanswered(&threads, &me, asked, |_| true), "which answers all the same");
    }

    #[test]
    fn a_thread_in_a_system_call_has_answered() {
        let r = Ranges::default();
        r.register(&[(0x1000, 16, 24, 0)]).unwrap();
        let (me, other) = (thread(), thread());
        let threads = ThreadSigs::default();
        threads.insert(1, Arc::clone(&me));
        threads.insert(2, Arc::clone(&other));
        r.enter_call(&other);
        let asked = r.ask();
        assert!(!r.unanswered(&threads, &me, asked, |_| true), "in its call");
        r.leave_call(&other);
        assert!(r.unanswered(&threads, &me, asked, |_| true), "back in its code, and not seen there since");
        r.enter_call(&other);
        r.leave_call(&other);
        assert!(!r.unanswered(&threads, &me, asked, |_| true), "a call made since is an answer");
        r.leave_call(&other);
        let asked = r.ask();
        assert!(r.unanswered(&threads, &me, asked, |_| true));
        assert!(!r.unanswered(&threads, &me, asked, |tid| tid != 2), "a thread there is no more of answers nothing");
    }

    #[test]
    fn a_forked_child_has_its_parents_ranges() {
        let r = Ranges::default();
        assert_eq!(r.carry(), "");
        r.register(&[(0x1000, 16, 24, 0), (0x2000, 8, 8, 0)]).unwrap();
        let text = r.carry();
        let child = Ranges::default();
        child.adopt(&text);
        assert!(child.on());
        assert_eq!(child.register(&[]), Err(46));
        let t = thread();
        child.ask();
        assert_eq!(child.point(&t, 0x2004), Some(0x2008));
        let copy = r.copy();
        copy.ask();
        assert_eq!(copy.point(&thread(), 0x1000), Some(0x1018));
        copy.clear();
        assert!(!copy.on());
        assert_eq!(copy.register(&[]), Ok(()));
    }
}
