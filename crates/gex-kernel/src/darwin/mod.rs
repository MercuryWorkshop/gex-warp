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
pub mod bottle;
mod attr;
mod reclaim;
pub mod restart;
pub mod root;
mod bsd;
pub mod info;
pub mod kqueue;
mod mach;
pub mod nr;
pub mod port;
pub mod psem;
pub mod psynch;
mod sock;
mod spawn;
pub mod thread;
pub mod tty;
pub mod workq;

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

/// The commpage: a page of facts about the machine that XNU maps into
/// every process, and that libSystem reads instead of asking (the page
/// size, the processors, what the processor can do).
///
/// XNU's is at `0xFFFFFC000` on arm64, with a second, read-only page two
/// below it at `0xFFFFF4000`, and at `0x7FFFFFE00000` on x86-64: past the
/// end of any memory a guest has. So it is here, just under where a program
/// is loaded (arm64's two pages the same distance apart); and a userland
/// built for gex is built with these addresses where Apple's headers have
/// XNU's (`_COMM_PAGE64_BASE_ADDRESS`, `_COMM_PAGE64_RO_ADDRESS`). A
/// library Apple built reads XNU's by constant, and in a process with the
/// Darwin window the translators fold that constant into this page
/// (`gex_abi::fold_commpage`).
///
/// `TEXT` is a page of code the kernel keeps for a task, between the two:
/// where a `posix_spawn`'s copy is sent back into the kernel from
/// (`spawn`). Made at a process's first `posix_spawn`.
pub mod commpage {
    pub use gex_abi::commpage::{DATA, PAGE, RO, TEXT};
    /// `_COMM_PAGE_THIS_VERSION`.
    pub const VERSION: u16 = 3;
    /// `_cpu_capabilities`: a 128-byte cache line, the thread pointer in a
    /// register user code can read, Advanced SIMD, VFP, fused multiply-add,
    /// the atomics of ARMv8.1, CRC32, LDAPR, synchronous floating-point
    /// exceptions; and the processor count in bits 16 to 23.
    pub const CAPABILITIES: u64 = 0x40 | 0x80 | 0x100 | 0x400 | 0x2000 | 0x0200_0000 | 0x0400_0000 | 1 << 48 | 1 << 58;
    pub const UP: u64 = 0x8000;
    /// x86-64's version, and its capabilities: MMX, SSE, SSE2, SSE3, a
    /// 64-byte cache line, the thread pointer in a segment register,
    /// SSSE3, 64 bits (what gex's `cpuid` says the processor has).
    pub const VERSION_X86: u16 = 14;
    pub const CAPABILITIES_X86: u64 = 0x1 | 0x2 | 0x4 | 0x8 | 0x20 | 0x80 | 0x100 | 0x200;
    /// The processor family an x86-64 page names, whatever Mac the process
    /// is told it is on: Westmere (CPUFAMILY_INTEL_WESTMERE, as Rosetta
    /// says), the newest whose code gex runs. libplatform chooses its
    /// `memmove`, `bzero` and the rest by this word alone, and a later
    /// family's are AVX, which gex's processor does not have (`cpuid`).
    pub const CPUFAMILY_X86: u32 = 0x573b_5eec;
    /// How many times a lock is tried before its waiter sleeps
    /// (`_COMM_PAGE_SPIN_COUNT`, XNU's `MP_SPIN_TRIES`).
    pub const SPIN_COUNT: u32 = 1000;
}

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
    /// `unlink`: a directory is refused as Darwin refuses one.
    Unlink,
    /// The call only waited to be made again ([`restart`]).
    Again,
    /// `fstat` of shared memory: the `stat` at `stat` says no kind of
    /// file.
    NoKind { stat: u64 },
    /// A Mach trap that waited: its answer as a `kern_return_t`.
    Kern { timed_out: u32 },
    /// A Mach trap that waited on a word for something to come: told to
    /// ask again (`woken`), or that its time is up.
    Wake { woken: u32, timed_out: u32 },
    /// `__semwait_signal` likewise, whose answers are error numbers.
    SemWait,
    /// A psynch call that waited ([`psynch`]): finished by the kernel
    /// ([`Kernel::darwin_psynch_end`]), or, by a host that finishes its
    /// waits without one, made again.
    Psynch,
    /// `getsockopt(SO_ERROR)`: the error at `at` is one of Linux's.
    SoError { at: u64 },
    /// `statfs64`, once the file is known to be there: Darwin's structure
    /// at `at`, or (`old`) the one from before 64-bit inodes.
    Statfs { at: u64, old: bool },
    /// `stat` under its first number: the structure at `at` as it was
    /// before 64-bit inodes, which is shorter; `was` is what lay past it.
    OldStat { at: u64, was: [u64; 3] },
    /// An extended-attribute call: no such attribute is `ENOATTR`.
    Xattr,
    /// A `kevent` that waited on its queue ([`kqueue`]): to be made again,
    /// by the kernel ([`Kernel::finish_blocking`]) or by a host that
    /// finishes its waits without one. `slot` is the thread's, whose mark
    /// says the call is one being made again.
    Kevent { slot: u64 },
    /// A work queue's thread that waited for work ([`workq`]): it asks
    /// again, however its wait ended.
    Workq,
    /// `waitid`: the kernel's `siginfo_t` at `at` as Darwin's, which is
    /// shorter; `was` is what the caller had there, and past it.
    Waitid { at: u64, was: [u64; 16] },
    /// `sigwait`: the signal that came, by Darwin's number, at `at`; the
    /// call answers zero.
    Sigwait { at: u64 },
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
        if !self.darwin_commpage {
            self.darwin_commpage = true;
            self.darwin_map_commpage(m);
        }
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
            Class::Mach(trap) => match self.darwin_wait_trap(m, trap, x, a) {
                Some(Ok(call)) => call,
                Some(Err(ready)) => {
                    self.darwin_ready = Some(ready);
                    MACH + trap
                }
                None => MACH + trap,
            },
            Class::Mdep(code) => MDEP + code,
            Class::Invalid => NATIVE,
        };
        if is_own(n) {
            a.copy_from_slice(&x[..6]);
        }
        n
    }

    /// Map the [`commpage`] and fill it in: at a process's first call, which
    /// comes before anything of libSystem's reads it.
    fn darwin_map_commpage(&mut self, m: &mut GuestMem<'_>) {
        // The processors and their family, as `sysctl` has them: the Mac's
        // this process is told it is on ([`info::Mac`]).
        let machine = self.darwin_machine();
        let cpus = machine.cpus.clamp(1, 255) as u8;
        if gex_abi::X86 {
            return self.darwin_map_commpage_x86(m, cpus);
        }
        let caps = commpage::CAPABILITIES | (cpus as u64) << 16 | if cpus == 1 { commpage::UP } else { 0 };
        let mut page = [0u8; 0x100];
        page[..16].copy_from_slice(b"commpage 64-bit\0");
        page[0x10..0x18].copy_from_slice(&caps.to_le_bytes());
        page[0x1e..0x20].copy_from_slice(&commpage::VERSION.to_le_bytes());
        page[0x20..0x24].copy_from_slice(&(caps as u32).to_le_bytes());
        // The processor count; the page shifts of 32- and 64-bit processes;
        // the cache line; one cluster; the counts again; the kernel's page.
        page[0x22] = cpus;
        page[0x24] = 14;
        page[0x25] = 14;
        page[0x26..0x28].copy_from_slice(&128u16.to_le_bytes());
        page[0x2f] = 1;
        page[0x34..0x37].fill(cpus);
        page[0x37] = 14;
        // The memory's size and the processor's family, as `sysctl` has them.
        page[0x38..0x40].copy_from_slice(&(4u64 << 30).to_le_bytes());
        page[0x80..0x84].copy_from_slice(&machine.cpufamily.to_le_bytes());
        // 0x40, the time of day, and 0x90, whether the clock can be read
        // without the kernel, are zero: libSystem asks.
        for at in [commpage::RO, commpage::DATA] {
            if self.mem.mmap_anon(m, at, commpage::PAGE, 3, mflags::FIXED_NOREPLACE, false, "[commpage]").is_ok() {
                let _ = m.write(at, &page);
            }
        }
    }

    /// x86-64's page (`osfmk/i386/cpu_capabilities.h`, as of 10.14's
    /// xnu-4903): the same facts at other offsets, and the clock. There
    /// `mach_absolute_time` does not ask the kernel: it reads the
    /// processor's counter and scales it by what the page says,
    /// `((tsc - base) << shift) * scale >> 32 + ns`, while a generation
    /// there is not zero. gex's counter counts nanoseconds, so the scale is
    /// one (a half, shifted once), and the base is where the kernel's own
    /// clock was zero. `gettimeofday` too reads the time of day from the
    /// page (`_COMM_PAGE_NEWTIMEOFDAY_DATA`): the time when the page was
    /// made and the absolute time then, good for a second after it (past
    /// that, libSystem asks the kernel).
    fn darwin_map_commpage_x86(&mut self, m: &mut GuestMem<'_>, cpus: u8) {
        let caps = commpage::CAPABILITIES_X86 | (cpus as u64) << 16 | if cpus == 1 { commpage::UP } else { 0 };
        let mut page = [0u8; 0x100];
        page[..16].copy_from_slice(b"commpage 64-bit\0");
        page[0x10..0x18].copy_from_slice(&caps.to_le_bytes());
        page[0x1e..0x20].copy_from_slice(&commpage::VERSION_X86.to_le_bytes());
        page[0x20..0x24].copy_from_slice(&(caps as u32).to_le_bytes());
        page[0x22] = cpus;
        page[0x26..0x28].copy_from_slice(&64u16.to_le_bytes());
        page[0x30..0x34].copy_from_slice(&commpage::SPIN_COUNT.to_le_bytes());
        // The active, physical and logical processors.
        page[0x34..0x37].fill(cpus);
        page[0x37] = 1;
        page[0x38..0x40].copy_from_slice(&(4u64 << 30).to_le_bytes());
        page[0x40..0x44].copy_from_slice(&commpage::CPUFAMILY_X86.to_le_bytes());
        // The kernel's and a process's page shift.
        page[0x4d] = 12;
        page[0x4e] = 12;
        // The counter is the wall clock in nanoseconds; the kernel's clock
        // is that less where it began.
        let (secs, nanos) = (self.clock)();
        let now = clock_ns(self.clock, 1);
        let base = (secs * 1_000_000_000 + nanos as u64).wrapping_sub(now);
        page[0x50..0x58].copy_from_slice(&base.to_le_bytes());
        page[0x58..0x5c].copy_from_slice(&0x8000_0000u32.to_le_bytes());
        page[0x5c..0x60].copy_from_slice(&1u32.to_le_bytes());
        page[0x68..0x6c].copy_from_slice(&1u32.to_le_bytes());
        // 0x6c, the old time of day's generation, is zero: what reads it
        // asks the kernel. 0x80, the approximate time, is not kept
        // (0x88), and the continuous time is the absolute time (0xc0).
        // When the machine started, in microseconds of the epoch.
        let boot = (secs * 1_000_000 + nanos as u64 / 1000).saturating_sub(now / 1000);
        page[0xc8..0xd0].copy_from_slice(&boot.to_le_bytes());
        // The time of day (`new_commpage_timeofday_data_t`): the absolute
        // time it was taken at, its seconds and fraction (of 2^64), a
        // tick's worth of fraction, and the ticks in a second.
        let frac = (((nanos as u128) << 64) / 1_000_000_000) as u64;
        let per_tick = ((1u128 << 64) / 1_000_000_000) as u64;
        for (i, v) in [now.max(1), secs, frac, per_tick, 1_000_000_000].into_iter().enumerate() {
            page[0xd0 + 8 * i..0xd8 + 8 * i].copy_from_slice(&v.to_le_bytes());
        }
        if self.mem.mmap_anon(m, commpage::DATA, commpage::PAGE, 3, mflags::FIXED_NOREPLACE, false, "[commpage]").is_ok() {
            let _ = m.write(commpage::DATA, &page);
        }
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
        let r = if post == Post::Psynch { self.darwin_psynch_end(m, r) } else { r };
        if matches!(post, Post::Kevent { .. }) {
            self.darwin_kevents.remove(&self.tid);
        }
        finish(m, self.ctx_addr, post, r)
    }
}

/// Apply what a rewritten call left to do (`post`) to its answer `r`, for
/// the thread whose context is at `ctx_addr`: [`Kernel::finish_blocking`]'s
/// part, for a host that finishes its waits without the kernel (the page
/// takes `Kernel::darwin_post` with the rest of what a parked call owes).
pub fn finish(m: &mut GuestMem<'_>, ctx_addr: u64, post: Post, r: SysResult) -> SysResult {
    // The exclusive monitor does not survive a system call, and its slots
    // may have been this call's scratch.
    if !gex_abi::X86 {
        let _ = m.write_u64(ctx_addr + ctx::MON_SIZE as u64, 0);
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
            let at = ctx_addr + SCRATCH_SLOT as u64;
            r.and_then(|_| {
                let (rd, wr) = (m.read_u32(at)?, m.read_u32(at + 4)?);
                m.write_u64(ctx_addr + RET2_SLOT as u64, wr as u64)?;
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
        // `unlink` of a directory is EPERM on Darwin (Linux says EISDIR).
        Post::Unlink => match r {
            Err(Errno::EISDIR) => Err(Errno::EPERM),
            r => r,
        },
        // (`st_mode` is the sixteen bits after `st_dev`.)
        Post::NoKind { stat } => r.and_then(|v| {
            let mode = m.read(stat + 4, 2).map(|b| u16::from_le_bytes([b[0], b[1]]))?;
            m.write(stat + 4, &(mode & 0o7777).to_le_bytes())?;
            Ok(v)
        }),
        Post::Wake { woken, timed_out } => Ok(match r {
            Err(Errno::ETIMEDOUT) => timed_out as u64,
            _ => woken as u64,
        }),
        Post::SemWait => match r {
            Ok(_) | Err(Errno::EAGAIN) => Err(Errno::EINTR),
            e => e,
        },
        // (The kernel's answer already, or a host that makes the call
        // again and does not come here.)
        Post::Psynch | Post::Workq | Post::Again => r,
        Post::SoError { at } => {
            if r.is_ok() {
                if let Ok(e @ 1..) = m.read_u32(at) {
                    let _ = m.write_u32(at, abi::errno_to_darwin(Errno(e as i32)));
                }
            }
            r
        }
        Post::Statfs { at, old } => {
            if r.is_ok() {
                crate::syscalls::statfs(m, false, at)?;
                let k = m.read(at, 120)?.to_vec();
                m.write(at, &if old { abi::statfs_old(&k) } else { abi::statfs(&k) })?;
            }
            r.map(|_| 0)
        }
        Post::OldStat { at, was } => {
            if r.is_ok() {
                let d = m.read(at, 144)?.to_vec();
                m.write(at, &abi::stat_old(&d))?;
            }
            for (i, w) in was.iter().enumerate() {
                let _ = m.write_u64(at + abi::STAT_OLD as u64 + 8 * i as u64, *w);
            }
            r
        }
        Post::Xattr => match r {
            Err(Errno::ENODATA) => Err(abi::K_ENOATTR),
            r => r,
        },
        // Not made again (a signal ended the wait, or the host could not):
        // the thread's next `kevent` is a new one.
        Post::Kevent { slot } => {
            let _ = m.write_u32(slot + 16, 0);
            r.and(Err(Errno::EINTR))
        }
        Post::Waitid { at, was } => {
            if at != 0 {
                let l = m.read(at, 128)?.to_vec();
                for (i, w) in was.iter().enumerate() {
                    let _ = m.write_u64(at + 8 * i as u64, *w);
                }
                // With nobody to report (WNOHANG), the kernel's is zeroes
                // and XNU leaves the caller's as it was.
                if r.is_ok() && l[..4] != [0; 4] {
                    m.write(at, &abi::siginfo_child(&l))?;
                }
            }
            r.map(|_| 0)
        }
        Post::Sigwait { at } => r.and_then(|signo| {
            if at != 0 {
                m.write_u32(at, abi::sig_to_darwin(signo as u32))?;
            }
            Ok(0)
        }),
        Post::Kern { timed_out } => Ok(match r {
            Ok(_) => mach::KERN_SUCCESS,
            Err(Errno::ETIMEDOUT) => timed_out as u64,
            Err(Errno::EINTR) => mach::KERN_ABORTED,
            Err(_) => mach::KERN_FAILURE,
        }),
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
    // (A `posix_spawn`'s copy makes its call again, with the arguments it
    // was made with: no second result is written over one of them.)
    if spawn::send_back(m, ctx_addr)? {
        return Ok(());
    }
    m.write_u64(ctx_addr + RET2_SLOT as u64, 1)
}

/// The context of a new process's first thread, beyond what every guest
/// gets: nothing, today. The thread pointer starts at zero; libpthread sets
/// it when it has made the main thread's structure.
pub fn init_context(_m: &mut GuestMem<'_>, _ctx_addr: u64) {}
