//! Threads, as libpthread asks XNU for them.
//!
//! A Darwin process registers its thread entry point once
//! (`bsdthread_register`); every thread `bsdthread_create` makes starts
//! there, with its `pthread_t`, its port, its function and argument in the
//! argument registers, on the stack libpthread allocated, and its thread
//! pointer set to the thread-specific data inside its `pthread_t`. A thread
//! ends with `bsdthread_terminate`, which also frees its stack. Waiting on
//! a word of memory is `ulock_wait` and `ulock_wake` (XNU's
//! `bsd/kern/sys_ulock.c`), which are the kernel's futexes with Darwin's
//! arguments and answers. The kernel side of these is libpthread's own
//! `kern/kern_support.c`.

use super::nr as d;
use super::*;
use crate::syscalls::nr;

/// `bsdthread_create`'s flags: libpthread set up the stack and the
/// `pthread_t` itself (the only kind there has been since 10.10).
const PTHREAD_START_CUSTOM: u64 = 0x0100_0000;
/// Told to the new thread: its thread pointer is already set.
const PTHREAD_START_TSD_BASE_SET: u64 = 0x1000_0000;
/// Asked and never told: the thread starts suspended.
const PTHREAD_START_SUSPENDED: u64 = 0x0400_0000;

/// What `bsdthread_register` answers: the features libpthread may use.
/// Fine-grained priorities, `bsdthread_ctl` and its set-self, the
/// maintenance and default QoS classes; not the workqueue ones, which there
/// is no workqueue behind.
const PTHREAD_FEATURES: u64 = 0x02 | 0x04 | 0x08 | 0x10 | 0x4000_0000;

/// `ulock`'s operations and flags.
const UL_COMPARE_AND_WAIT: u32 = 1;
const UL_UNFAIR_LOCK: u32 = 2;
const UL_COMPARE_AND_WAIT_SHARED: u32 = 3;
const UL_UNFAIR_LOCK64_SHARED: u32 = 4;
const UL_COMPARE_AND_WAIT64: u32 = 5;
const UL_COMPARE_AND_WAIT64_SHARED: u32 = 6;
const ULF_WAKE_ALL: u32 = 0x100;
pub const ULF_NO_ERRNO: u32 = 0x0100_0000;

/// What a process told the kernel of its threads (`bsdthread_register`),
/// shared by them and copied by a fork.
#[derive(Clone, Copy, Debug, Default)]
pub struct Registration {
    pub registered: bool,
    /// Where every new thread starts (`_thread_start`).
    pub threadstart: u64,
    /// Where a workqueue thread would (`_start_wqthread`).
    pub wqthread: u64,
    /// The size of a `pthread_t`.
    pub pthsize: u64,
    /// Where in a `pthread_t` its thread-specific data begins: the new
    /// thread's thread pointer.
    pub tsd_offset: u64,
}

impl Kernel {
    /// `bsdthread_register(threadstart, wqthread, pthsize, data, datasize,
    /// dispatchqueue_offset)`. `data` is a `struct _pthread_registration_data`:
    /// version (its size) at 0, dispatch_queue_offset at 8, main_qos at 16
    /// (written back), tsd_offset at 24, and the rest, of which the stack
    /// hint at 36 and the mutex policy at 44 are written back.
    pub(super) fn darwin_bsdthread_register(&mut self, m: &mut GuestMem<'_>, x: [u64; 8]) -> SysResult {
        let (threadstart, wqthread, pthsize, data, size) = (x[0], x[1], x[2] as i32, x[3], x[4]);
        if !(0..=64 * 1024).contains(&pthsize) {
            return Err(Errno::EINVAL);
        }
        let mut tsd_offset = 0;
        if data != 0 {
            if size < 8 || m.read_u64(data)? != size {
                return Err(Errno::EINVAL);
            }
            if size >= 28 {
                tsd_offset = m.read_u32(data + 24)? as u64;
            }
        }
        let mut reg = self.darwin_threads.lock().unwrap_or_else(|e| e.into_inner());
        if reg.registered {
            return Err(Errno::EINVAL);
        }
        if tsd_offset + 8 > pthsize as u64 {
            tsd_offset = 0;
        }
        *reg = Registration { registered: true, threadstart, wqthread, pthsize: pthsize as u64, tsd_offset };
        drop(reg);
        if data != 0 {
            // _pthread_unspecified_priority: no QoS is imposed on the main
            // thread. No hint for where stacks go; the default mutex policy
            // (first fit).
            if size >= 24 {
                m.write_u64(data + 16, 0)?;
            }
            if size >= 44 {
                m.write_u64(data + 36, 0)?;
            }
            if size >= 48 {
                m.write_u32(data + 44, 3)?;
            }
        }
        Ok(PTHREAD_FEATURES)
    }

    /// The calls that end in something other than an answer: a thread made,
    /// a thread ended.
    pub(in crate::syscalls) fn darwin_outcome(&mut self, m: &mut GuestMem<'_>, n: u64) -> Option<SyscallOutcome> {
        let x = self.darwin_x;
        match n.checked_sub(NATIVE)? {
            d::BSDTHREAD_CREATE => Some(match self.darwin_bsdthread_create(m, x) {
                Ok(req) => {
                    // The caller's answer is the `pthread_t` it passed.
                    let _ = set_result(m, self.ctx_addr, x[3]);
                    if self.trace {
                        eprintln!("[gex:{}] bsdthread_create(func={:#x}, stack={:#x}, pthread={:#x}) = thread {}", self.pid, x[0], x[2], x[3], req.tid);
                    }
                    SyscallOutcome::SpawnThread { ctx_addr: req.ctx_addr, tid: req.tid }
                }
                Err(e) => {
                    let _ = set_result(m, self.ctx_addr, e.as_return());
                    SyscallOutcome::Continue
                }
            }),
            // (stackaddr, freesize, kthport, sem): the stack goes, then the
            // thread. The last thread's end is the process's.
            d::BSDTHREAD_TERMINATE => {
                if x[0] != 0 && x[1] != 0 {
                    let _ = self.map_io().call(m, nr::MUNMAP, [x[0], x[1], 0, 0, 0, 0]);
                }
                if self.trace {
                    eprintln!("[gex:{}] bsdthread_terminate (thread {})", self.pid, self.tid);
                }
                if !one_of_several(&self.live_threads) {
                    self.map_io().flush_all_shared(m);
                    return Some(SyscallOutcome::Exit(0));
                }
                let end = ThreadEnd {
                    thread: self.signals.thread(),
                    threads: &self.thread_signals,
                    thread_fs: &self.thread_fs,
                    futexes: &self.futexes,
                    procs: &self.procs,
                };
                end.end(m, self.ctx_addr, self.tid);
                self.thread_calls.remove(&self.tid);
                Some(SyscallOutcome::ThreadExit)
            }
            _ => None,
        }
    }

    /// `bsdthread_create(func, arg, stack, pthread, flags)`: a thread whose
    /// context is libpthread's start routine called with
    /// `(pthread, port, func, arg, stack, flags)` on `stack`.
    fn darwin_bsdthread_create(&mut self, m: &mut GuestMem<'_>, x: [u64; 8]) -> Result<SpawnRequest, Errno> {
        let (func, arg, stack, pthread, flags) = (x[0], x[1], x[2], x[3], x[4]);
        let reg = *self.darwin_threads.lock().unwrap_or_else(|e| e.into_inner());
        if !reg.registered || flags & PTHREAD_START_CUSTOM == 0 {
            return Err(Errno::EINVAL);
        }
        let fs = self.fs_of(self.ctx_addr);
        let how = clone_flags::VM | clone_flags::THREAD | clone_flags::FS;
        let req = self.thread_maker(&fs).make(m, self.ctx_addr, [how, stack, 0, 0, 0, 0])?;
        let child = req.ctx_addr;
        let mut flags = flags & !PTHREAD_START_SUSPENDED;
        if reg.tsd_offset != 0 {
            flags |= PTHREAD_START_TSD_BASE_SET;
            m.write_u64(child + ctx::TPIDRRO as u64, pthread + reg.tsd_offset)?;
        } else {
            m.write_u64(child + ctx::TPIDRRO as u64, 0)?;
        }
        let port = super::mach::thread_port(self.pid, req.tid) as u64;
        // The argument registers: x0-x5, or rdi, rsi, rdx, rcx, r8, r9.
        let regs: [usize; 6] = if gex_abi::X86 { [ctx::x(7), ctx::x(6), ctx::x(2), ctx::x(1), ctx::x(8), ctx::x(9)] } else { [ctx::x(0), ctx::x(1), ctx::x(2), ctx::x(3), ctx::x(4), ctx::x(5)] };
        for (slot, v) in regs.into_iter().zip([pthread, port, func, arg, stack, flags]) {
            m.write_u64(child + slot as u64, v)?;
        }
        if !gex_abi::X86 {
            // No caller to return to.
            m.write_u64(child + ctx::x(29) as u64, 0)?;
            m.write_u64(child + ctx::x(30) as u64, 0)?;
        }
        m.write_u64(child + ctx::PC as u64, reg.threadstart)?;
        Ok(req)
    }

    /// `ulock_wait(operation, addr, value, timeout)` as the futex wait it
    /// is: the canonical call's arguments in `a`. `timeout` is in
    /// microseconds, or nanoseconds for `ulock_wait2`; none is for ever.
    pub(super) fn darwin_ulock_wait(&mut self, m: &mut GuestMem<'_>, x: [u64; 8], nanos: bool, a: &mut [u64; 6]) -> u64 {
        let op = x[0] as u32 & 0xff;
        let wide = matches!(op, UL_UNFAIR_LOCK64_SHARED | UL_COMPARE_AND_WAIT64 | UL_COMPARE_AND_WAIT64_SHARED);
        if !matches!(op, UL_COMPARE_AND_WAIT | UL_UNFAIR_LOCK | UL_COMPARE_AND_WAIT_SHARED) && !wide {
            return NATIVE + d::ULOCK_WAIT;
        }
        // A 64-bit word whose high half has already changed is not waited
        // on; the futex compares the low half.
        if wide && m.read_u32(x[1] + 4).is_ok_and(|hi| hi != (x[2] >> 32) as u32) {
            return nr::NOOP_OK;
        }
        let timeout = if nanos { x[3] } else { (x[3] & 0xffff_ffff) * 1000 };
        let mut ts = 0;
        if timeout != 0 {
            ts = self.ctx_addr + SCRATCH_SLOT as u64;
            let _ = m.write_u64(ts, timeout / 1_000_000_000);
            let _ = m.write_u64(ts + 8, timeout % 1_000_000_000);
        }
        self.darwin_post = Post::UlockWait;
        *a = [x[1], crate::futex::op::WAIT as u64, x[2] & 0xffff_ffff, ts, 0, 0];
        nr::FUTEX
    }

    /// `ulock_wake(operation, addr, wake_value)`: one waiter, or all.
    pub(super) fn darwin_ulock_wake(&mut self, x: [u64; 8], a: &mut [u64; 6]) -> u64 {
        let all = x[0] as u32 & ULF_WAKE_ALL != 0;
        self.darwin_post = Post::UlockWake;
        *a = [x[1], crate::futex::op::WAKE as u64, if all { i32::MAX as u64 } else { 1 }, 0, 0, 0];
        nr::FUTEX
    }
}
