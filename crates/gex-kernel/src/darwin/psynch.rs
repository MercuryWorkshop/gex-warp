//! psynch: the kernel's half of libpthread's mutexes, condition variables
//! and read-write locks.
//!
//! libpthread keeps each of them as sequence words in the program's memory
//! (L, how many have asked; U, how many have been let go; S, how many the
//! kernel has answered) and takes and drops them with no call at all while
//! nobody is in the way. When somebody is, it calls here: to wait
//! (`psynch_mutexwait`, `psynch_cvwait`, `psynch_rw_rdlock`,
//! `psynch_rw_wrlock`), and to say it has let go (`psynch_mutexdrop`,
//! `psynch_cvsignal`, `psynch_cvbroad`, `psynch_rw_unlock`). The kernel
//! keeps, for each address called about, a queue of who waits by sequence
//! number, and what was said before the one it was said to arrived (a
//! prepost).
//!
//! This is XNU's `libpthread/kern/kern_synch.c`, routine for routine, and
//! the words it hands back are that file's: libpthread's own half assumes
//! them. What is not here is what has no meaning here: the turnstiles and
//! owners (priority inheritance), and objects shared between processes
//! (every one is its process's own).
//!
//! A thread that waits sleeps as a futex waiter on a word of its own, in a
//! page the kernel keeps in the task's memory. Whoever grants it what it
//! waits for takes it off its queue, notes the answer, and wakes the word.
//! The call is then finished with the kernel in hand
//! ([`Kernel::darwin_psynch_end`]): by the wait's end where the host
//! finishes its waits through the kernel, and by the call made again where
//! it does not ([`Post::Psynch`]). So a wait that ended for any other
//! reason (its time, a signal) leaves its queue as XNU's continuations
//! leave it.

use super::nr as d;
use super::*;
use crate::syscalls::{clock_ns, nr};
use std::collections::HashMap;

// A sequence word: a count above, bits below.
const INC: u32 = 1 << 8;
const BIT_MASK: u32 = INC - 1;
const COUNT_MASK: u32 = !BIT_MASK;
const MAX_READERS: u32 = COUNT_MASK;

// The L word's bits.
/// Cannot be taken in user mode.
const KBIT: u32 = 0x01;
/// An exclusive lock is in progress.
const EBIT: u32 = 0x02;
/// Writers wait in the kernel.
const WBIT: u32 = 0x04;
/// A condition's waiter, granted, now waiting for its mutex.
const MTX_WAIT: u32 = 0x20;
/// Overlapping grants from the kernel (in what a call returns only).
const MBIT: u32 = 0x40;
/// The lock was reset, until its first unlock.
const IBIT: u32 = 0x80;
const RWL_INIT: u32 = IBIT;

// The S word's bits.
/// The kernel's sequence is not set yet.
const SBIT: u32 = 0x01;
/// (A condition's:) the kernel has cleared all it knew.
const CV_CBIT: u32 = 0x01;
/// (A condition's:) the kernel holds only preposts, no waiters.
const CV_PBIT: u32 = 0x02;
/// The W bit, saved.
const WSVBIT: u32 = 0x04;
const RWS_INIT: u32 = SBIT;

// A mutex's options, as libpthread passes them in `flags`.
const MTX_OPT_MUTEX: u32 = 0x2000;
const MTX_OPT_POLICY_FIRSTFIT: u32 = 0x080;
const MTX_OPT_POLICY_MASK: u32 = 0x1c0;

// What `psynch_cvwait` adds to its error: the kernel cleared the
// condition's state, or holds only preposts.
const ECVCLEARED: u32 = 0x100;
const ECVPREPOST: u32 = 0x200;

// kflags
const KWF_INITCLEARED: u32 = 0x1;
const KWF_ZEROEDOUT: u32 = 0x2;
const KWF_OVERLAP_GUARD: u32 = 0x8;

// How `handle_unlock` was come to.
const UNLOCK_PREPOST: u32 = 0x01;
const UNLOCK_PREPOST_READLOCK: u32 = 0x08;
const UNLOCK_PREPOST_WRLOCK: u32 = 0x20;

const RW_TYPE_READ: u32 = 0x01;
const RW_TYPE_WRITE: u32 = 0x04;
const RW_TYPE_MASK: u32 = 0xff;
const RWSHFT_TYPE_READ: u32 = 0x0100;
const RWSHFT_TYPE_WRITE: u32 = 0x0400;
const RWSHFT_TYPE_MASK: u32 = 0xff00;

/// The two queues of an address: readers, and everybody else.
const READ: usize = 0;
const WRITE: usize = 1;

/// How long a read-write lock's queue is kept after its last use
/// (`KSYN_CLEANUP_DEADLINE`): what it remembers of the last unlock is what
/// lets a late reader in.
const KEEP_NS: u64 = 10_000_000_000;

/// The most waiters a broadcast may claim (a task's most threads).
const THREAD_MAX: u32 = 8192;

// Sequence numbers wrap, so one is lower than another when it is less than
// half the range behind it.

fn is_seqlower(x: u32, y: u32) -> bool {
    let (x, y) = (x & COUNT_MASK, y & COUNT_MASK);
    if x < y {
        y - x < MAX_READERS / 2
    } else {
        x - y > MAX_READERS / 2
    }
}

fn is_seqlower_eq(x: u32, y: u32) -> bool {
    x & COUNT_MASK == y & COUNT_MASK || is_seqlower(x, y)
}

fn is_seqhigher(x: u32, y: u32) -> bool {
    let (x, y) = (x & COUNT_MASK, y & COUNT_MASK);
    if x > y {
        x - y < MAX_READERS / 2
    } else {
        y - x > MAX_READERS / 2
    }
}

fn is_seqhigher_eq(x: u32, y: u32) -> bool {
    x & COUNT_MASK == y & COUNT_MASK || is_seqhigher(x, y)
}

fn diff_genseq(x: u32, y: u32) -> u32 {
    let (x, y) = (x & COUNT_MASK, y & COUNT_MASK);
    if x == y {
        0
    } else if x > y {
        x - y
    } else {
        (MAX_READERS - y).wrapping_add(x).wrapping_add(INC)
    }
}

fn find_diff(upto: u32, lowest: u32) -> u32 {
    if upto == lowest {
        return 0;
    }
    let diff = if is_seqlower(upto, lowest) { diff_genseq(lowest, upto) } else { diff_genseq(upto, lowest) };
    diff >> 8
}

/// What an address is being used as.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Ty {
    #[default]
    None,
    Mutex,
    Cvar,
    Rwlock,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    /// A thread, waiting.
    InWait,
    /// A signal that came before its waiter.
    Prepost,
    /// A broadcast that came before some of its waiters.
    Broadcast,
}

/// An entry of a queue (`ksyn_waitq_element`).
#[derive(Clone, Copy, Debug)]
struct Kwe {
    state: State,
    lockseq: u32,
    count: u32,
    /// The thread, for one that waits.
    tid: i32,
}

#[derive(Default, Debug)]
struct Queue {
    list: Vec<Kwe>,
    firstnum: u32,
    lastnum: u32,
}

#[derive(Default, Debug)]
struct Prepost {
    count: u32,
    lseq: u32,
    sseq: u32,
}

/// What the kernel keeps of one address (`ksyn_wait_queue`).
#[derive(Default, Debug)]
struct Kwq {
    ty: Ty,
    kflags: u32,
    /// Calls in hand and waiters not yet finished (`kw_iocount`).
    users: u32,
    /// When it was last let go of unused, for one that is kept a while.
    stamp: Option<u64>,
    inqueue: u32,
    fakecount: u32,
    highseq: u32,
    lowseq: u32,
    lword: u32,
    uword: u32,
    sword: u32,
    /// The last sequence that unlocked; for a condition, the sequence the
    /// kernel has seen so far.
    lastunlockseq: u32,
    lastseqword: u32,
    nextseqword: u32,
    prepost: Prepost,
    q: [Queue; 2],
    /// Threads taken off a queue with their answers, for
    /// [`Psynch::settle`] to tell.
    woken: Vec<(i32, u32)>,
}

impl Kwq {
    fn clear_prepost(&mut self) {
        self.prepost = Prepost { count: 0, lseq: 0, sseq: RWS_INIT };
    }

    fn mark_prepost(&mut self, count: u32, lseq: u32, sseq: u32) {
        self.prepost = Prepost { count, lseq, sseq };
    }

    /// `CLEAR_REINIT_BITS`
    fn clear_reinit_bits(&mut self) {
        if self.ty == Ty::Rwlock {
            self.nextseqword = RWS_INIT;
            self.kflags &= !KWF_OVERLAP_GUARD;
        }
        self.clear_prepost();
        self.lastunlockseq = RWL_INIT;
        self.lastseqword = RWS_INIT;
        self.lword = 0;
        self.uword = 0;
        self.sword = RWS_INIT;
    }

    /// `_kwq_is_used`
    fn is_used(&self) -> bool {
        self.inqueue != 0 || self.prepost.count != 0
    }

    /// `_ksyn_check_init`: the first to notice that the lock was reset
    /// clears what was preposted to the one before.
    fn check_init(&mut self, lgenval: u32) -> bool {
        let init = lgenval & RWL_INIT != 0;
        if init && self.kflags & KWF_INITCLEARED == 0 {
            self.clear_reinit_bits();
            self.kflags |= KWF_INITCLEARED;
        }
        init
    }

    /// `UPDATE_CVKWQ`: a condition's words as its caller has them.
    fn update_cv(&mut self, mgen: u32, ugen: u32, rw_wc: u32) {
        let sinit = rw_wc & CV_CBIT != 0;
        if self.kflags & KWF_ZEROEDOUT != 0 {
            // L, U and S were cleared when L met S.
            self.lword = mgen;
            self.uword = ugen;
            self.sword = rw_wc;
            self.kflags &= !KWF_ZEROEDOUT;
        } else {
            if is_seqhigher(mgen, self.lword) {
                self.lword = mgen;
            }
            if is_seqhigher(ugen, self.uword) {
                self.uword = ugen;
            }
            if sinit && is_seqhigher(rw_wc, self.sword) {
                self.sword = rw_wc;
            }
        }
        if sinit && is_seqlower(self.lastunlockseq, rw_wc) {
            self.lastunlockseq = rw_wc & COUNT_MASK;
        }
    }

    fn update_low_high(&mut self, lockseq: u32) {
        if self.inqueue == 1 {
            self.lowseq = lockseq;
            self.highseq = lockseq;
        } else {
            if is_seqlower(lockseq, self.lowseq) {
                self.lowseq = lockseq;
            }
            if is_seqhigher(lockseq, self.highseq) {
                self.highseq = lockseq;
            }
        }
    }

    fn find_nextlowseq(&self) -> u32 {
        let mut lowest: Option<u32> = None;
        for q in &self.q {
            if !q.list.is_empty() && lowest.is_none_or(|l| is_seqlower(q.firstnum, l)) {
                lowest = Some(q.firstnum);
            }
        }
        lowest.unwrap_or(0)
    }

    fn find_nexthighseq(&self) -> u32 {
        let mut highest: Option<u32> = None;
        for q in &self.q {
            if !q.list.is_empty() && highest.is_none_or(|h| is_seqhigher(q.lastnum, h)) {
                highest = Some(q.lastnum);
            }
        }
        highest.unwrap_or(0)
    }

    /// `ksyn_queue_insert`: in arriving order for a first-fit mutex, by
    /// sequence otherwise. Two of one sequence is `EBUSY`.
    fn insert(&mut self, kqi: usize, kwe: Kwe, mgen: u32, firstfit: bool) -> Result<(), Errno> {
        let lockseq = mgen & COUNT_MASK;
        let kq = &mut self.q[kqi];
        if kq.list.is_empty() {
            kq.list.push(kwe);
            kq.firstnum = lockseq;
            kq.lastnum = lockseq;
        } else if firstfit {
            kq.list.push(kwe);
            if is_seqlower(lockseq, kq.firstnum) {
                kq.firstnum = lockseq;
            }
            if is_seqhigher(lockseq, kq.lastnum) {
                kq.lastnum = lockseq;
            }
        } else if lockseq == kq.firstnum || lockseq == kq.lastnum {
            return Err(Errno::EBUSY);
        } else if is_seqlower(kq.lastnum, lockseq) {
            kq.list.push(kwe);
            kq.lastnum = lockseq;
        } else if is_seqlower(lockseq, kq.firstnum) {
            kq.list.insert(0, kwe);
            kq.firstnum = lockseq;
        } else {
            let at = kq.list.iter().position(|q| is_seqhigher(q.lockseq, lockseq)).ok_or(Errno::ESRCH)?;
            kq.list.insert(at, kwe);
        }
        self.inqueue += 1;
        self.update_low_high(lockseq);
        Ok(())
    }

    /// `ksyn_queue_remove_item`
    fn remove(&mut self, kqi: usize, at: usize) -> Kwe {
        let kq = &mut self.q[kqi];
        let kwe = kq.list.remove(at);
        if let (Some(first), Some(last)) = (kq.list.first(), kq.list.last()) {
            kq.firstnum = first.lockseq & COUNT_MASK;
            kq.lastnum = last.lockseq & COUNT_MASK;
        } else {
            kq.firstnum = 0;
            kq.lastnum = 0;
        }
        self.inqueue -= 1;
        if self.inqueue > 0 {
            let curseq = kwe.lockseq & COUNT_MASK;
            if self.lowseq == curseq {
                self.lowseq = self.find_nextlowseq();
            }
            if self.highseq == curseq {
                self.highseq = self.find_nexthighseq();
            }
        } else {
            self.lowseq = 0;
            self.highseq = 0;
        }
        kwe
    }

    /// Take the thread `tid` off queue `kqi`, if it is still on it.
    fn remove_thread(&mut self, kqi: usize, tid: i32) -> bool {
        match self.q[kqi].list.iter().position(|k| k.state == State::InWait && k.tid == tid) {
            Some(at) => {
                self.remove(kqi, at);
                true
            }
            None => false,
        }
    }

    /// `ksyn_signal`: the waiter at `at` (the first, for none) leaves its
    /// queue with `updateval` as its answer.
    fn signal(&mut self, kqi: usize, at: Option<usize>, updateval: u32) {
        let at = at.unwrap_or(0);
        debug_assert!(self.q[kqi].list[at].state == State::InWait);
        let kwe = self.remove(kqi, at);
        self.woken.push((kwe.tid, updateval));
    }

    /// `ksyn_prepost`: an entry for something said before its waiter came.
    fn fake(&mut self, state: State, lockseq: u32) {
        let _ = self.insert(WRITE, Kwe { state, lockseq, count: 1, tid: 0 }, lockseq, false);
        self.fakecount += 1;
    }

    fn find_seq(&self, kqi: usize, seq: u32) -> Option<usize> {
        self.q[kqi].list.iter().position(|k| k.lockseq & COUNT_MASK == seq)
    }

    /// `ksyn_queue_find_cvpreposeq`: the thread at the sequence, or a
    /// prepost or broadcast at or above it.
    fn find_cvpreposeq(&self, cgen: u32) -> Option<usize> {
        let lgen = cgen & COUNT_MASK;
        let at = self.q[WRITE].list.iter().position(|k| is_seqhigher_eq(k.lockseq, cgen))?;
        let k = &self.q[WRITE].list[at];
        (!(k.state == State::InWait && k.lockseq & COUNT_MASK != lgen)).then_some(at)
    }

    /// `ksyn_queue_find_signalseq`: whom a signal up to `uptoseq` is for.
    /// Any thread at or below it, the one at the signal's own sequence for
    /// preference; or a prepost at it, or a broadcast above.
    fn find_signalseq(&self, uptoseq: u32, signalseq: u32) -> Option<usize> {
        let mut result = None;
        for (i, k) in self.q[WRITE].list.iter().enumerate() {
            match k.state {
                State::Prepost | State::Broadcast => {
                    if k.state == State::Prepost && is_seqhigher(k.lockseq, uptoseq) {
                        return result;
                    }
                    if is_seqlower(k.lockseq, uptoseq) {
                        continue;
                    }
                    return Some(i);
                }
                State::InWait => {
                    if is_seqhigher(k.lockseq, uptoseq) {
                        return result;
                    }
                    if is_seqhigher_eq(k.lockseq, signalseq) {
                        return Some(i);
                    }
                    if result.is_none() {
                        result = Some(i);
                    }
                }
            }
        }
        result
    }

    /// `ksyn_queue_free_items`: everything up to `upto` (or everything)
    /// goes. A thread among it is woken, told to count itself.
    fn free_items(&mut self, kqi: usize, upto: u32, all: bool) {
        let tseq = upto & COUNT_MASK;
        while let Some(k) = self.q[kqi].list.first().copied() {
            if !all && is_seqhigher(k.lockseq, tseq) {
                break;
            }
            if k.state == State::InWait {
                self.signal(kqi, Some(0), INC | MBIT | MTX_WAIT);
            } else {
                self.remove(kqi, 0);
                self.fakecount = self.fakecount.wrapping_sub(1);
            }
        }
    }

    /// `ksyn_queue_count_tolowest`
    fn count_tolowest(&self, kqi: usize, upto: u32) -> u32 {
        let kq = &self.q[kqi];
        if kq.list.is_empty() || is_seqhigher(kq.firstnum, upto) {
            return 0;
        }
        if upto == kq.firstnum {
            return 1;
        }
        let mut i = 0;
        for k in &kq.list {
            let curval = k.lockseq & COUNT_MASK;
            if is_seqhigher(curval, upto) {
                break;
            }
            i += 1;
            if upto == curval {
                break;
            }
        }
        i
    }

    /// `find_seq_till`: whether `nwaiters` wait at or below `upto`, and how
    /// many do.
    fn find_seq_till(&self, upto: u32, nwaiters: u32) -> (bool, u32) {
        let mut count = 0;
        for kqi in [READ, WRITE] {
            count += self.count_tolowest(kqi, upto);
            if count >= nwaiters {
                break;
            }
        }
        (count != 0 && count >= nwaiters, count)
    }

    /// `ksyn_handle_cvbroad`: every waiter up to `upto` is woken, and the
    /// preposts below it are spent. Returns how many, as sequence counts.
    fn handle_cvbroad(&mut self, upto: u32) -> u32 {
        let mut updatebits = 0u32;
        // (Each one looked at leaves the queue.)
        while let Some(k) = self.q[WRITE].list.first().copied() {
            if is_seqhigher(k.lockseq, upto) {
                break;
            }
            match k.state {
                State::InWait => {
                    self.signal(WRITE, Some(0), MTX_WAIT);
                    updatebits = updatebits.wrapping_add(INC);
                }
                State::Broadcast | State::Prepost => {
                    self.remove(WRITE, 0);
                    self.fakecount = self.fakecount.wrapping_sub(1);
                }
            }
        }
        // Waiters still to come (L is not yet S) find a broadcast.
        if diff_genseq(self.lword, self.sword) != 0 {
            self.fake(State::Broadcast, upto);
        }
        updatebits
    }

    /// `ksyn_cvupdate_fixup`: with L at S nothing is owed and the state is
    /// cleared (the C bit); with only preposts left, the P bit.
    fn cvupdate_fixup(&mut self, updatebits: &mut u32) {
        if self.lword & COUNT_MASK == self.sword & COUNT_MASK {
            if self.inqueue != 0 {
                self.free_items(WRITE, self.lword, false);
            }
            self.lword = 0;
            self.uword = 0;
            self.sword = 0;
            self.kflags |= KWF_ZEROEDOUT;
            *updatebits |= CV_CBIT;
        } else if self.inqueue != 0 && self.fakecount == self.inqueue {
            *updatebits |= CV_PBIT;
        }
    }

    /// `_ksyn_cvsignal_any`
    fn cvsignal_any(&mut self, uptoseq: u32, signalseq: u32, updatebits: &mut u32, broadcast: &mut bool) {
        match self.find_signalseq(uptoseq, signalseq) {
            Some(at) => {
                let k = self.q[WRITE].list[at];
                match k.state {
                    // A thread in range but below the signal: matching it
                    // could leave the one the signal was for with nobody
                    // to wake it, so everybody is woken.
                    State::InWait if is_seqlower(k.lockseq, signalseq) => *broadcast = true,
                    State::InWait => {
                        self.signal(WRITE, Some(at), MTX_WAIT);
                        *updatebits = updatebits.wrapping_add(INC);
                    }
                    State::Prepost => self.q[WRITE].list[at].count += 1,
                    State::Broadcast => {}
                }
            }
            None => self.fake(State::Prepost, uptoseq),
        }
    }

    /// `_ksyn_cvsignal_thread`: a signal for one thread by name.
    fn cvsignal_thread(&mut self, tid: i32, uptoseq: u32, signalseq: u32, updatebits: &mut u32, broadcast: &mut bool) {
        let Some(at) = self.q[WRITE].list.iter().position(|k| k.state == State::InWait && k.tid == tid) else {
            *broadcast = true;
            return;
        };
        let lockseq = self.q[WRITE].list[at].lockseq;
        self.signal(WRITE, Some(at), MTX_WAIT);
        if is_seqhigher(lockseq, uptoseq) || is_seqlower(lockseq, signalseq) {
            *broadcast = true;
            return;
        }
        *updatebits = updatebits.wrapping_add(INC);
    }

    /// `_psynch_mutexdrop_internal`: the mutex was unlocked with waiters
    /// its unlocker knew of. The next is woken; if it is not here yet, it
    /// finds this said when it comes.
    fn mutexdrop(&mut self, mgen: u32, ugen: u32, flags: u32) {
        let firstfit = flags & MTX_OPT_POLICY_MASK == MTX_OPT_POLICY_FIRSTFIT;
        let nextgen = ugen.wrapping_add(INC);
        self.lastunlockseq = ugen & COUNT_MASK;
        let updatebits = (self.highseq & COUNT_MASK) | EBIT | KBIT;
        if firstfit {
            if self.inqueue == 0 {
                self.mark_prepost(self.prepost.count + 1, mgen & COUNT_MASK, 0);
            } else {
                self.signal(WRITE, None, updatebits);
            }
            return;
        }
        let mut prepost = false;
        if self.inqueue == 0 {
            prepost = true;
        } else {
            let low_writer = self.q[WRITE].firstnum & COUNT_MASK;
            if low_writer == nextgen {
                // (The grant could be a condition's: the bit says the
                // mutex is waited for, in case the thread is interrupted.)
                self.signal(WRITE, None, updatebits | MTX_WAIT);
            } else if is_seqhigher(low_writer, nextgen) {
                prepost = true;
            } else {
                match self.find_seq(WRITE, nextgen) {
                    Some(at) => self.signal(WRITE, Some(at), updatebits | MTX_WAIT),
                    None => prepost = true,
                }
            }
        }
        if prepost && self.prepost.count == 0 {
            self.mark_prepost(1, nextgen & COUNT_MASK, 0);
        }
    }

    /// `_kwq_handle_overlap`: a reader arriving while readers are being
    /// let in, with no writer waiting, goes in with them.
    fn handle_overlap(&mut self, ty: u32, lgenval: u32, rw_wc: u32) -> Option<u32> {
        if ty != RW_TYPE_READ {
            return None;
        }
        if self.kflags & KWF_OVERLAP_GUARD != 0
            && rw_wc & WSVBIT == 0
            && lgenval & WBIT == 0
            && (is_seqlower_eq(rw_wc, self.nextseqword) || is_seqhigher_eq(self.lastseqword, rw_wc))
        {
            self.nextseqword = self.nextseqword.wrapping_add(INC);
            return Some(INC | (self.nextseqword & BIT_MASK) | MBIT);
        }
        None
    }

    /// `_kwq_handle_preposted_wakeup`: an unlock was said before all it
    /// was for had arrived; the last of them to arrive does its work.
    fn handle_preposted_wakeup(&mut self, ty: u32, lseq: u32) -> Option<u32> {
        if self.prepost.count == 0 || !is_seqlower_eq(lseq, self.prepost.lseq) {
            return None;
        }
        self.prepost.count -= 1;
        if self.prepost.count > 0 {
            return None;
        }
        let (pp_lseq, pp_sseq) = (self.prepost.lseq, self.prepost.sseq);
        self.clear_prepost();
        self.kflags &= !KWF_INITCLEARED;
        let (updatebits, block) = self.handle_unlock(pp_lseq, pp_sseq, ty | UNLOCK_PREPOST, lseq);
        (!block).then_some(updatebits)
    }

    /// `kwq_find_rw_lowest`: which kinds wait (shifted), which kind is
    /// lowest, and each queue's lowest sequence. `premgen` is the arriving
    /// thread's, which counts as waiting when `flags` says what it is.
    fn find_rw_lowest(&self, flags: u32, premgen: u32) -> (u32, [u32; 2]) {
        let mut ty = 0;
        let mut lowest = [0u32; 2];
        let mut numbers: Vec<(u32, u32)> = Vec::new();
        for (kqi, pre, shifted, plain) in [(READ, UNLOCK_PREPOST_READLOCK, RWSHFT_TYPE_READ, RW_TYPE_READ), (WRITE, UNLOCK_PREPOST_WRLOCK, RWSHFT_TYPE_WRITE, RW_TYPE_WRITE)] {
            let kq = &self.q[kqi];
            if kq.list.is_empty() && flags & pre == 0 {
                continue;
            }
            ty |= shifted;
            let first = if kq.list.is_empty() {
                premgen
            } else if flags & pre != 0 && is_seqlower(premgen, kq.firstnum) {
                premgen
            } else {
                kq.firstnum
            };
            lowest[kqi] = first;
            numbers.push((first, plain));
        }
        if let Some(&(mut low, mut lowtype)) = numbers.first() {
            for &(n, t) in &numbers[1..] {
                if is_seqlower(n, low) {
                    low = n;
                    lowtype = t;
                }
            }
            ty |= lowtype;
        }
        (ty, lowest)
    }

    /// `ksyn_wakeupreaders`: readers up to the lowest writer, or all.
    fn wakeupreaders(&mut self, limitread: u32, allreaders: bool, updatebits: u32) {
        while !self.q[READ].list.is_empty() && (allreaders || is_seqlower(self.q[READ].firstnum, limitread)) {
            self.signal(READ, None, updatebits);
        }
    }

    /// `kwq_handle_unlock`: grant the lock to who is next, on an unlock or
    /// on the arrival of the last a preposted unlock was for. Returns the
    /// bits the lock's word takes, and whether the arriving thread (for a
    /// prepost) is to wait all the same.
    fn handle_unlock(&mut self, _mgen: u32, rw_wc: u32, flags: u32, premgen: u32) -> (u32, bool) {
        let mut block = true;
        let mut updatebits = 0u32;
        self.lastseqword = rw_wc;
        self.lastunlockseq = rw_wc & COUNT_MASK;
        self.kflags &= !KWF_OVERLAP_GUARD;
        let (rwtype, lowest) = self.find_rw_lowest(flags, premgen);
        let low_writer = lowest[WRITE];
        match rwtype & RW_TYPE_MASK {
            RW_TYPE_READ => {
                if rwtype & RWSHFT_TYPE_MASK != 0 && rwtype & RWSHFT_TYPE_WRITE != 0 {
                    updatebits |= WBIT | KBIT;
                }
                let mut limitrdnum = 0;
                let mut allreaders = false;
                let mut numneeded;
                if rwtype & RWSHFT_TYPE_WRITE != 0 {
                    limitrdnum = low_writer;
                    numneeded = self.count_tolowest(READ, limitrdnum);
                    if flags & UNLOCK_PREPOST_READLOCK != 0 && is_seqlower(premgen, limitrdnum) {
                        block = false;
                        numneeded += 1;
                    }
                } else {
                    allreaders = true;
                    self.kflags |= KWF_OVERLAP_GUARD;
                    numneeded = self.q[READ].list.len() as u32;
                    if flags & UNLOCK_PREPOST_READLOCK != 0 {
                        block = false;
                        numneeded += 1;
                    }
                }
                updatebits = updatebits.wrapping_add(numneeded << 8);
                self.nextseqword = (rw_wc & COUNT_MASK).wrapping_add(updatebits);
                self.wakeupreaders(limitrdnum, allreaders, updatebits);
            }
            RW_TYPE_WRITE => {
                // Only one is granted.
                updatebits |= INC | KBIT | EBIT;
                if flags & UNLOCK_PREPOST_WRLOCK != 0 && low_writer == premgen {
                    block = false;
                    if !self.q[WRITE].list.is_empty() {
                        updatebits |= WBIT;
                    }
                } else {
                    // Not the arriving thread: with other writers waiting,
                    // or it, the W bit stays.
                    if self.q[WRITE].list.len() > 1 || flags & UNLOCK_PREPOST_WRLOCK != 0 {
                        updatebits |= WBIT;
                    }
                    if !self.q[WRITE].list.is_empty() {
                        self.signal(WRITE, None, updatebits);
                    }
                }
                self.nextseqword = (rw_wc & COUNT_MASK).wrapping_add(updatebits);
            }
            _ => {}
        }
        (updatebits, block)
    }
}

/// What a thread waits as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Mutex,
    Cvar,
    /// A read-write lock, on the queue named.
    Rwlock(usize),
}

/// A thread parked in one of the waiting calls.
#[derive(Clone, Copy, Debug)]
struct Wait {
    addr: u64,
    kind: Kind,
    /// When the wait is over regardless, on the kernel's clock.
    deadline: Option<u64>,
    /// The answer it was woken with (`kwe_psynchretval`), once it has
    /// been taken off its queue.
    granted: Option<u32>,
}

/// A process's psynch state: what its threads share, and a fork does not
/// inherit.
#[derive(Default, Debug)]
pub struct Psynch {
    kwqs: HashMap<u64, Kwq>,
    waits: HashMap<i32, Wait>,
    /// The page of words waiting threads sleep on, in the task's memory.
    words: u64,
    /// Threads granted something and not yet woken.
    wake: Vec<i32>,
    /// When the queues kept past their use were last looked through.
    swept: u64,
}

/// How a call here comes out.
enum Out {
    Done(Result<u32, Errno>),
    /// It waits: on its queue already.
    Park(Wait),
}

impl Psynch {
    /// `ksyn_wqfind`: the queue for `addr`, made if there is none, and
    /// counted as in use. An address used as one kind of thing cannot be
    /// used as another while anything of the first is outstanding.
    fn find(&mut self, addr: u64, mgen: u32, ugen: u32, sgen: u32, ty: Ty) -> Result<&mut Kwq, Errno> {
        let kwq = self.kwqs.entry(addr).or_default();
        kwq.stamp = None;
        if kwq.ty != Ty::None && kwq.ty != ty {
            if kwq.is_used() || kwq.users != 0 {
                return Err(Errno::EINVAL);
            }
            kwq.ty = Ty::None;
        }
        if kwq.ty == Ty::None {
            *kwq = Kwq { ty, ..Kwq::default() };
            kwq.clear_reinit_bits();
            kwq.lword = mgen;
            kwq.uword = ugen;
            kwq.sword = sgen;
        }
        kwq.users += 1;
        Ok(kwq)
    }

    /// `ksyn_wqrelease`: a call's hold on `addr`'s queue is over. Unused,
    /// the queue goes: now, or (a read-write lock's) after a while.
    fn release(&mut self, addr: u64, freenow: bool, now: u64) {
        self.settle(addr);
        if let Some(kwq) = self.kwqs.get_mut(&addr) {
            kwq.users = kwq.users.saturating_sub(1);
            if kwq.users == 0 && !kwq.is_used() {
                if freenow {
                    self.kwqs.remove(&addr);
                } else {
                    kwq.stamp = Some(now);
                }
            }
        }
        if now.saturating_sub(self.swept) >= KEEP_NS / 10 {
            self.swept = now;
            self.kwqs.retain(|_, k| !(k.users == 0 && !k.is_used() && k.stamp.is_some_and(|s| now.saturating_sub(s) >= KEEP_NS)));
        }
    }

    /// Tell the threads `addr`'s queue let go their answers.
    fn settle(&mut self, addr: u64) {
        let Some(kwq) = self.kwqs.get_mut(&addr) else { return };
        for (tid, val) in kwq.woken.drain(..) {
            if let Some(w) = self.waits.get_mut(&tid) {
                w.granted = Some(val);
            }
            self.wake.push(tid);
        }
    }

    /// `ksyn_wait`: `tid` onto `addr`'s queue `kqi`.
    fn enqueue(&mut self, addr: u64, kqi: usize, tid: i32, lockseq: u32, firstfit: bool) -> Result<(), Errno> {
        let kwq = self.kwqs.get_mut(&addr).ok_or(Errno::EINVAL)?;
        kwq.insert(kqi, Kwe { state: State::InWait, lockseq: lockseq & COUNT_MASK, count: 1, tid }, lockseq, firstfit)
    }

    /// `psynch_mutexwait(mutex, mgen, ugen, tid, flags)`
    fn mutexwait(&mut self, tid: i32, x: [u64; 8], now: u64) -> Out {
        let (mutex, mgen, ugen, flags) = (x[0], x[1] as u32, x[2] as u32, x[4] as u32);
        let firstfit = flags & MTX_OPT_POLICY_MASK == MTX_OPT_POLICY_FIRSTFIT;
        let lseq = mgen & COUNT_MASK;
        let kwq = match self.find(mutex, mgen, ugen, 0, Ty::Mutex) {
            Ok(k) => k,
            Err(e) => return Out::Done(Err(e)),
        };
        if kwq.prepost.count != 0 && (firstfit || lseq == kwq.prepost.lseq) {
            // The unlock came first: the lock is this thread's.
            kwq.prepost.count -= 1;
            if !firstfit {
                if kwq.prepost.count > 0 {
                    kwq.prepost.lseq = kwq.prepost.lseq.wrapping_add(INC);
                    self.release(mutex, true, now);
                    return Out::Done(Err(Errno::EINVAL));
                }
                kwq.clear_prepost();
            }
            let updatebits = if kwq.inqueue == 0 { lseq | KBIT | EBIT } else { (kwq.highseq & COUNT_MASK) | KBIT | EBIT };
            self.release(mutex, true, now);
            return Out::Done(Ok(updatebits & !MTX_WAIT));
        }
        if let Err(e) = self.enqueue(mutex, WRITE, tid, mgen, firstfit) {
            self.release(mutex, true, now);
            return Out::Done(Err(e));
        }
        Out::Park(Wait { addr: mutex, kind: Kind::Mutex, deadline: None, granted: None })
    }

    /// `psynch_mutexdrop(mutex, mgen, ugen, tid, flags)`
    fn mutexdrop(&mut self, mutex: u64, mgen: u32, ugen: u32, flags: u32, now: u64) -> Result<u32, Errno> {
        self.find(mutex, mgen, ugen, 0, Ty::Mutex)?.mutexdrop(mgen, ugen, flags);
        self.release(mutex, true, now);
        Ok(0)
    }

    /// `__psynch_cvsignal`: one waiter of the condition, `thread` when it
    /// is named, or (`broadcast`) all.
    #[allow(clippy::too_many_arguments)]
    fn cvsignal(&mut self, cv: u64, cgen: u32, cugen: u32, csgen: u32, mut broadcast: bool, named: bool, thread: Option<i32>, now: u64) -> Result<u32, Errno> {
        let uptoseq = cgen & COUNT_MASK;
        let fromseq = (cugen & COUNT_MASK).wrapping_add(INC);
        if (!named && is_seqhigher(fromseq, uptoseq)) || is_seqhigher(csgen, uptoseq) {
            return Err(Errno::EINVAL);
        }
        if named && thread.is_none() {
            return Err(Errno::ESRCH);
        }
        let kwq = self.find(cv, cgen, cugen, csgen, Ty::Cvar)?;
        let mut updatebits = 0;
        kwq.update_cv(cgen, cugen, csgen);
        if !broadcast && diff_genseq(kwq.lword, kwq.sword) != 0 {
            match thread {
                Some(tid) => kwq.cvsignal_thread(tid, uptoseq, fromseq, &mut updatebits, &mut broadcast),
                None => kwq.cvsignal_any(uptoseq, fromseq, &mut updatebits, &mut broadcast),
            }
        }
        if broadcast {
            updatebits |= kwq.handle_cvbroad(uptoseq);
        }
        kwq.sword = kwq.sword.wrapping_add(updatebits & COUNT_MASK);
        kwq.cvupdate_fixup(&mut updatebits);
        self.release(cv, true, now);
        Ok(updatebits)
    }

    /// `psynch_cvwait(cv, cvlsgen, cvugen, mutex, mugen, flags, sec, nsec)`:
    /// drop the mutex and wait on the condition, as one step.
    fn cvwait(&mut self, tid: i32, x: [u64; 8], now: u64) -> Out {
        let (cv, cvlsgen, cvugen, mutex, mugen, flags) = (x[0], x[1], x[2] as u32, x[3], x[4], x[5] as u32);
        let (sec, nsec) = (x[6] as i64, x[7] as u32 & 0x3fff_ffff);
        let (csgen, cgen) = ((cvlsgen >> 32) as u32, cvlsgen as u32);
        let (ugen, mgen) = ((mugen >> 32) as u32, mugen as u32);
        let lockseq = cgen & COUNT_MASK;
        // (U may be out of range, for a condition used only for its
        // timeouts; S may not.)
        if is_seqhigher_eq(csgen, lockseq) {
            return Out::Done(Err(Errno::EINVAL));
        }
        if let Err(e) = self.find(cv, cgen, cvugen, csgen, Ty::Cvar) {
            return Out::Done(Err(e));
        }
        if mutex != 0 {
            if let Err(e) = self.mutexdrop(mutex, mgen, ugen, flags, now) {
                self.release(cv, true, now);
                return Out::Done(Err(e));
            }
        }
        let Some(kwq) = self.kwqs.get_mut(&cv) else { return Out::Done(Err(Errno::EINVAL)) };
        kwq.update_cv(cgen, cvugen, csgen);
        // A signal that came before this waiter, or a thread already at
        // its sequence.
        if let Some(at) = kwq.find_cvpreposeq(lockseq) {
            let k = kwq.q[WRITE].list[at];
            let mut updatebits = 0;
            let mut error = None;
            match k.state {
                State::Prepost if k.lockseq & COUNT_MASK == lockseq => {
                    kwq.q[WRITE].list[at].count -= 1;
                    if kwq.q[WRITE].list[at].count == 0 {
                        kwq.remove(WRITE, at);
                        kwq.fakecount = kwq.fakecount.wrapping_sub(1);
                    }
                }
                // A prepost above this sequence can be taken, but that
                // could leave the higher thread unmatched: it becomes a
                // broadcast.
                State::Prepost => updatebits |= kwq.handle_cvbroad(k.lockseq),
                State::Broadcast => {}
                State::InWait => error = Some(Errno::EBUSY),
            }
            let r = match error {
                Some(e) => Err(e),
                None => {
                    updatebits |= INC;
                    kwq.sword = kwq.sword.wrapping_add(INC);
                    kwq.cvupdate_fixup(&mut updatebits);
                    Ok(updatebits)
                }
            };
            self.release(cv, true, now);
            return Out::Done(r);
        }
        let deadline = (sec != 0 || nsec != 0).then(|| now.saturating_add((sec.max(0) as u64).saturating_mul(1_000_000_000).saturating_add(nsec as u64)));
        if let Err(e) = self.enqueue(cv, WRITE, tid, cgen, false) {
            self.release(cv, true, now);
            return Out::Done(Err(e));
        }
        Out::Park(Wait { addr: cv, kind: Kind::Cvar, deadline, granted: None })
    }

    /// `psynch_cvclrprepost(cv, cvgen, cvugen, cvsgen, prepocnt, preposeq,
    /// flags)`: what was said before its waiter came is taken back.
    fn cvclrprepost(&mut self, x: [u64; 8], now: u64) -> Result<u32, Errno> {
        let (cv, cvgen, cvugen, cvsgen, preposeq, flags) = (x[0], x[1] as u32, x[2] as u32, x[3] as u32, x[5] as u32, x[6] as u32);
        let mutex = flags & MTX_OPT_MUTEX != 0;
        let kwq = self.find(cv, cvgen, cvugen, if mutex { 0 } else { cvsgen }, if mutex { Ty::Mutex } else { Ty::Cvar })?;
        if mutex {
            let firstfit = flags & MTX_OPT_POLICY_MASK == MTX_OPT_POLICY_FIRSTFIT;
            if firstfit && kwq.prepost.count != 0 && is_seqlower_eq(kwq.prepost.lseq, cvgen) {
                kwq.clear_prepost();
            }
        } else {
            kwq.free_items(WRITE, preposeq, false);
        }
        self.release(cv, true, now);
        Ok(0)
    }

    /// `__psynch_rw_lock(type, rwlock, lgenval, ugenval, rw_wc, flags)`
    fn rw_lock(&mut self, tid: i32, read: bool, x: [u64; 8], now: u64) -> Out {
        let (rwlock, lgenval, ugenval, rw_wc) = (x[0], x[1] as u32, x[2] as u32, x[3] as u32);
        let lockseq = lgenval & COUNT_MASK;
        let (ty, prepost_type, kqi) = if read { (RW_TYPE_READ, UNLOCK_PREPOST_READLOCK, READ) } else { (RW_TYPE_WRITE, UNLOCK_PREPOST_WRLOCK, WRITE) };
        let kwq = match self.find(rwlock, lgenval, ugenval, rw_wc, Ty::Rwlock) {
            Ok(k) => k,
            Err(e) => return Out::Done(Err(e)),
        };
        kwq.check_init(lgenval);
        let ready = kwq.handle_overlap(ty, lgenval, rw_wc).or_else(|| kwq.handle_preposted_wakeup(prepost_type, lockseq));
        if let Some(v) = ready {
            self.release(rwlock, false, now);
            return Out::Done(Ok(v));
        }
        if let Err(e) = self.enqueue(rwlock, kqi, tid, lgenval, false) {
            self.release(rwlock, false, now);
            return Out::Done(Err(e));
        }
        // (Its readers may have been let go by the prepost's handling.)
        self.settle(rwlock);
        Out::Park(Wait { addr: rwlock, kind: Kind::Rwlock(kqi), deadline: None, granted: None })
    }

    /// `psynch_rw_unlock(rwlock, lgenval, ugenval, rw_wc, flags)`: grant
    /// the lock to who is next; if not all the unlocker knew of are here
    /// yet, the last of them to arrive does it.
    fn rw_unlock(&mut self, x: [u64; 8], now: u64) -> Result<u32, Errno> {
        let (rwlock, lgenval, ugenval, rw_wc) = (x[0], x[1] as u32, x[2] as u32, x[3] as u32);
        let curgen = lgenval & COUNT_MASK;
        let kwq = self.find(rwlock, lgenval, ugenval, rw_wc, Ty::Rwlock)?;
        let isinit = kwq.check_init(lgenval);
        let mut updatebits = 0;
        // One lower than the last that unlocked is spurious.
        if !(kwq.lastunlockseq != RWL_INIT && is_seqlower(ugenval, kwq.lastunlockseq)) {
            let diff = find_diff(lgenval, ugenval);
            let (all, count) = kwq.find_seq_till(curgen, diff);
            if !all && (count == 0 || count < diff) {
                // Not all are in place: keep the highest such unlock.
                if kwq.prepost.sseq & SBIT != 0 || is_seqhigher_eq(rw_wc, kwq.prepost.sseq) {
                    kwq.mark_prepost(diff - count, curgen, rw_wc);
                    updatebits = lgenval;
                }
            } else {
                if isinit && kwq.kflags & KWF_INITCLEARED != 0 {
                    kwq.kflags &= !KWF_INITCLEARED;
                }
                kwq.clear_prepost();
                updatebits = kwq.handle_unlock(lgenval, rw_wc, 0, 0).0;
            }
        }
        self.release(rwlock, false, now);
        Ok(updatebits)
    }

    /// What XNU's continuations do: finish the wait `w` of `tid`, with the
    /// answer it was granted, or with `error` when none came.
    fn resume(&mut self, tid: i32, w: Wait, error: Errno, now: u64) -> Result<u32, Errno> {
        let r = match (w.kind, w.granted) {
            (Kind::Mutex, Some(v)) => Ok(v & !MTX_WAIT),
            (Kind::Rwlock(_), Some(v)) => Ok(v),
            // Woken as its condition was cleared: it counts itself.
            (Kind::Cvar, Some(v)) => Ok(if v & MBIT != 0 { INC | CV_CBIT } else { 0 }),
            (Kind::Mutex, None) | (Kind::Rwlock(_), None) => {
                let kqi = if let Kind::Rwlock(kqi) = w.kind { kqi } else { WRITE };
                if let Some(kwq) = self.kwqs.get_mut(&w.addr) {
                    kwq.remove_thread(kqi, tid);
                }
                Err(error)
            }
            (Kind::Cvar, None) => {
                // The waiter leaves uncounted by any signal: S moves for
                // it, and it is told what that left of the kernel's state.
                let mut bits = 0;
                if let Some(kwq) = self.kwqs.get_mut(&w.addr) {
                    kwq.remove_thread(WRITE, tid);
                    kwq.sword = kwq.sword.wrapping_add(INC);
                    if kwq.lword & COUNT_MASK == kwq.sword & COUNT_MASK {
                        bits |= ECVCLEARED;
                        if kwq.inqueue != 0 {
                            kwq.free_items(WRITE, kwq.lword, true);
                        }
                        kwq.lword = 0;
                        kwq.uword = 0;
                        kwq.sword = 0;
                        kwq.kflags |= KWF_ZEROEDOUT;
                    } else if kwq.inqueue != 0 && kwq.fakecount == kwq.inqueue {
                        bits |= ECVPREPOST;
                    }
                }
                Err(if bits == 0 { error } else { abi::errno_raw(abi::errno_to_darwin(error) | bits) })
            }
        };
        self.release(w.addr, !matches!(w.kind, Kind::Rwlock(_)), now);
        r
    }
}

impl Kernel {
    /// One of the psynch calls. Its answer is left for
    /// [`Kernel::darwin_native`] (`darwin_answer`), or it waits: the
    /// canonical call that sleeps on the thread's word is returned, its
    /// arguments in `a`, with [`Post::Psynch`] left to finish it.
    pub(super) fn darwin_psynch(&mut self, m: &mut GuestMem<'_>, code: u64, x: [u64; 8], a: &mut [u64; 6]) -> u64 {
        let shared = Arc::clone(&self.darwin_psynch);
        let mut ps = shared.lock().unwrap_or_else(|e| e.into_inner());
        let now = clock_ns(self.clock, 1);
        let tid = self.tid;
        let kind = match code {
            d::PSYNCH_MUTEXWAIT => Some(Kind::Mutex),
            d::PSYNCH_CVWAIT => Some(Kind::Cvar),
            d::PSYNCH_RW_RDLOCK => Some(Kind::Rwlock(READ)),
            d::PSYNCH_RW_WRLOCK => Some(Kind::Rwlock(WRITE)),
            _ => None,
        };
        // A thread whose wait is over makes its call again where the host
        // could not finish it: this is that call, and the wait's end.
        let mut out = None;
        if let Some(w) = ps.waits.remove(&tid) {
            let error = if w.deadline.is_some_and(|at| now >= at) { Errno::ETIMEDOUT } else { Errno::EINTR };
            let r = ps.resume(tid, w, error, now);
            if kind == Some(w.kind) && w.addr == x[0] {
                out = Some(Out::Done(r));
            }
        }
        let out = out.unwrap_or_else(|| match code {
            d::PSYNCH_MUTEXWAIT => ps.mutexwait(tid, x, now),
            d::PSYNCH_MUTEXDROP => Out::Done(ps.mutexdrop(x[0], x[1] as u32, x[2] as u32, x[4] as u32, now)),
            // (cv, cvlsgen, cvudgen, flags, mutex, mugen, tid)
            d::PSYNCH_CVBROAD => Out::Done(if (x[2] as u32) >> 8 > THREAD_MAX {
                Err(Errno::EBUSY)
            } else {
                ps.cvsignal(x[0], x[1] as u32, (x[2] >> 32) as u32, (x[1] >> 32) as u32, true, false, None, now)
            }),
            // (cv, cvlsgen, cvugen, thread_port, mutex, mugen, tid, flags)
            d::PSYNCH_CVSIGNAL => {
                let port = x[3] as u32;
                let thread = super::mach::port_thread(self.pid, port);
                Out::Done(ps.cvsignal(x[0], x[1] as u32, x[2] as u32, (x[1] >> 32) as u32, false, port != 0, thread, now))
            }
            d::PSYNCH_CVWAIT => ps.cvwait(tid, x, now),
            d::PSYNCH_CVCLRPREPOST => Out::Done(ps.cvclrprepost(x, now)),
            d::PSYNCH_RW_RDLOCK => ps.rw_lock(tid, true, x, now),
            d::PSYNCH_RW_WRLOCK => ps.rw_lock(tid, false, x, now),
            d::PSYNCH_RW_UNLOCK => Out::Done(ps.rw_unlock(x, now)),
            // The long read lock and the yielding write lock, which no
            // libpthread has asked for in years; the upgrade and downgrade
            // that do nothing; and an unlock nobody implemented.
            d::PSYNCH_RW_LONGRDLOCK | d::PSYNCH_RW_YIELDWRLOCK => Out::Done(Err(Errno::ESRCH)),
            d::PSYNCH_RW_UPGRADE | d::PSYNCH_RW_DOWNGRADE => Out::Done(Ok(0)),
            _ => Out::Done(Err(Errno::EOPNOTSUPP)),
        });
        let n = match out {
            Out::Done(r) => {
                self.darwin_answer = Some(r.map(u64::from));
                NATIVE + code
            }
            Out::Park(w) => match self.darwin_psynch_park(m, &mut ps, w, now, a) {
                Ok(call) => call,
                Err(e) => {
                    let _ = ps.resume(tid, w, e, now);
                    self.darwin_answer = Some(Err(e));
                    NATIVE + code
                }
            },
        };
        self.darwin_psynch_wake(m, &mut ps);
        n
    }

    /// The word thread `tid` sleeps on, in the page the task's memory
    /// holds for them (made at the first need).
    fn darwin_psynch_word(&mut self, m: &mut GuestMem<'_>, ps: &mut Psynch, tid: i32) -> Result<u64, Errno> {
        if ps.words == 0 || self.mem.lock().vma_at(ps.words).is_none() {
            ps.words = self.mem.mmap_anon(m, 0, abi::PAGE_SIZE, 3, 0, false, "[psynch]")?;
        }
        Ok(ps.words + 4 * (tid as u64 & 0xfff))
    }

    /// Put this thread to sleep as `w`: the canonical call that waits on
    /// its word as it is now, until `w`'s time.
    fn darwin_psynch_park(&mut self, m: &mut GuestMem<'_>, ps: &mut Psynch, w: Wait, now: u64, a: &mut [u64; 6]) -> Result<u64, Errno> {
        let at = self.darwin_psynch_word(m, ps, self.tid)?;
        let seen = m.read_u32(at)?;
        let mut ts = 0;
        if let Some(deadline) = w.deadline {
            let left = deadline.saturating_sub(now).max(1);
            ts = self.ctx_addr + SCRATCH_SLOT as u64;
            m.write_u64(ts, left / 1_000_000_000)?;
            m.write_u64(ts + 8, left % 1_000_000_000)?;
        }
        ps.waits.insert(self.tid, w);
        self.darwin_post = Post::Psynch;
        *a = [at, crate::futex::op::WAIT as u64, seen as u64, ts, 0, 0];
        Ok(nr::FUTEX)
    }

    /// Wake the threads that were granted something.
    fn darwin_psynch_wake(&mut self, m: &mut GuestMem<'_>, ps: &mut Psynch) {
        let wake = std::mem::take(&mut ps.wake);
        if ps.words == 0 {
            return;
        }
        for tid in wake {
            let at = ps.words + 4 * (tid as u64 & 0xfff);
            let v = m.read_u32(at).unwrap_or(0);
            let _ = m.write_u32(at, v.wrapping_add(1));
            let _ = self.sys_futex(m, [at, crate::futex::op::WAKE as u64, i32::MAX as u64, 0, 0, 0]);
        }
    }

    /// A psynch wait is over, `r` being how the sleep ended: the call's
    /// answer. Granted, it is what it was granted; otherwise its time ran
    /// out, or (a signal, or a wake that was another thread's) it was
    /// interrupted, which libpthread takes as cause to look again.
    pub(super) fn darwin_psynch_end(&mut self, m: &mut GuestMem<'_>, r: SysResult) -> SysResult {
        let shared = Arc::clone(&self.darwin_psynch);
        let mut ps = shared.lock().unwrap_or_else(|e| e.into_inner());
        let now = clock_ns(self.clock, 1);
        let Some(w) = ps.waits.remove(&self.tid) else { return Err(Errno::EINTR) };
        let error = if r == Err(Errno::ETIMEDOUT) || w.deadline.is_some_and(|at| now >= at) { Errno::ETIMEDOUT } else { Errno::EINTR };
        let r = ps.resume(self.tid, w, error, now);
        self.darwin_psynch_wake(m, &mut ps);
        r.map(u64::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRSTFIT: u64 = MTX_OPT_POLICY_FIRSTFIT as u64;

    fn granted(ps: &Psynch, tid: i32) -> Option<u32> {
        ps.waits.get(&tid).and_then(|w| w.granted)
    }

    /// As the kernel does on a park.
    fn park(ps: &mut Psynch, tid: i32, out: Out) {
        match out {
            Out::Park(w) => {
                ps.waits.insert(tid, w);
            }
            Out::Done(r) => panic!("did not wait: {r:?}"),
        }
    }

    #[test]
    fn sequences_wrap() {
        assert!(is_seqlower(0x100, 0x200));
        assert!(is_seqhigher(0x200, 0x100));
        assert!(is_seqlower(0xffff_ff00, 0x100));
        assert!(is_seqhigher(0x100, 0xffff_ff00));
        assert!(is_seqlower_eq(0x105, 0x1ff));
        assert_eq!(diff_genseq(0x300, 0x100), 0x200);
        assert_eq!(diff_genseq(0x100, 0xffff_ff00), 0x200);
        assert_eq!(find_diff(0x500, 0x200), 3);
    }

    #[test]
    fn a_first_fit_mutex_wakes_its_first_waiter_or_remembers() {
        let mut ps = Psynch::default();
        // Nobody waits: the drop is remembered, and the next to come has it.
        assert_eq!(ps.mutexdrop(0x1000, 0x200, 0x100, FIRSTFIT as u32, 0), Ok(0));
        match ps.mutexwait(7, [0x1000, 0x300, 0x100, 0, FIRSTFIT, 0, 0, 0], 0) {
            Out::Done(Ok(v)) => assert_eq!(v, 0x300 | KBIT | EBIT),
            _ => panic!("the prepost was not taken"),
        }
        assert!(ps.kwqs.is_empty());
        // Two wait; each drop wakes the one that came first.
        let out = ps.mutexwait(7, [0x1000, 0x400, 0x100, 0, FIRSTFIT, 0, 0, 0], 0);
        park(&mut ps, 7, out);
        let out = ps.mutexwait(8, [0x1000, 0x500, 0x100, 0, FIRSTFIT, 0, 0, 0], 0);
        park(&mut ps, 8, out);
        ps.mutexdrop(0x1000, 0x500, 0x200, FIRSTFIT as u32, 0).unwrap();
        assert_eq!(granted(&ps, 7), Some(0x500 | KBIT | EBIT));
        assert_eq!(granted(&ps, 8), None);
        assert_eq!(ps.wake, vec![7]);
        let w = ps.waits.remove(&7).unwrap();
        assert_eq!(ps.resume(7, w, Errno::EINTR, 0), Ok(0x500 | KBIT | EBIT));
        // The other is interrupted: it leaves its queue, and the queue goes.
        let w = ps.waits.remove(&8).unwrap();
        assert_eq!(ps.resume(8, w, Errno::EINTR, 0), Err(Errno::EINTR));
        assert!(ps.kwqs.is_empty());
    }

    #[test]
    fn a_condition_signalled_before_its_waiter_arrives() {
        let mut ps = Psynch::default();
        // L is 1 (one waiter on its way), U and S 0: the signal is kept.
        let r = ps.cvsignal(0x2000, 0x100, 0, 0, false, false, None, 0).unwrap();
        assert_eq!(r & CV_PBIT, CV_PBIT);
        // The waiter arrives and takes it: one answered, and with L at S
        // the kernel has nothing more.
        match ps.cvwait(3, [0x2000, 0x100, 0, 0, 0, 0, 0, 0], 0) {
            Out::Done(Ok(v)) => assert_eq!(v, INC | CV_CBIT),
            _ => panic!("the prepost was not taken"),
        }
        assert!(ps.kwqs.is_empty());
    }

    #[test]
    fn a_condition_waiter_is_signalled_or_times_out() {
        let mut ps = Psynch::default();
        let out = ps.cvwait(3, [0x2000, 0x100, 0, 0, 0, 0, 1, 0], 0);
        park(&mut ps, 3, out);
        assert_eq!(ps.waits[&3].deadline, Some(1_000_000_000));
        let out = ps.cvwait(4, [0x2000, 0x200, 0, 0, 0, 0, 0, 0], 0);
        park(&mut ps, 4, out);
        // A signal for the first: it is woken, S moves by one.
        let r = ps.cvsignal(0x2000, 0x200, 0, 0, false, false, None, 0).unwrap();
        assert_eq!(r, INC);
        assert_eq!(granted(&ps, 3), Some(MTX_WAIT));
        let w = ps.waits.remove(&3).unwrap();
        assert_eq!(ps.resume(3, w, Errno::ETIMEDOUT, 0), Ok(0));
        // The second gives up: S moves for it, L is S, and it is told the
        // kernel's state is cleared.
        let w = ps.waits.remove(&4).unwrap();
        assert_eq!(ps.resume(4, w, Errno::ETIMEDOUT, 0), Err(abi::errno_raw(60 | ECVCLEARED)));
        assert!(ps.kwqs.is_empty());
    }

    #[test]
    fn a_broadcast_wakes_every_waiter() {
        let mut ps = Psynch::default();
        for (tid, l) in [(3, 0x100u64), (4, 0x200), (5, 0x300)] {
            let out = ps.cvwait(tid, [0x2000, l, 0, 0, 0, 0, 0, 0], 0);
            park(&mut ps, tid, out);
        }
        let r = ps.cvsignal(0x2000, 0x300, 0, 0, true, false, None, 0).unwrap();
        assert_eq!(r, 0x300 | CV_CBIT);
        assert_eq!(ps.wake, vec![3, 4, 5]);
    }

    #[test]
    fn a_writer_waits_for_the_readers_unlock() {
        let mut ps = Psynch::default();
        // Two readers hold it (L is 2); a writer arrives as the third.
        let out = ps.rw_lock(9, false, [0x3000, 0x300 | WBIT as u64 | KBIT as u64, 0, 0, 0, 0, 0, 0], 0);
        park(&mut ps, 9, out);
        // The first reader's unlock knows of one waiter at or below 3, the
        // writer, which is there: it is granted the lock alone.
        let r = ps.rw_unlock([0x3000, 0x300 | WBIT as u64 | KBIT as u64, 0x200, 0x200, 0, 0, 0, 0], 0).unwrap();
        assert_eq!(r, INC | KBIT | EBIT);
        assert_eq!(granted(&ps, 9), Some(INC | KBIT | EBIT));
    }
}
