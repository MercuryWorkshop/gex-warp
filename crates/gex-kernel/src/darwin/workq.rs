//! Work queues: the threads XNU makes for a process that asks for work to
//! be done (`workq_kernreturn`), which is how libdispatch gets its threads.
//! libpthread asks for some number at a priority; the kernel makes them, or
//! wakes ones it has, each on a stack of the kernel's laying out, entered at
//! the routine libpthread registered (`_start_wqthread`). A thread that has
//! done its work comes back (`WQOPS_THREAD_RETURN`) and is given more, or
//! waits for some, or after a while is told to end.
//!
//! This is the plain half of XNU's (`pthread_workqueue.c`, and libpthread's
//! `kern_support.c` for the stack and the registers): requests are served
//! in the order made, by any thread; there is one pool, whatever the
//! priority, and no limit by processor. The other half, events of a kqueue
//! delivered to these threads (`WQ_FLAG_THREAD_KEVENT`, workloops), is not
//! here, and `bsdthread_register` does not claim it.
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
mod op {
    pub const THREAD_RETURN: u64 = 0x004;
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
}

/// What a thread is entered with (`upcall_flags`): its quality of service
/// in the low bits, and how it comes to be running.
mod flag {
    pub const PRIO_QOS: u32 = 0x0000_4000;
    pub const OVERCOMMIT: u32 = 0x0001_0000;
    pub const REUSE: u32 = 0x0002_0000;
    pub const NEWSPI: u32 = 0x0004_0000;
    pub const EVENT_MANAGER: u32 = 0x0010_0000;
    pub const TSD_BASE_SET: u32 = 0x0020_0000;
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
const MOST: usize = 64;
const IDLE: u64 = 5_000_000_000;

/// Where in the kernel's page of code a new thread begins.
pub const STUB: u64 = commpage::TEXT + 0x100;
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

/// One of the queue's threads.
#[derive(Clone, Copy, Debug)]
struct Worker {
    /// Its stack, from the guard page up.
    stack: u64,
    /// Since when it has waited for work, if it does.
    parked: Option<u64>,
    /// Work given it while it waited, or made for.
    given: Option<u32>,
    /// Whether it has yet run (its first entry sets its `pthread_t` up).
    fresh: bool,
    /// The work it last had: the class it is of, which it is entered with
    /// again to be told to end (libpthread stops a thread entered with no
    /// class at all, "Missing priority").
    last: u32,
}

/// A process's work queue.
#[derive(Debug, Default)]
pub struct Workq {
    /// Requests no thread has yet, each the flags its thread is entered
    /// with.
    pending: VecDeque<u32>,
    workers: HashMap<i32, Worker>,
    /// The page the waiting threads' words are in, 0 before one waits.
    words: u64,
    /// Threads made and not yet seen to begin (they are in the stub).
    starting: HashSet<i32>,
}

/// What a `workq_kernreturn` ends in other than an answer
/// ([`Kernel::darwin_workq_outcome`]).
#[derive(Clone, Copy, Debug)]
pub enum Does {
    /// A thread is made for this work; the caller is answered 0.
    Make(u32),
    /// This thread is entered again at libpthread's routine: with these
    /// flags, and this count of events (0, or [`EXIT`]).
    Enter(u32, i64),
}

/// A `pthread_priority_t` as the flags a thread is entered with: its class
/// is one bit of six from bit 8, numbered from 1; none is the default's.
fn flags_of(priority: u64) -> u32 {
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
        }
    }

    /// `workq_kernreturn(options, item, arg2, arg3)`. Its answer is left
    /// for [`Kernel::darwin_native`], or what it ends in for
    /// [`Kernel::darwin_workq_outcome`]; or the thread waits for work, as
    /// the canonical call returned with its arguments in `a`.
    pub(super) fn darwin_workq(&mut self, m: &mut GuestMem<'_>, x: [u64; 8], a: &mut [u64; 6]) -> u64 {
        let shared = Arc::clone(&self.darwin_workq);
        let mut wq = shared.lock().unwrap_or_else(|e| e.into_inner());
        let (tid, now) = (self.tid, clock_ns(self.clock, 1));
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
                        wq.pending.push_back(flags);
                    }
                }
                // The threads that wait take what there is; then one is
                // made, which makes the next.
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
                    if let Some(work) = wq.pending.pop_front() {
                        self.darwin_workq_does = Some(Does::Make(work));
                        return NATIVE + d::WORKQ_KERNRETURN;
                    }
                }
                Ok(0)
            }
            op::BEGIN | op::THREAD_RETURN => {
                let Some(mut w) = wq.workers.get(&tid).copied() else {
                    self.darwin_answer = Some(Err(Errno::EINVAL));
                    return NATIVE + d::WORKQ_KERNRETURN;
                };
                wq.starting.remove(&tid);
                let work = w.given.take().or_else(|| wq.pending.pop_front());
                if let Some(work) = work {
                    // Work: the thread is entered with it, as a new one
                    // (its thread pointer set already) or one used again.
                    let how = if std::mem::take(&mut w.fresh) { flag::TSD_BASE_SET } else { flag::REUSE };
                    w.parked = None;
                    w.last = work;
                    wq.workers.insert(tid, w);
                    self.darwin_workq_does = Some(Does::Enter(work | how, 0));
                    return NATIVE + d::WORKQ_KERNRETURN;
                }
                let since = *w.parked.get_or_insert(now);
                wq.workers.insert(tid, w);
                if now.saturating_sub(since) >= IDLE {
                    // Long enough without: it is told to end, and ends
                    // itself, freeing its stack as it goes.
                    wq.workers.remove(&tid);
                    self.darwin_workq_does = Some(Does::Enter(w.last | flag::REUSE, EXIT));
                    return NATIVE + d::WORKQ_KERNRETURN;
                }
                // Wait for work on the thread's word, for what is left of
                // its time; then ask again.
                if wq.words == 0 || self.mem.lock().vma_at(wq.words).is_none() {
                    match self.mem.mmap_anon(m, 0, abi::PAGE_SIZE, 3, 0, false, "[workq]") {
                        Ok(at) => wq.words = at,
                        Err(e) => {
                            self.darwin_answer = Some(Err(e));
                            return NATIVE + d::WORKQ_KERNRETURN;
                        }
                    }
                }
                let at = wq.words + 4 * (tid as u64 & 0xfff);
                let seen = m.read_u32(at).unwrap_or(0);
                let left = (IDLE - now.saturating_sub(since)).max(1);
                let ts = self.ctx_addr + SCRATCH_SLOT as u64;
                let _ = m.write_u64(ts, left / 1_000_000_000);
                let _ = m.write_u64(ts + 8, left % 1_000_000_000);
                self.darwin_post = Post::Workq;
                *a = [at, crate::futex::op::WAIT as u64, seen as u64, ts, 0, 0];
                return nr::FUTEX;
            }
            _ => Err(Errno::EINVAL),
        };
        self.darwin_answer = Some(answer);
        NATIVE + d::WORKQ_KERNRETURN
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
                    let stack = self.mem.mmap_anon(m, 0, page + STACK + top, 3, 0, false, "[workq stack]")?;
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
                    wq.workers.insert(req.tid, Worker { stack, parked: None, given: Some(work), fresh: true, last: work });
                    wq.starting.insert(req.tid);
                    Ok(req)
                })();
                Some(match made {
                    Ok(req) => {
                        if self.trace {
                            eprintln!("[gex:{}] workq: thread {} made", self.pid, req.tid);
                        }
                        let _ = set_result(m, self.ctx_addr, 0);
                        SyscallOutcome::SpawnThread { ctx_addr: req.ctx_addr, tid: req.tid }
                    }
                    // No thread: the work waits for one that comes back.
                    Err(_) => {
                        self.darwin_workq.lock().unwrap_or_else(|e| e.into_inner()).pending.push_front(work);
                        let _ = set_result(m, self.ctx_addr, 0);
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
                for (slot, v) in regs.into_iter().zip([me, port, bottom, 0, flags as u64, events as u64]) {
                    let _ = m.write_u64(c + slot as u64, v);
                }
                let _ = m.write_u64(c + ctx::SP as u64, me & !15);
                if !gex_abi::X86 {
                    let _ = m.write_u64(c + ctx::x(29) as u64, 0);
                    let _ = m.write_u64(c + ctx::x(30) as u64, 0);
                }
                let _ = m.write_u64(c + ctx::PC as u64, reg.wqthread);
                Some(SyscallOutcome::Resume(reg.wqthread))
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
