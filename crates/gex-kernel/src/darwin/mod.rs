//! The Darwin personality: XNU's system-call ABI, on the same kernel.
//!
//! A Mach-O process enters the kernel as it would enter XNU. On arm64 that
//! is `svc #0x80` with the call in `x16`: a positive number is a BSD system
//! call (`bsd/kern/syscalls.master`), a negative one a Mach trap
//! (`osfmk/kern/syscall_sw.c`), and `0x80000000` a machine-dependent call
//! whose selector is in `x3`. On x86-64 it is `syscall` with the class in
//! the top byte of `rax`. A BSD call answers in `x0` with the carry flag
//! clear, or with the carry flag set and a positive error number in `x0`;
//! a Mach trap answers with a `kern_return_t`.
//!
//! The kernel underneath is the one Linux programs run on, so most of this
//! is translation, as [`crate::i386`] is for the i386 ABI: a BSD call is
//! rewritten into the canonical call that does the same work, with its
//! flags and numbers respelled ([`abi`]), and what it leaves in memory in a
//! layout of its own is converted when it returns ([`Post`]). What only
//! Darwin has -- Mach traps, `sysctl`, the code-signing and process-info
//! calls its libraries make at start -- is answered here ([`Kernel::darwin_call`]),
//! under canonical numbers of its own ([`NATIVE`], [`MACH`], [`MDEP`]).

pub mod abi;
mod bsd;
mod mach;
pub mod nr;
pub mod thread;

use super::*;

/// Whether this host process's guests are Darwin programs. One setting per
/// host process, like [`crate::set_compat32`]: the native runner keeps one
/// translator for every guest process, and the browser has a shim per
/// process.
static ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set(on: bool) {
    ON.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn on() -> bool {
    ON.load(std::sync::atomic::Ordering::Relaxed)
}

/// Canonical numbers for what only Darwin has, above anything Linux or the
/// x86 legacy calls use: a BSD call this module answers itself is
/// `NATIVE + number`, a Mach trap `MACH + number`, a machine-dependent call
/// `MDEP + selector`.
pub const NATIVE: u64 = 0x1_0000;
pub const MACH: u64 = 0x2_0000;
pub const MDEP: u64 = 0x3_0000;
/// One past the last of them.
pub const END: u64 = 0x4_0000;

/// The first Mach port name a task allocates.
pub const FIRST_PORT_NAME: u32 = mach::port::FIRST_ALLOCATED;

/// Whether `n` is one of this module's canonical numbers.
pub fn is_own(n: u64) -> bool {
    (NATIVE..END).contains(&n)
}

/// The name of one of this module's canonical numbers, for the trace.
pub fn name(n: u64) -> Option<&'static str> {
    match n {
        n if (NATIVE..MACH).contains(&n) => nr::name(n - NATIVE),
        n if (MACH..MDEP).contains(&n) => nr::trap::name(n - MACH).or(match n - MACH {
            3 => Some("mach_absolute_time"),
            4 => Some("mach_continuous_time"),
            _ => None,
        }),
        n if n == MDEP + 2 => Some("thread_set_cthread_self"),
        n if n == MDEP + 3 => Some(if gex_abi::X86 { "thread_fast_set_cthread_self64" } else { "thread_get_cthread_self" }),
        _ => None,
    }
}

/// Where the call's number is: `x16`, or `rax`.
pub const NR_SLOT: usize = if gex_abi::X86 { ctx::x(0) } else { ctx::x(16) };

/// Where a call's second result goes (`fork`'s child flag, `pipe`'s second
/// descriptor): `x1`, or `rdx`.
const RET2_SLOT: usize = if gex_abi::X86 { ctx::x(2) } else { ctx::x(1) };

/// Sixteen bytes of the thread's context the kernel may build a structure
/// in for a rewritten call. On x86-64 the context has a scratch area; on
/// arm64 they are the exclusive monitor's address and value, which an
/// exception clears on hardware too (the monitor is left clear).
const SCRATCH_SLOT: usize = if gex_abi::X86 { ctx::SCRATCH } else { ctx::MON_ADDR };

/// What remains to be done once a rewritten call has its answer, kept with
/// the thread while the call waits ([`Kernel::darwin_finish`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Post {
    #[default]
    None,
    /// `wait4`: the status word at `status` names signals by Linux's
    /// numbers.
    WaitStatus { status: u64 },
    /// The call answers zero, whatever its canonical form counted.
    Zero,
    /// `pipe`: the two descriptors `pipe2` wrote to the scratch are the
    /// call's two results.
    Pipe,
    /// The four words at `at` were respelled for the call and are the
    /// caller's again.
    Restore { at: u64, was: [u64; 4], words: u8 },
    /// `getrlimit`: the two limits at `at`, Linux's infinity as Darwin's.
    Rlimit { at: u64 },
    /// `ulock_wait`: a word that had already changed is no error, and a
    /// wake answers zero.
    UlockWait,
    /// `ulock_wake`: waking nobody is `ENOENT`.
    UlockWake,
    /// A Mach trap that waited: its answer as a `kern_return_t`.
    Kern { timed_out: u32 },
}

/// The call's class.
enum Class {
    Unix(u64),
    Mach(u64),
    Mdep(u64),
    Invalid,
}

impl Kernel {
    /// The arguments of the call the thread is making, as XNU reads them:
    /// `x0` to `x7`, or `rdi, rsi, rdx, r10, r8, r9` and two from the stack.
    fn darwin_args(&self, m: &GuestMem<'_>) -> [u64; 8] {
        let mut x = [0u64; 8];
        if gex_abi::X86 {
            for (i, slot) in ctx::SYSCALL_ARGS.iter().enumerate() {
                x[i] = m.read_u64(self.ctx_addr + *slot as u64).unwrap_or(0);
            }
            // Past the return address the `syscall` stub was called with.
            let sp = m.read_u64(self.ctx_addr + ctx::SP as u64).unwrap_or(0);
            x[6] = m.read_u64(sp + 8).unwrap_or(0);
            x[7] = m.read_u64(sp + 16).unwrap_or(0);
        } else {
            for (i, v) in x.iter_mut().enumerate() {
                *v = m.read_u64(self.ctx_addr + ctx::x(i as u8) as u64).unwrap_or(0);
            }
        }
        x
    }

    /// The call a Darwin thread is making, as the canonical call that does
    /// its work: the number, with `a` its arguments. `raw` is the number
    /// register.
    pub(super) fn darwin_translate(&mut self, m: &mut GuestMem<'_>, raw: u64, a: &mut [u64; 6]) -> u64 {
        self.darwin_post = Post::None;
        let mut x = self.darwin_args(m);
        let class = if gex_abi::X86 {
            match (raw >> 24) & 0xff {
                1 => Class::Mach(raw & 0xff_ffff),
                2 => Class::Unix(raw & 0xff_ffff),
                3 => Class::Mdep(raw & 0xff_ffff),
                _ => Class::Invalid,
            }
        } else {
            let w = raw as u32;
            if w == 0x8000_0000 {
                Class::Mdep(x[3] & 0xffff_ffff)
            } else if (w as i32) < 0 {
                Class::Mach((w as i32).unsigned_abs() as u64)
            } else {
                Class::Unix((w & 0xffff) as u64)
            }
        };
        // `syscall(number, ...)`: the call is the first argument's.
        let class = match class {
            Class::Unix(0) => {
                let code = x[0] & 0xffff;
                x.copy_within(1.., 0);
                x[7] = 0;
                Class::Unix(code)
            }
            c => c,
        };
        self.darwin_x = x;
        let n = match class {
            Class::Unix(code) => self.darwin_bsd(m, code, x, a),
            Class::Mach(trap) => MACH + trap,
            Class::Mdep(code) => MDEP + code,
            Class::Invalid => NATIVE,
        };
        if is_own(n) {
            a.copy_from_slice(&x[..6]);
        }
        n
    }

    /// One of this module's own calls ([`NATIVE`], [`MACH`], [`MDEP`]).
    pub(super) fn darwin_call(&mut self, m: &mut GuestMem<'_>, n: u64) -> SysResult {
        let x = self.darwin_x;
        match n {
            n if (MACH..MDEP).contains(&n) => self.darwin_mach(m, n - MACH, x),
            n if n >= MDEP => self.darwin_mdep(m, n - MDEP, x),
            n => self.darwin_native(m, n - NATIVE, x),
        }
    }

    /// A machine-dependent call: the thread's own pointer, which its
    /// thread-specific data hangs off (`TPIDRRO_EL0`, or the `gs` base).
    fn darwin_mdep(&mut self, m: &mut GuestMem<'_>, code: u64, x: [u64; 8]) -> SysResult {
        let tsd = self.ctx_addr + ctx::TPIDRRO as u64;
        match (gex_abi::X86, code) {
            // thread_set_cthread_self; `x0` is left as it was.
            (false, 2) => {
                m.write_u64(tsd, x[0])?;
                Ok(x[0])
            }
            (false, 3) => m.read_u64(tsd),
            // thread_fast_set_cthread_self64
            (true, 3) => {
                m.write_u64(tsd, x[0])?;
                Ok(0)
            }
            // The cache flushes arm64 no longer has, and anything unknown:
            // nothing, as XNU does.
            _ => Ok(x[0]),
        }
    }

    /// Apply what a rewritten call left to do ([`Post`]) to its answer.
    pub(super) fn darwin_finish(&mut self, m: &mut GuestMem<'_>, r: SysResult) -> SysResult {
        let post = std::mem::take(&mut self.darwin_post);
        // The exclusive monitor does not survive a system call, and its
        // slots may have been this call's scratch.
        if !gex_abi::X86 {
            let _ = m.write_u64(self.ctx_addr + ctx::MON_SIZE as u64, 0);
        }
        match post {
            Post::None => r,
            Post::Zero => r.map(|_| 0),
            Post::WaitStatus { status } => {
                if r.is_ok() && status != 0 {
                    if let Ok(w) = m.read_u32(status) {
                        let _ = m.write_u32(status, abi::wait_status_to_darwin(w));
                    }
                }
                r
            }
            Post::Pipe => {
                let at = self.ctx_addr + SCRATCH_SLOT as u64;
                r.and_then(|_| {
                    let (rd, wr) = (m.read_u32(at)?, m.read_u32(at + 4)?);
                    m.write_u64(self.ctx_addr + RET2_SLOT as u64, wr as u64)?;
                    Ok(rd as u64)
                })
            }
            Post::Restore { at, was, words } => {
                for (i, w) in was.iter().take(words as usize).enumerate() {
                    let _ = m.write_u64(at + 8 * i as u64, *w);
                }
                r
            }
            Post::Rlimit { at } => {
                if r.is_ok() {
                    for o in [0, 8] {
                        if m.read_u64(at + o) == Ok(RLIM_INFINITY) {
                            let _ = m.write_u64(at + o, abi::RLIM_INFINITY);
                        }
                    }
                }
                r
            }
            Post::UlockWait => match r {
                Ok(_) | Err(Errno::EAGAIN) => Ok(0),
                e => e,
            },
            Post::UlockWake => match r {
                Ok(0) => Err(Errno::ENOENT),
                Ok(_) => Ok(0),
                e => e,
            },
            Post::Kern { timed_out } => Ok(match r {
                Ok(_) => mach::KERN_SUCCESS,
                Err(Errno::ETIMEDOUT) => timed_out as u64,
                Err(Errno::EINTR) => mach::KERN_ABORTED,
                Err(_) => mach::KERN_FAILURE,
            }),
        }
    }
}

/// Hand a system call's answer back to a Darwin thread. A BSD call's is
/// the value with the carry flag clear, or a positive Darwin error number
/// with it set, as XNU's `arm_prepare_syscall_return` leaves them. A Mach
/// trap's or a machine-dependent call's is the value alone: no errors, and
/// the flags are not touched. `ret` is the kernel's own convention
/// (`-errno`). The call is the one the context's number register still
/// names.
pub fn set_result(m: &mut GuestMem<'_>, ctx_addr: u64, ret: u64) -> Result<(), Errno> {
    let raw = m.read_u64(ctx_addr + ctx::LAST_SYSCALL as u64)?;
    let unix = if gex_abi::X86 { (raw >> 24) & 0xff == 2 } else { (raw as u32 as i32) >= 0 };
    if !unix {
        return m.write_u64(ctx_addr + ctx::x(0) as u64, ret);
    }
    let (value, carry) = match ret as i64 {
        e @ -4095..=-1 => {
            let number = abi::errno_to_darwin(Errno(-e as i32)) as u64;
            // `ULF_NO_ERRNO`: a `ulock` call that wants its error as a
            // negative answer, not through the carry flag and `errno`.
            if ulock_no_errno(m, ctx_addr, raw) {
                (number.wrapping_neg(), 0u8)
            } else {
                (number, 1u8)
            }
        }
        _ => (ret, 0),
    };
    m.write_u64(ctx_addr + ctx::x(0) as u64, value)?;
    m.write(ctx_addr + ctx::CF as u64, &[carry])
}

/// Whether the BSD call `raw` names, its arguments still in the context, is
/// a `ulock` call made with `ULF_NO_ERRNO`.
fn ulock_no_errno(m: &GuestMem<'_>, ctx_addr: u64, raw: u64) -> bool {
    let args = ctx::SYSCALL_ARGS;
    let arg = |i: usize| m.read_u64(ctx_addr + args[i] as u64).unwrap_or(0);
    // `syscall(number, ...)`: the call and its arguments are one along.
    let (code, op) = match raw & if gex_abi::X86 { 0xff_ffff } else { 0xffff } {
        0 => (arg(0) & 0xffff, arg(1)),
        c => (c, arg(0)),
    };
    matches!(code, nr::ULOCK_WAIT | nr::ULOCK_WAKE | nr::ULOCK_WAIT2) && op as u32 & thread::ULF_NO_ERRNO != 0
}

/// A forked child's second result: one (its parent's is zero), which is
/// how libsyscall's `fork` tells the two apart.
pub fn set_fork_child(m: &mut GuestMem<'_>, ctx_addr: u64) -> Result<(), Errno> {
    m.write_u64(ctx_addr + RET2_SLOT as u64, 1)
}

/// The context of a new process's first thread, beyond what every guest
/// gets: nothing, today. The thread pointer starts at zero; libpthread sets
/// it when it has made the main thread's structure.
pub fn init_context(_m: &mut GuestMem<'_>, _ctx_addr: u64) {}
