//! Work queues: the threads XNU makes for a process that asks for work to
//! be done (`workq_kernreturn`), which is how libdispatch gets its threads.
//! libpthread asks for some number at a priority; the kernel makes them, or
//! wakes ones it has, each on a stack of the kernel's laying out, entered at
//! the routine libpthread registered (`_start_wqthread`). A thread that has
//! done its work comes back (`WQOPS_THREAD_RETURN`) and is given more, or
//! waits for some, or after a while is told to end.
//!
//! This is XNU's (`pthread_workqueue.c`, and libpthread's `kern_support.c`
//! for the stack and the registers): requests are served in the order
//! made, by any thread; there is one pool, whatever the priority, and no
//! limit by processor. A request is for a thread to run libdispatch's
//! routine, or to be told of events: the work queue's own kqueue's, or a
//! workloop's (`WQ_FLAG_THREAD_KEVENT`, [`super::kqwl`]), the events laid
//! out under its `pthread_t` as XNU lays them. A thread told of events
//! comes back with the changes it made of them (`WQOPS_THREAD_KEVENT_RETURN`,
//! `WQOPS_THREAD_WORKLOOP_RETURN`) and is told of more while there are any.
//! One of the pool is the kernel's: it never runs the program's code, and
//! waits on everything those queues watch, to ask for a thread when one
//! has something to say ([`POLL_STUB`]).
//!
//! One system call can start one thread. A request for several starts the
//! first, and each new thread, before it is libpthread's, starts the next:
//! it begins in a few instructions of the kernel's own ([`STUB`]) that make
//! two calls, one for the next thread and one to be given its work.
use super::nr as d;
use super::*;
use crate::syscalls::{clock_ns, nr};
use std::collections::{HashMap, HashSet, VecDeque};

/// `workq_kernreturn`'s operations.
pub(super) mod op {
    pub const THREAD_RETURN: u64 = 0x004;
    pub const THREAD_KEVENT_RETURN: u64 = 0x040;
    pub const THREAD_WORKLOOP_RETURN: u64 = 0x100;
    pub const QUEUE_NEWSPISUPP: u64 = 0x010;
    pub const QUEUE_REQTHREADS: u64 = 0x020;
    pub const QUEUE_REQTHREADS2: u64 = 0x030;
    pub const SET_EVENT_MANAGER_PRIORITY: u64 = 0x080;
    pub const SHOULD_NARROW: u64 = 0x200;
    pub const SETUP_DISPATCH: u64 = 0x400;
    /// The stub's two, which are no operations of XNU's: start the next
    /// thread there is work for, and give this one its own.
    pub const NEXT: u64 = 0x7ff1;
    pub const BEGIN: u64 = 0x7ff2;
    /// The kernel's own thread's, made again and again ([`super::POLL_STUB`]).
    pub const POLL: u64 = 0x7ff3;
}

/// What a thread is entered with (`upcall_flags`): its quality of service
/// in the low bits, and how it comes to be running.
pub(super) mod flag {
    pub const PRIO_QOS: u32 = 0x0000_4000;
    pub const OVERCOMMIT: u32 = 0x0001_0000;
    pub const REUSE: u32 = 0x0002_0000;
    pub const NEWSPI: u32 = 0x0004_0000;
    pub const KEVENT: u32 = 0x0008_0000;
    pub const EVENT_MANAGER: u32 = 0x0010_0000;
    pub const TSD_BASE_SET: u32 = 0x0020_0000;
    pub const WORKLOOP: u32 = 0x0040_0000;
    pub const COOPERATIVE: u32 = 0x0100_0000;
}

/// A thread's stack, as `workq_thread_allocsize` lays it out: a guard page,
/// this much to run on, and above it the page or pages its `pthread_t` is
/// in (on arm64 some way into them, to share a page with the stack's top).
const STACK: u64 = 512 * 1024;
const PTHREAD_T_OFFSET: u64 = if gex_abi::X86 { 0 } else { 12 * 1024 };
/// The count of events that tells a thread it is to end
/// (`WORKQ_EXIT_THREAD_NKEVENT`).
const EXIT: i64 = -1;
/// How many threads a queue may have, and how long one waits for work
/// before it is let go.
pub(super) const MOST: usize = 64;
const IDLE: u64 = 5_000_000_000;

/// A thread's events, under its `pthread_t` (`WQ_KEVENT_LIST_LEN` of
/// `struct kevent_qos_s`), the workloop's id under them, and the room for
/// the messages they receive under that (`WQ_KEVENT_DATA_SIZE`).
pub(super) const KEVENT_LIST: u64 = 16;
pub(super) const KEVENT_SIZE: u64 = 72;
pub(super) const KEVENT_DATA: u64 = 32 * 1024;

/// Where in the kernel's page of code a new thread begins.
pub const STUB: u64 = commpage::TEXT + 0x100;
/// Where the kernel's own thread of the pool runs: a call, made again for
/// ever, that waits on the queues the threads are told of events from.
pub const POLL_STUB: u64 = commpage::TEXT + 0x140;
const POLL_ARM64: [u32; 4] = [
    0xd280_2e10, // mov x16, #368
    0xd28f_fe60, // mov x0, #0x7ff3
    0xd400_1001, // svc #0x80
    0x17ff_fffd, // b .-12
];
const POLL_X86: [u8; 14] = [
    0xb8, 0x70, 0x01, 0x00, 0x02, // mov eax, 0x2000170
    0xbf, 0xf3, 0x7f, 0x00, 0x00, // mov edi, 0x7ff3
    0x0f, 0x05, // syscall
    0xeb, 0xf2, // jmp .-14
];
const STUB_ARM64: [u32; 5] = [
    0xd400_1001, // svc #0x80        (x16 368, x0 NEXT: the kernel's)
    0xd280_2e10, // mov x16, #368
    0xd28f_fe40, // mov x0, #0x7ff2
    0xd400_1001, // svc #0x80
    0xd420_0020, // brk #1
];
const STUB_X86: [u8; 16] = [
    0x0f, 0x05, // syscall           (rax 0x2000170, rdi NEXT: the kernel's)
    0xb8, 0x70, 0x01, 0x00, 0x02, // mov eax, 0x2000170
    0xbf, 0xf2, 0x7f, 0x00, 0x00, // mov edi, 0x7ff2
    0x0f, 0x05, // syscall
    0x0f, 0x0b, // ud2
];

/// What a thread is asked for to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    /// Run libdispatch's routine, entered with these flags.
    Plain(u32),
    /// Be told of a queue's events ([`super::kqwl`]).
    Events(super::kqwl::Target),
    /// Be the kernel's own thread, which waits on the queues.
    Poll,
}

/// One of the queue's threads.
#[derive(Clone, Copy, Debug)]
pub(super) struct Worker {
    /// Its stack, from the guard page up.
    pub stack: u64,
    /// Since when it has waited for work, if it does.
    parked: Option<u64>,
    /// Work given it while it waited, or made for.
    given: Option<Work>,
    /// Whether it has yet run (its first entry sets its `pthread_t` up).
    fresh: bool,
    /// The work it last had: the class it is of, which it is entered with
    /// again to be told to end (libpthread stops a thread entered with no
    /// class at all, "Missing priority").
    pub last: u32,
    /// The queue whose events it was last told of, while it has not come
    /// back without more ([`super::kqwl`]).
    pub bound: Option<super::kqwl::Target>,
}

/// A process's work queue.
#[derive(Debug, Default)]
pub struct Workq {
    /// Requests no thread has yet.
    pub(super) pending: VecDeque<Work>,
    pub(super) workers: HashMap<i32, Worker>,
    /// The page the waiting threads' words are in, 0 before one waits.
    pub(super) words: u64,
    /// Threads made and not yet seen to begin (they are in the stub).
    starting: HashSet<i32>,
    /// The queues whose events the threads are told of.
    pub(super) events: super::kqwl::Events,
}

/// What a `workq_kernreturn` ends in other than an answer
/// ([`Kernel::darwin_workq_outcome`]).
#[derive(Clone, Copy, Debug)]
pub enum Does {
    /// A thread is made for this work; the caller is answered 0, or what
    /// its own call was (`darwin_answer`).
    Make(Work),
    /// This thread is entered again at libpthread's routine: with these
    /// flags, and this count of events (0, or [`EXIT`]), which with
    /// [`flag::KEVENT`] are under its `pthread_t` ([`event_areas`]).
    Enter(u32, i64),
    /// This thread goes to be the kernel's own ([`POLL_STUB`]).
    Poll,
}

/// Where a thread whose `pthread_t` is at `me` is told of events: the
/// list, the workloop's id (under the list), the room for messages, and
/// the top of the stack it runs on under all that.
pub(super) fn event_areas(me: u64) -> (u64, u64, u64, u64) {
    let list = me - KEVENT_LIST * KEVENT_SIZE;
    let id = list - 8;
    let data = id - KEVENT_DATA;
    (list, id, data, (data - 64) & !15)
}

/// The `pthread_t` of a thread of the pool whose stack is at `stack`.
pub(super) fn pthread_of(stack: u64) -> u64 {
    stack + abi::PAGE_SIZE + STACK + PTHREAD_T_OFFSET
}

/// A `pthread_priority_t` as the flags a thread is entered with: its class
/// is one bit of six from bit 8, numbered from 1; none is the default's.
pub(super) fn flags_of(priority: u64) -> u32 {
    let class = match (priority >> 8) & 0x3f {
        0 => 4,
        bits => bits.trailing_zeros() + 1,
    };
    let mut f = flag::NEWSPI | flag::PRIO_QOS | class;
    for (bit, means) in [(0x8000_0000u64, flag::OVERCOMMIT), (0x0800_0000, flag::COOPERATIVE), (0x0200_0000, flag::EVENT_MANAGER)] {
        if priority & bit != 0 {
            f |= means;
        }
    }
    f
}

impl Kernel {
    /// The kernel's page of code, made at its first need: where a
    /// `posix_spawn`'s copy re-enters the kernel, and where a work queue's
    /// thread begins.
    pub(super) fn darwin_text_page(&mut self, m: &mut GuestMem<'_>) {
        if self.mem.mmap_anon(m, commpage::TEXT, commpage::PAGE, 7, mflags::FIXED_NOREPLACE, false, "[commpage text]").is_ok() {
            let _ = m.write(commpage::TEXT, &super::spawn::text());
            let stub: Vec<u8> = if gex_abi::X86 { STUB_X86.to_vec() } else { STUB_ARM64.iter().flat_map(|w| w.to_le_bytes()).collect() };
            let _ = m.write(STUB, &stub);
            let poll: Vec<u8> = if gex_abi::X86 { POLL_X86.to_vec() } else { POLL_ARM64.iter().flat_map(|w| w.to_le_bytes()).collect() };
            let _ = m.write(POLL_STUB, &poll);
        }
    }

    /// `workq_kernreturn(options, item, arg2, arg3)`. Its answer is left
    /// for [`Kernel::darwin_native`], or what it ends in for
    /// [`Kernel::darwin_workq_outcome`]; or the thread waits for work, as
    /// the canonical call returned with its arguments in `a`.
    pub(super) fn darwin_workq(&mut self, m: &mut GuestMem<'_>, x: [u64; 8], a: &mut [u64; 6]) -> u64 {
        let shared = Arc::clone(&self.darwin_workq);
        let mut wq = shared.lock().unwrap_or_else(|e| e.into_inner());
        let tid = self.tid;
        let answer = match x[0] {
            op::QUEUE_NEWSPISUPP | op::SETUP_DISPATCH | op::SET_EVENT_MANAGER_PRIORITY | op::SHOULD_NARROW => Ok(0),
            op::QUEUE_REQTHREADS | op::QUEUE_REQTHREADS2 | op::NEXT => {
                if x[0] != op::NEXT {
                    if !self.darwin_threads.lock().unwrap_or_else(|e| e.into_inner()).registered {
                        self.darwin_answer = Some(Err(Errno::EINVAL));
                        return NATIVE + d::WORKQ_KERNRETURN;
                    }
                    // (Some threads, at a priority that names a class:
                    // the event manager's own names none.)
                    if x[2] as i32 <= 0 || (x[3] >> 8) & 0x3f == 0 {
                        self.darwin_answer = Some(Err(Errno::EINVAL));
                        return NATIVE + d::WORKQ_KERNRETURN;
                    }
                    let flags = flags_of(x[3]);
                    for _ in 0..(x[2] as i32).clamp(0, MOST as i32) {
                        wq.pending.push_back(Work::Plain(flags));
                    }
                }
                // The threads that wait take what there is; then one is
                // made, which makes the next.
                if let Some(work) = self.darwin_workq_hand_out(m, &mut wq) {
                    self.darwin_workq_does = Some(Does::Make(work));
                    return NATIVE + d::WORKQ_KERNRETURN;
                }
                Ok(0)
            }
            // A thread told of events, back with the changes it made of
            // them: told of more, or let go of the queue.
            op::THREAD_KEVENT_RETURN | op::THREAD_WORKLOOP_RETURN => {
                let Some(w) = wq.workers.get(&tid).copied() else {
                    self.darwin_answer = Some(Err(Errno::EINVAL));
                    return NATIVE + d::WORKQ_KERNRETURN;
                };
                if let Some(t) = w.bound {
                    if let Some((flags, n)) = self.darwin_events_return(m, &mut wq, w, t, x[1], x[2]) {
                        self.darwin_workq_does = Some(Does::Enter(flags | flag::REUSE, n));
                        return NATIVE + d::WORKQ_KERNRETURN;
                    }
                }
                // (Made again, should it wait, as the plain return it now is.)
                let at = self.ctx_addr + if gex_abi::X86 { ctx::x(7) } else { ctx::x(0) } as u64;
                let _ = m.write_u64(at, op::THREAD_RETURN);
                return self.darwin_workq_return(m, &mut wq, a);
            }
            op::BEGIN | op::THREAD_RETURN => return self.darwin_workq_return(m, &mut wq, a),
            op::POLL if wq.events.poller == Some(tid) => return self.darwin_events_poll(m, &mut wq, a),
            _ => Err(Errno::EINVAL),
        };
        self.darwin_answer = Some(answer);
        NATIVE + d::WORKQ_KERNRETURN
    }

    /// Give the threads that wait for work what there is of it; what is
    /// left, if a thread may be made for it, is the work to make one for.
    pub(super) fn darwin_workq_hand_out(&mut self, m: &mut GuestMem<'_>, wq: &mut Workq) -> Option<Work> {
        let idle: Vec<i32> = wq.workers.iter().filter(|(_, w)| w.parked.is_some() && w.given.is_none()).map(|(t, _)| *t).collect();
        for t in idle {
            let Some(work) = wq.pending.pop_front() else { break };
            wq.workers.get_mut(&t).unwrap().given = Some(work);
            if wq.words != 0 {
                let at = wq.words + 4 * (t as u64 & 0xfff);
                let v = m.read_u32(at).unwrap_or(0);
                let _ = m.write_u32(at, v.wrapping_add(1));
                let _ = self.sys_futex(m, [at, crate::futex::op::WAKE as u64, i32::MAX as u64, 0, 0, 0]);
            }
        }
        if wq.workers.len() < MOST {
            return wq.pending.pop_front();
        }
        None
    }

    /// The thread is the queue's to give work: entered with some, told
    /// of events, made the kernel's own, or waiting for work (the canonical
    /// call, with its arguments in `a`), or told to end.
    fn darwin_workq_return(&mut self, m: &mut GuestMem<'_>, wq: &mut Workq, a: &mut [u64; 6]) -> u64 {
        let (tid, now) = (self.tid, clock_ns(self.clock, 1));
        let Some(mut w) = wq.workers.get(&tid).copied() else {
            self.darwin_answer = Some(Err(Errno::EINVAL));
            return NATIVE + d::WORKQ_KERNRETURN;
        };
        wq.starting.remove(&tid);
        // (A thread that came back from a queue's events is that queue's
        // no longer.)
        if let Some(t) = w.bound.take() {
            wq.workers.insert(tid, w);
            self.darwin_events_unbind(wq, t);
        }
        while let Some(work) = w.given.take().or_else(|| wq.pending.pop_front()) {
            // Work: the thread is entered with it, as a new one (its
            // thread pointer set already) or one used again.
            let how = if w.fresh { flag::TSD_BASE_SET } else { flag::REUSE };
            match work {
                Work::Plain(flags) => {
                    w.fresh = false;
                    w.parked = None;
                    w.last = flags;
                    wq.workers.insert(tid, w);
                    self.darwin_workq_does = Some(Does::Enter(flags | how, 0));
                    return NATIVE + d::WORKQ_KERNRETURN;
                }
                Work::Events(t) => {
                    // (Nothing to say by now: the next.)
                    if let Some((flags, n)) = self.darwin_events_deliver(m, wq, w, t) {
                        w.fresh = false;
                        w.parked = None;
                        w.last = flags;
                        w.bound = Some(t);
                        wq.workers.insert(tid, w);
                        self.darwin_workq_does = Some(Does::Enter(flags | how, n));
                        return NATIVE + d::WORKQ_KERNRETURN;
                    }
                }
                Work::Poll => {
                    w.parked = None;
                    wq.workers.insert(tid, w);
                    wq.events.poller = Some(tid);
                    self.darwin_workq_does = Some(Does::Poll);
                    return NATIVE + d::WORKQ_KERNRETURN;
                }
            }
        }
        let since = *w.parked.get_or_insert(now);
        wq.workers.insert(tid, w);
        if now.saturating_sub(since) >= IDLE {
            // Long enough without: it is told to end, and ends itself,
            // freeing its stack as it goes. (Entered as a plain thread of
            // the class it last had.)
            wq.workers.remove(&tid);
            let last = w.last & !(flag::KEVENT | flag::WORKLOOP | flag::EVENT_MANAGER);
            let last = if last & (flag::PRIO_QOS | 0xfff) == 0 { last | flag::PRIO_QOS | 4 } else { last };
            self.darwin_workq_does = Some(Does::Enter(last | flag::REUSE, EXIT));
            return NATIVE + d::WORKQ_KERNRETURN;
        }
        // Wait for work on the thread's word, for what is left of its
        // time; then ask again.
        let at = match self.darwin_workq_word(m, wq, tid) {
            Ok(at) => at,
            Err(e) => {
                self.darwin_answer = Some(Err(e));
                return NATIVE + d::WORKQ_KERNRETURN;
            }
        };
        let seen = m.read_u32(at).unwrap_or(0);
        let left = (IDLE - now.saturating_sub(since)).max(1);
        let ts = self.ctx_addr + SCRATCH_SLOT as u64;
        let _ = m.write_u64(ts, left / 1_000_000_000);
        let _ = m.write_u64(ts + 8, left % 1_000_000_000);
        self.darwin_post = Post::Workq;
        *a = [at, crate::futex::op::WAIT as u64, seen as u64, ts, 0, 0];
        nr::FUTEX
    }

    /// The word thread `tid` waits on, in the page the queue keeps for
    /// them (made at its first need).
    pub(super) fn darwin_workq_word(&mut self, m: &mut GuestMem<'_>, wq: &mut Workq, tid: i32) -> Result<u64, Errno> {
        if wq.words == 0 || self.mem.lock().vma_at(wq.words).is_none() {
            wq.words = self.mem.mmap_anon(m, 0, abi::PAGE_SIZE, 3, 0, false, "[workq]")?;
        }
        Ok(wq.words + 4 * (tid as u64 & 0xfff))
    }

    /// Wake thread `tid` from its word.
    pub(super) fn darwin_workq_wake(&mut self, m: &mut GuestMem<'_>, wq: &mut Workq, tid: i32) {
        if let Ok(at) = self.darwin_workq_word(m, wq, tid) {
            let v = m.read_u32(at).unwrap_or(0);
            let _ = m.write_u32(at, v.wrapping_add(1));
            let _ = self.sys_futex(m, [at, crate::futex::op::WAKE as u64, i32::MAX as u64, 0, 0, 0]);
        }
    }

    /// What a `workq_kernreturn` ends in, when that is not an answer: a
    /// thread made, or this one entered at libpthread's routine.
    pub(super) fn darwin_workq_outcome(&mut self, m: &mut GuestMem<'_>) -> Option<SyscallOutcome> {
        let does = self.darwin_workq_does.take()?;
        let reg = *self.darwin_threads.lock().unwrap_or_else(|e| e.into_inner());
        let page = abi::PAGE_SIZE;
        // (self, port, the stack's lowest address, events, flags, how many
        // events): x0-x5, or rdi, rsi, rdx, rcx, r8, r9.
        let regs: [usize; 6] = if gex_abi::X86 { [ctx::x(7), ctx::x(6), ctx::x(2), ctx::x(1), ctx::x(8), ctx::x(9)] } else { [ctx::x(0), ctx::x(1), ctx::x(2), ctx::x(3), ctx::x(4), ctx::x(5)] };
        match does {
            Does::Make(work) => {
                let made = (|| -> Result<SpawnRequest, Errno> {
                    self.darwin_text_page(m);
                    // The stack: a guard page, the stack, the `pthread_t`.
                    let top = (reg.pthsize + PTHREAD_T_OFFSET).next_multiple_of(page);
                    // (where the thread is told it is: in a process's window, `gex_abi::DARWIN_BASE`)
                    let stack = gex_abi::guest_addr(self.mem.mmap_anon(m, 0, page + STACK + top, 3, 0, false, "[workq stack]")?);
                    let _ = self.map_io().call(m, nr::MPROTECT, [stack, page, 0, 0, 0, 0]);
                    let me = stack + page + STACK + PTHREAD_T_OFFSET;
                    let fs = self.fs_of(self.ctx_addr);
                    let how = clone_flags::VM | clone_flags::THREAD | clone_flags::FS;
                    let req = self.thread_maker(&fs).make(m, self.ctx_addr, [how, me & !15, 0, 0, 0, 0])?;
                    let child = req.ctx_addr;
                    let port = super::mach::thread_port(self.pid, req.tid);
                    // Its thread pointer, and in what hangs off that the
                    // name of its own port, as for any thread.
                    if reg.tsd_offset != 0 {
                        let tsd = me + reg.tsd_offset;
                        m.write_u64(child + ctx::TPIDRRO as u64, tsd)?;
                        m.write_u32(tsd + super::thread::TSD_MACH_THREAD_SELF * 8, port)?;
                    } else {
                        m.write_u64(child + ctx::TPIDRRO as u64, 0)?;
                    }
                    // It begins in the stub, about to ask for the next.
                    let (number, first) = if gex_abi::X86 { (ctx::x(0), ctx::x(7)) } else { (ctx::x(16), ctx::x(0)) };
                    m.write_u64(child + number as u64, if gex_abi::X86 { 0x0200_0000 | d::WORKQ_KERNRETURN } else { d::WORKQ_KERNRETURN })?;
                    m.write_u64(child + first as u64, op::NEXT)?;
                    m.write_u64(child + ctx::PC as u64, STUB)?;
                    let mut wq = self.darwin_workq.lock().unwrap_or_else(|e| e.into_inner());
                    let last = if let Work::Plain(flags) = work { flags } else { flag::NEWSPI | flag::PRIO_QOS | 4 };
                    wq.workers.insert(req.tid, Worker { stack, parked: None, given: Some(work), fresh: true, last, bound: None });
                    wq.starting.insert(req.tid);
                    Ok(req)
                })();
                // (What the call that made it answers: 0 for a request of
                // threads, a `kevent`'s own answer.)
                let answer = match self.darwin_answer.take() {
                    Some(r) => r.map_or_else(|e| e.as_return(), |v| v),
                    None => 0,
                };
                Some(match made {
                    Ok(req) => {
                        if self.trace {
                            eprintln!("[gex:{}] workq: thread {} made for {:?}", self.pid, req.tid, work);
                        }
                        let _ = set_result(m, self.ctx_addr, answer);
                        SyscallOutcome::SpawnThread { ctx_addr: req.ctx_addr, tid: req.tid }
                    }
                    // No thread: the work waits for one that comes back.
                    Err(_) => {
                        let mut wq = self.darwin_workq.lock().unwrap_or_else(|e| e.into_inner());
                        wq.pending.push_front(work);
                        if work == Work::Poll {
                            wq.events.poller = None;
                        }
                        drop(wq);
                        let _ = set_result(m, self.ctx_addr, answer);
                        SyscallOutcome::Continue
                    }
                })
            }
            Does::Enter(flags, events) => {
                let stack = self.darwin_workq.lock().unwrap_or_else(|e| e.into_inner()).workers.get(&self.tid).map(|w| w.stack);
                // (One told to end is no longer the queue's: its stack is
                // where its thread pointer says.)
                let me = match stack {
                    Some(stack) => stack + page + STACK + PTHREAD_T_OFFSET,
                    None => m.read_u64(self.ctx_addr + ctx::TPIDRRO as u64).unwrap_or(0).wrapping_sub(reg.tsd_offset),
                };
                let bottom = me - PTHREAD_T_OFFSET - STACK;
                let port = super::mach::thread_port(self.pid, self.tid) as u64;
                let c = self.ctx_addr;
                // (Events are under the `pthread_t`, and the stack under them.)
                let (list, sp) = if flags & flag::KEVENT != 0 && events >= 0 {
                    let (list, _, _, top) = event_areas(me);
                    (list, top)
                } else {
                    (0, me)
                };
                for (slot, v) in regs.into_iter().zip([me, port, bottom, list, flags as u64, events as u64]) {
                    let _ = m.write_u64(c + slot as u64, v);
                }
                let _ = m.write_u64(c + ctx::SP as u64, sp & !15);
                if !gex_abi::X86 {
                    let _ = m.write_u64(c + ctx::x(29) as u64, 0);
                    let _ = m.write_u64(c + ctx::x(30) as u64, 0);
                }
                let start = gex_abi::canon(reg.wqthread);
                let _ = m.write_u64(c + ctx::PC as u64, start);
                Some(SyscallOutcome::Resume(start))
            }
            Does::Poll => {
                self.darwin_text_page(m);
                if self.trace {
                    eprintln!("[gex:{}] workq: thread {} waits on the queues", self.pid, self.tid);
                }
                let _ = set_result(m, self.ctx_addr, 0);
                let _ = m.write_u64(self.ctx_addr + ctx::PC as u64, POLL_STUB);
                Some(SyscallOutcome::Resume(POLL_STUB))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_priority_is_a_class_and_what_kind_of_thread() {
        // QOS_CLASS_DEFAULT is the fourth bit; none is the default too.
        assert_eq!(flags_of(0x0800), flag::NEWSPI | flag::PRIO_QOS | 4);
        assert_eq!(flags_of(0), flag::NEWSPI | flag::PRIO_QOS | 4);
        // User-interactive, overcommitted.
        assert_eq!(flags_of(0x8000_2000), flag::NEWSPI | flag::PRIO_QOS | flag::OVERCOMMIT | 6);
        assert_eq!(flags_of(0x0100) & 0xfff, 1);
    }
}
