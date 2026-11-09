//! Darwin's BSD system calls.
//!
//! [`Kernel::darwin_bsd`] rewrites a call into the canonical one that does
//! its work; [`Kernel::darwin_native`] answers the ones nothing canonical
//! does.

use super::abi::{self, at_flags, dirfd};
use super::nr as d;
use super::*;
use crate::syscalls::nr;

const CWD: u64 = (-100i64) as u64;
/// The kernel's `AT_SYMLINK_NOFOLLOW` and `AT_REMOVEDIR`.
const K_NOFOLLOW: u64 = 0x100;
const K_REMOVEDIR: u64 = 0x200;
const SIGCHLD: u64 = signal::sig::SIGCHLD as u64;

/// Darwin's `fcntl` commands.
mod f {
    pub const DUPFD: u64 = 0;
    pub const GETFD: u64 = 1;
    pub const SETFD: u64 = 2;
    pub const GETFL: u64 = 3;
    pub const SETFL: u64 = 4;
    pub const GETOWN: u64 = 5;
    pub const SETOWN: u64 = 6;
    pub const RDAHEAD: u64 = 45;
    pub const NOCACHE: u64 = 48;
    pub const GETPATH: u64 = 50;
    pub const FULLFSYNC: u64 = 51;
    pub const DUPFD_CLOEXEC: u64 = 67;
    pub const SETNOSIGPIPE: u64 = 73;
    pub const GETNOSIGPIPE: u64 = 74;
    pub const BARRIERFSYNC: u64 = 85;
    pub const GETPATH_NOFIRMLINK: u64 = 102;
}

/// Darwin's `sa_flags`.
mod sa {
    pub const ONSTACK: u64 = 0x1;
    pub const RESTART: u64 = 0x2;
    pub const RESETHAND: u64 = 0x4;
    pub const NOCLDSTOP: u64 = 0x8;
    pub const NODEFER: u64 = 0x10;
    pub const NOCLDWAIT: u64 = 0x20;
    pub const SIGINFO: u64 = 0x40;
}

/// Each `(darwin, kernel)` pair of `sa_flags`.
const SA_FLAGS: [(u64, u64); 7] = [
    (sa::ONSTACK, signal::sa::ONSTACK),
    (sa::RESTART, signal::sa::RESTART),
    (sa::RESETHAND, signal::sa::RESETHAND),
    (sa::NOCLDSTOP, signal::sa::NOCLDSTOP),
    (sa::NODEFER, signal::sa::NODEFER),
    (sa::NOCLDWAIT, signal::sa::NOCLDWAIT),
    (sa::SIGINFO, signal::sa::SIGINFO),
];

fn sa_flags_to_kernel(f: u64) -> u64 {
    SA_FLAGS.iter().filter(|(d, _)| f & d != 0).fold(0, |o, (_, k)| o | k)
}

fn sa_flags_to_darwin(f: u64) -> u64 {
    SA_FLAGS.iter().filter(|(_, k)| f & k != 0).fold(0, |o, (d, _)| o | d)
}

impl Kernel {
    /// BSD call `code` with arguments `x`, as a canonical call: its number,
    /// with `a` its arguments. A call with no canonical form is
    /// `NATIVE + code` ([`Kernel::darwin_native`]).
    pub(super) fn darwin_bsd(&mut self, m: &mut GuestMem<'_>, code: u64, x: [u64; 8], a: &mut [u64; 6]) -> u64 {
        let mut set = |n: u64, args: &[u64]| {
            *a = [0; 6];
            a[..args.len()].copy_from_slice(args);
            n
        };
        match code {
            d::EXIT => set(nr::EXIT_GROUP, &[x[0]]),
            // A new process: the parent's second result is zero, the
            // child's (which the host gives it) one.
            d::FORK | d::VFORK => {
                let _ = m.write_u64(self.ctx_addr + RET2_SLOT as u64, 0);
                set(nr::CLONE, &[SIGCHLD])
            }
            d::READ | d::READ_NOCANCEL => set(nr::READ, &x[..3]),
            d::WRITE | d::WRITE_NOCANCEL => set(nr::WRITE, &x[..3]),
            d::READV | d::READV_NOCANCEL => set(nr::READV, &x[..3]),
            d::WRITEV | d::WRITEV_NOCANCEL => set(nr::WRITEV, &x[..3]),
            d::PREAD | d::PREAD_NOCANCEL => set(nr::PREAD64, &x[..4]),
            d::PWRITE | d::PWRITE_NOCANCEL => set(nr::PWRITE64, &x[..4]),
            // The flags are respelled at the kernel's door
            // (`oflags::from_guest`).
            d::OPEN | d::OPEN_NOCANCEL => set(nr::OPENAT, &[CWD, x[0], x[1], x[2]]),
            d::OPENAT | d::OPENAT_NOCANCEL => set(nr::OPENAT, &[dirfd(x[0]), x[1], x[2], x[3]]),
            d::CLOSE | d::CLOSE_NOCANCEL => set(nr::CLOSE, &x[..1]),
            d::WAIT4 | d::WAIT4_NOCANCEL => {
                // WNOHANG and WUNTRACED are 1 and 2 in both; WCONTINUED is
                // 0x10 here and 8 there.
                let opts = (x[2] & 3) | if x[2] & 0x10 != 0 { 8 } else { 0 };
                self.darwin_post = Post::WaitStatus { status: x[1] };
                set(nr::WAIT4, &[x[0] as i32 as i64 as u64, x[1], opts, x[3]])
            }
            d::LINK => set(nr::LINKAT, &[CWD, x[0], CWD, x[1], 0]),
            d::LINKAT => set(nr::LINKAT, &[dirfd(x[0]), x[1], dirfd(x[2]), x[3], at_flags(x[4])]),
            d::UNLINK => set(nr::UNLINKAT, &[CWD, x[0], 0]),
            d::UNLINKAT => set(nr::UNLINKAT, &[dirfd(x[0]), x[1], at_flags(x[2])]),
            d::RMDIR => set(nr::UNLINKAT, &[CWD, x[0], K_REMOVEDIR]),
            d::MKDIR => set(nr::MKDIRAT, &[CWD, x[0], x[1]]),
            d::MKDIRAT => set(nr::MKDIRAT, &[dirfd(x[0]), x[1], x[2]]),
            d::RENAME => set(nr::RENAMEAT, &[CWD, x[0], CWD, x[1]]),
            d::RENAMEAT => set(nr::RENAMEAT, &[dirfd(x[0]), x[1], dirfd(x[2]), x[3]]),
            d::SYMLINK => set(nr::SYMLINKAT, &[x[0], CWD, x[1]]),
            d::SYMLINKAT => set(nr::SYMLINKAT, &[x[0], dirfd(x[1]), x[2]]),
            d::READLINK => set(nr::READLINKAT, &[CWD, x[0], x[1], x[2]]),
            d::READLINKAT => set(nr::READLINKAT, &[dirfd(x[0]), x[1], x[2], x[3]]),
            d::CHDIR => set(nr::CHDIR, &x[..1]),
            d::FCHDIR => set(nr::FCHDIR, &x[..1]),
            d::CHROOT => set(nr::CHROOT, &x[..1]),
            d::CHMOD => set(nr::FCHMODAT, &[CWD, x[0], x[1]]),
            d::FCHMODAT => set(nr::FCHMODAT, &[dirfd(x[0]), x[1], x[2]]),
            d::FCHMOD => set(nr::FCHMOD, &x[..2]),
            d::CHOWN => set(nr::FCHOWNAT, &[CWD, x[0], x[1], x[2], 0]),
            d::LCHOWN => set(nr::FCHOWNAT, &[CWD, x[0], x[1], x[2], K_NOFOLLOW]),
            d::FCHOWNAT => set(nr::FCHOWNAT, &[dirfd(x[0]), x[1], x[2], x[3], at_flags(x[4])]),
            d::FCHOWN => set(nr::FCHOWN, &x[..3]),
            d::ACCESS => set(nr::FACCESSAT, &[CWD, x[0], x[1]]),
            d::FACCESSAT => set(nr::FACCESSAT, &[dirfd(x[0]), x[1], x[2]]),
            d::UMASK => set(nr::UMASK, &x[..1]),
            d::SYNC => set(nr::SYNC, &[]),
            d::FSYNC | d::FSYNC_NOCANCEL => set(nr::FSYNC, &x[..1]),
            d::FDATASYNC => set(nr::FDATASYNC, &x[..1]),
            d::DUP => set(nr::DUP, &x[..1]),
            d::FLOCK => set(nr::FLOCK, &x[..2]),
            // The descriptors come back in the scratch and become the
            // call's two results.
            d::PIPE => {
                self.darwin_post = Post::Pipe;
                set(nr::PIPE2, &[self.ctx_addr + SCRATCH_SLOT as u64, 0])
            }
            // `SEEK_HOLE` and `SEEK_DATA` are 3 and 4 here, 4 and 3 there.
            d::LSEEK => {
                let whence = match x[2] {
                    3 => 4,
                    4 => 3,
                    w => w,
                };
                set(nr::LSEEK, &[x[0], x[1], whence])
            }
            d::TRUNCATE => set(nr::TRUNCATE, &x[..2]),
            d::FTRUNCATE => set(nr::FTRUNCATE, &x[..2]),
            // `struct stat` is Darwin's at the kernel's door (`Stat::encode_for`).
            d::STAT64 => set(nr::NEWFSTATAT, &[CWD, x[0], x[1], 0]),
            d::LSTAT64 => set(nr::NEWFSTATAT, &[CWD, x[0], x[1], K_NOFOLLOW]),
            d::FSTAT64 => set(nr::FSTAT, &x[..2]),
            d::FSTATAT64 => set(nr::NEWFSTATAT, &[dirfd(x[0]), x[1], x[2], at_flags(x[3])]),
            d::GETPID => set(nr::GETPID, &[]),
            d::GETPPID => set(nr::GETPPID, &[]),
            d::GETUID => set(nr::GETUID, &[]),
            d::GETEUID => set(nr::GETEUID, &[]),
            d::GETGID => set(nr::GETGID, &[]),
            d::GETEGID => set(nr::GETEGID, &[]),
            d::SETUID => set(nr::SETUID, &x[..1]),
            d::SETGID => set(nr::SETGID, &x[..1]),
            d::SETEUID => set(nr::SETRESUID, &[u64::MAX, x[0], u64::MAX]),
            d::SETEGID => set(nr::SETRESGID, &[u64::MAX, x[0], u64::MAX]),
            d::SETREUID => set(nr::SETREUID, &x[..2]),
            d::SETREGID => set(nr::SETREGID, &x[..2]),
            d::GETGROUPS => set(nr::GETGROUPS, &x[..2]),
            d::SETGROUPS => set(nr::SETGROUPS, &x[..2]),
            d::GETPGRP => set(nr::GETPGID, &[0]),
            d::GETPGID => set(nr::GETPGID, &x[..1]),
            d::SETPGID => set(nr::SETPGID, &x[..2]),
            d::GETSID => set(nr::GETSID, &x[..1]),
            d::SETSID => set(nr::SETSID, &[]),
            d::KILL => match x[1] as u32 {
                0 => set(nr::KILL, &[x[0] as i32 as i64 as u64, 0]),
                s => match abi::sig_to_linux(s) {
                    0 => NATIVE + d::KILL,
                    l => set(nr::KILL, &[x[0] as i32 as i64 as u64, l as u64]),
                },
            },
            // `sigsuspend`'s mask is an argument, not a pointer: the
            // kernel's 64-bit one goes in the scratch.
            d::SIGSUSPEND | d::SIGSUSPEND_NOCANCEL => {
                let at = self.ctx_addr + SCRATCH_SLOT as u64;
                let _ = m.write_u64(at, abi::sigset_to_linux(x[0] as u32));
                set(nr::RT_SIGSUSPEND, &[at, 8])
            }
            d::EXECVE => set(nr::EXECVE, &x[..3]),
            // A handler's return, which restores a whole context; or, by its
            // second argument, a note that the thread is on or off its
            // alternate stack, which is worked out from its stack pointer
            // here and needs no note.
            d::SIGRETURN => match x[1] as u32 {
                signal::dframe::UC_SET_ALT_STACK | signal::dframe::UC_RESET_ALT_STACK => set(nr::NOOP_OK, &[]),
                _ => set(nr::RT_SIGRETURN, &x[..3]),
            },
            d::MUNMAP => set(nr::MUNMAP, &x[..2]),
            d::MPROTECT => set(nr::MPROTECT, &x[..3]),
            d::MSYNC => set(nr::MSYNC, &x[..3]),
            // Advice the two spell alike is passed on up to `MADV_WILLNEED`.
            // Past it Darwin's is to do with reuse, and none of it lets the
            // kernel drop what the pages hold (Linux's `MADV_DONTNEED` does).
            d::MADVISE if x[2] <= 3 => set(nr::MADVISE, &x[..3]),
            d::MADVISE | d::MLOCK | d::MUNLOCK | d::MLOCKALL | d::MUNLOCKALL | d::MINCORE => set(nr::NOOP_OK, &[]),
            d::GETRUSAGE => set(nr::GETRUSAGE, &x[..2]),
            d::GETRLIMIT => match abi::rlimit(x[0]) {
                Some(which) => {
                    self.darwin_post = Post::Rlimit { at: x[1] };
                    set(nr::PRLIMIT64, &[0, which, 0, x[1]])
                }
                None => NATIVE + d::GETRLIMIT,
            },
            d::SETRLIMIT => match abi::rlimit(x[0]) {
                Some(which) => set(nr::PRLIMIT64, &[0, which, x[1], 0]),
                None => NATIVE + d::SETRLIMIT,
            },
            d::GETPRIORITY => set(nr::NOOP_OK, &[]),
            d::SETPRIORITY => set(nr::SETPRIORITY, &x[..3]),
            d::GETITIMER => set(nr::GETITIMER, &x[..2]),
            // A `struct timeval`'s microseconds are 32 bits and four of
            // padding here, 64 bits there: the padding is cleared.
            d::SETITIMER => {
                for at in [x[1] + 8, x[1] + 24] {
                    if let Ok(us) = m.read_u32(at) {
                        let _ = m.write_u64(at, us as u64);
                    }
                }
                set(nr::SETITIMER, &x[..3])
            }
            // The two `timeval`s are `timespec`s for the call, and the
            // caller's again after it.
            d::UTIMES | d::FUTIMES => {
                let times = x[1];
                if times != 0 {
                    let mut was = [0u64; 4];
                    for (i, w) in was.iter_mut().enumerate() {
                        *w = m.read_u64(times + 8 * i as u64).unwrap_or(0);
                    }
                    for at in [times + 8, times + 24] {
                        let us = m.read_u32(at).unwrap_or(0) as u64;
                        let _ = m.write_u64(at, us * 1000);
                    }
                    self.darwin_post = Post::Restore { at: times, was, words: 4 };
                }
                if code == d::UTIMES {
                    set(nr::UTIMENSAT, &[CWD, x[0], times, 0])
                } else {
                    set(nr::UTIMENSAT, &[x[0], 0, times, 0])
                }
            }
            // `select`'s `timeval` is `pselect6`'s `timespec`, in the
            // scratch.
            d::SELECT | d::SELECT_NOCANCEL => {
                let mut ts = 0;
                if x[4] != 0 {
                    ts = self.ctx_addr + SCRATCH_SLOT as u64;
                    let sec = m.read_u64(x[4]).unwrap_or(0);
                    let us = m.read_u32(x[4] + 8).unwrap_or(0) as u64;
                    let _ = m.write_u64(ts, sec);
                    let _ = m.write_u64(ts + 8, us * 1000);
                }
                set(nr::PSELECT6, &[x[0], x[1], x[2], x[3], ts, 0])
            }
            // `poll`'s milliseconds are `ppoll`'s `timespec`, likewise.
            d::POLL | d::POLL_NOCANCEL => {
                let mut ts = 0;
                let ms = x[2] as i32;
                if ms >= 0 {
                    ts = self.ctx_addr + SCRATCH_SLOT as u64;
                    let _ = m.write_u64(ts, ms as u64 / 1000);
                    let _ = m.write_u64(ts + 8, ms as u64 % 1000 * 1_000_000);
                }
                set(nr::PPOLL, &[x[0], x[1], ts, 0, 0])
            }
            // At most 256 bytes a call.
            d::GETENTROPY if x[1] <= 256 => {
                self.darwin_post = Post::Zero;
                set(nr::GETRANDOM, &[x[0], x[1], 0])
            }
            _ => NATIVE + code,
        }
    }

    /// BSD call `code`, one with no canonical form.
    pub(super) fn darwin_native(&mut self, m: &mut GuestMem<'_>, code: u64, x: [u64; 8]) -> SysResult {
        match code {
            // A signal number Darwin does not have.
            d::KILL => Err(Errno::EINVAL),
            d::GETRLIMIT | d::SETRLIMIT | d::GETENTROPY => Err(Errno::EINVAL),
            // A wait on a Mach semaphore, which is how the libc sleeps: the
            // semaphore must be one.
            d::SEMWAIT_SIGNAL | d::SEMWAIT_SIGNAL_NOCANCEL => Err(Errno::EINVAL),
            d::GETDTABLESIZE => Ok(self.rlimits.get()[RLIMIT_NOFILE][0].min(i32::MAX as u64)),
            d::DUP2 => {
                if x[0] != x[1] {
                    self.fd_io().closing_writes(m, x[1], x[1]);
                }
                self.sys_dup2(x[0] as i32, x[1] as i32)
            }
            d::ISSETUGID => Ok(0),
            d::THREAD_SELFID => Ok(self.tid as u64),
            d::MMAP => self.darwin_mmap(m, x),
            d::FCNTL | d::FCNTL_NOCANCEL => self.darwin_fcntl(m, x[0] as i32, x[1], x[2]),
            d::SIGACTION => self.darwin_sigaction(m, x[0] as u32, x[1], x[2]),
            d::SIGPROCMASK | d::PTHREAD_SIGMASK => self.darwin_sigprocmask(m, x[0] as u32, x[1], x[2]),
            d::SIGPENDING => {
                if x[0] != 0 {
                    m.write_u32(x[0], abi::sigset_to_darwin(self.signals.pending_raw()))?;
                }
                Ok(0)
            }
            d::SIGALTSTACK => self.darwin_sigaltstack(m, x[0], x[1]),
            d::GETTIMEOFDAY => {
                let (secs, nanos) = (self.clock)();
                if x[0] != 0 {
                    m.write_u64(x[0], secs)?;
                    m.write_u64(x[0] + 8, (nanos / 1000) as u64)?;
                }
                // The timezone is not kept; the libc asks its own database.
                if x[1] != 0 {
                    m.write_u64(x[1], 0)?;
                }
                if x[2] != 0 {
                    m.write_u64(x[2], self.darwin_abstime())?;
                }
                Ok(0)
            }
            d::GETDIRENTRIES64 => self.darwin_getdirentries(m, x[0] as i32, x[1], x[2], x[3]),
            _ => {
                if !self.unimplemented.contains(&(NATIVE + code)) {
                    self.unimplemented.push(NATIVE + code);
                    if self.trace || std::env::var_os("GEX_DARWIN_UNIMPLEMENTED").is_some() {
                        eprintln!("[gex:{}] unimplemented Darwin system call {code} ({}) -> ENOSYS", self.pid, d::name(code).unwrap_or("?"));
                    }
                }
                Err(Errno::ENOSYS)
            }
        }
    }

    /// `mmap`. A mapping the caller does not place is placed on Darwin's
    /// page (16 KiB on arm64), which is coarser than the kernel's own: a
    /// range with room to spare is reserved, and the mapping made at the
    /// first Darwin page in it.
    fn darwin_mmap(&mut self, m: &mut GuestMem<'_>, x: [u64; 8]) -> SysResult {
        let (addr, len, prot_bits, flags, off) = (x[0], x[1], x[2] & 7, x[3], x[5]);
        let mut kflags = abi::mmap_flags(flags);
        // An anonymous mapping's descriptor carries Mach VM flags (a tag
        // for `vmmap`), not a file.
        let fd = if flags & abi::map::ANON != 0 { u64::MAX } else { x[4] as i32 as i64 as u64 };
        // POSIX's rules are for a caller that asks for them (the libc's
        // `mmap` does, with `MAP_UNIX03`): without, a mapping neither shared
        // nor private is private, and one of no length is no mapping.
        let unix03 = flags & abi::map::UNIX03 != 0;
        if kflags & (mflags::SHARED | mflags::PRIVATE) == 0 {
            if unix03 {
                return Err(Errno::EINVAL);
            }
            kflags |= mflags::PRIVATE;
        }
        if fd != u64::MAX {
            self.fds.get(fd as i32)?;
        }
        if len == 0 {
            return if unix03 { Err(Errno::EINVAL) } else { Ok(0) };
        }
        let page = abi::PAGE_SIZE;
        if flags & abi::map::FIXED != 0 {
            if addr % page != 0 {
                return Err(Errno::EINVAL);
            }
            return self.map_io().mmap(m, [addr, len, prot_bits, kflags, fd, off]);
        }
        let len = len.checked_next_multiple_of(page).ok_or(Errno::ENOMEM)?;
        let at = self.darwin_place(addr, len, prot_bits as u32, fd != u64::MAX, 0)?;
        // The hint is honoured: the range was free a moment ago, under the
        // same lock.
        self.map_io().mmap(m, [at, len, prot_bits, kflags, fd, off])
    }

    /// A free range of `len` bytes on a Darwin page boundary (and on one of
    /// `mask + 1`, a power of two, when that is coarser), at `hint` when
    /// that is free: found by reserving, and given back for the caller to
    /// map. `low`: where a file's mapping or code goes in a 64-bit memory.
    pub(super) fn darwin_place(&mut self, hint: u64, len: u64, prot_bits: u32, low: bool, mask: u64) -> Result<u64, Errno> {
        let align = (mask.wrapping_add(1)).max(abi::PAGE_SIZE);
        if !align.is_power_of_two() {
            return Err(Errno::EINVAL);
        }
        let low = low || prot_bits & crate::mem::prot::EXEC != 0;
        let fl = mflags::PRIVATE | mflags::ANONYMOUS;
        let mut mm = self.mem.lock();
        let reserve = |mm: &mut MemoryMap, hint: u64, len: u64| {
            if low { mm.mmap_low(hint, len, 0, fl, "") } else { mm.mmap(hint, len, 0, fl, "") }
        };
        let got = reserve(&mut mm, hint & !(align - 1), len)?;
        mm.unmap(got, len)?;
        if got % align == 0 {
            return Ok(got);
        }
        let slack = align - crate::mem::PAGE_SIZE;
        let got = reserve(&mut mm, 0, len.checked_add(slack).ok_or(Errno::ENOMEM)?)?;
        mm.unmap(got, len + slack)?;
        Ok(got.next_multiple_of(align))
    }

    /// `fcntl`: Darwin's commands, the descriptor ones as the kernel's.
    fn darwin_fcntl(&mut self, m: &mut GuestMem<'_>, fd: i32, cmd: u64, arg: u64) -> SysResult {
        match cmd {
            f::DUPFD | f::GETFD | f::SETFD | f::GETFL | f::SETFL => self.fd_io().fcntl_call(m, fd, cmd, arg),
            f::DUPFD_CLOEXEC => self.fd_io().fcntl_call(m, fd, 1030, arg),
            f::GETPATH | f::GETPATH_NOFIRMLINK => {
                let file = self.fds.get(fd)?;
                if !file.path.starts_with('/') {
                    return Err(Errno::EBADF);
                }
                let mut p = file.path.clone().into_bytes();
                p.push(0);
                // The buffer is MAXPATHLEN bytes.
                if p.len() > 1024 {
                    return Err(Errno::ENOSPC);
                }
                m.write(arg, &p)?;
                Ok(0)
            }
            // Advice about caching, and durability the kernel's files have
            // no less of anyway.
            f::RDAHEAD | f::NOCACHE | f::FULLFSYNC | f::BARRIERFSYNC | f::SETNOSIGPIPE | f::SETOWN => self.fds.get(fd).map(|_| 0),
            f::GETNOSIGPIPE | f::GETOWN => self.fds.get(fd).map(|_| 0),
            _ => {
                self.fds.get(fd)?;
                if self.trace {
                    eprintln!("[gex:{}] fcntl({fd}, {cmd}): a command Darwin has and gex does not", self.pid);
                }
                Err(Errno::EINVAL)
            }
        }
    }

    /// `sigaction`. The new action is a `struct __sigaction`: the handler,
    /// the trampoline its libc wants a handler entered through, a 32-bit
    /// mask and the flags. The old one is a `struct sigaction`, which has
    /// no trampoline.
    fn darwin_sigaction(&mut self, m: &mut GuestMem<'_>, signo: u32, act: u64, old: u64) -> SysResult {
        let l = abi::sig_to_linux(signo);
        if l == 0 {
            return Err(Errno::EINVAL);
        }
        let new = if act == 0 {
            None
        } else {
            Some(SigAction {
                handler: m.read_u64(act)?,
                restorer: m.read_u64(act + 8)?,
                mask: abi::sigset_to_linux(m.read_u32(act + 16)?),
                flags: sa_flags_to_kernel(m.read_u32(act + 20)? as u64),
            })
        };
        if old != 0 {
            let a = self.signals.action(l);
            m.write_u64(old, a.handler)?;
            m.write_u32(old + 8, abi::sigset_to_darwin(a.mask))?;
            m.write_u32(old + 12, sa_flags_to_darwin(a.flags) as u32)?;
        }
        if let Some(a) = new {
            self.signals.set_action(l, a)?;
        }
        Ok(0)
    }

    /// `sigprocmask`, with Darwin's 32-bit sets and its `how` (block 1,
    /// unblock 2, set 3).
    fn darwin_sigprocmask(&mut self, m: &mut GuestMem<'_>, how: u32, set: u64, old: u64) -> SysResult {
        let blocked = self.signals.blocked();
        let want = if set == 0 {
            None
        } else {
            let mask = abi::sigset_to_linux(m.read_u32(set)?);
            Some(match how {
                1 => blocked | mask,
                2 => blocked & !mask,
                3 => mask,
                _ => return Err(Errno::EINVAL),
            })
        };
        if old != 0 {
            m.write_u32(old, abi::sigset_to_darwin(blocked))?;
        }
        if let Some(mask) = want {
            self.signals.set_blocked(mask & !UNBLOCKABLE);
        }
        Ok(0)
    }

    /// `sigaltstack`. Darwin's `stack_t` is the pointer, the size, then the
    /// flags, with `SS_DISABLE` 4 (Linux's is 2).
    fn darwin_sigaltstack(&mut self, m: &mut GuestMem<'_>, new: u64, old: u64) -> SysResult {
        let flags_in = |f: u32| (f & 1) | if f & 4 != 0 { 2 } else { 0 };
        let flags_out = |f: u32| (f & 1) | if f & 2 != 0 { 4 } else { 0 };
        let want = if new == 0 {
            None
        } else {
            Some(SigAltStack { sp: m.read_u64(new)?, size: m.read_u64(new + 8)?, flags: flags_in(m.read_u32(new + 16)?) })
        };
        if old != 0 {
            let a = self.signals.alt_stack();
            m.write_u64(old, a.sp)?;
            m.write_u64(old + 8, a.size)?;
            // A thread that has set none has a disabled one.
            m.write_u32(old + 16, if a.size == 0 { 4 } else { flags_out(a.flags) })?;
        }
        if let Some(a) = want {
            self.signals.set_alt_stack(a);
        }
        Ok(0)
    }

    /// `getdirentries64`: Darwin's directory records, and at `basep` where
    /// the read began.
    fn darwin_getdirentries(&mut self, m: &mut GuestMem<'_>, fd: i32, buf: u64, len: u64, basep: u64) -> SysResult {
        let file = self.fds.get(fd)?;
        let before = file.seek(0, 1).unwrap_or(0);
        let linux = file.read_dirents(abi::dirents_room(len) as usize)?;
        let out = abi::dirents(&linux);
        m.write(buf, &out)?;
        if basep != 0 {
            m.write_u64(basep, before)?;
        }
        Ok(out.len() as u64)
    }
}
