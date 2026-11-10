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
        if let Some(n) = self.darwin_socket_call(m, code, x, a) {
            return n;
        }
        if matches!(code, d::CLOSE | d::CLOSE_NOCANCEL | d::GUARDED_CLOSE_NP) {
            self.darwin_kqueue_closing(x[0] as i32);
        }
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
            // (path, flags, protection class, protection flags, mode):
            // files are protected no differently here.
            d::OPEN_DPROTECTED_NP => set(nr::OPENAT, &[CWD, x[0], x[1], x[4]]),
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
            d::UNLINK => {
                self.darwin_post = Post::Unlink;
                set(nr::UNLINKAT, &[CWD, x[0], 0])
            }
            // (`AT_REMOVEDIR_DATALESS`, 0x100, is a directory removed
            // without its contents being fetched first, which `rm` asks
            // for: a directory removed.)
            d::UNLINKAT => {
                self.darwin_post = Post::Unlink;
                set(nr::UNLINKAT, &[dirfd(x[0]), x[1], at_flags(x[2]) | if x[2] & 0x100 != 0 { K_REMOVEDIR } else { 0 }])
            }
            d::RMDIR => set(nr::UNLINKAT, &[CWD, x[0], K_REMOVEDIR]),
            d::MKDIR => set(nr::MKDIRAT, &[CWD, x[0], x[1]]),
            d::MKDIRAT => set(nr::MKDIRAT, &[dirfd(x[0]), x[1], x[2]]),
            d::RENAME => set(nr::RENAMEAT, &[CWD, x[0], CWD, x[1]]),
            d::RENAMEAT => set(nr::RENAMEAT, &[dirfd(x[0]), x[1], dirfd(x[2]), x[3]]),
            // RENAME_EXCL (4) is the kernel's RENAME_NOREPLACE. RENAME_SWAP
            // (2) is not something its filesystems do, which is ENOTSUP
            // ([`Kernel::darwin_native`]) as it is of a volume that cannot.
            d::RENAMEATX_NP if x[4] & !4 == 0 => set(nr::RENAMEAT2, &[dirfd(x[0]), x[1], dirfd(x[2]), x[3], x[4] >> 2]),
            // A file's flags (`chflags`) are not kept; the file must exist.
            d::CHFLAGS => set(nr::FACCESSAT, &[CWD, x[0], 0]),
            d::FCHFLAGS => set(nr::FCNTL, &[x[0], 1]),
            // The kernel's answer is one for every file, there or not: the
            // call is one that says whether the file is, and the answer is
            // written when it has ([`Post::Statfs`]).
            d::STATFS64 | d::STATFS => {
                self.darwin_post = Post::Statfs { at: x[1], old: code == d::STATFS };
                set(nr::FACCESSAT, &[CWD, x[0], 0])
            }
            d::FSTATFS64 | d::FSTATFS => {
                self.darwin_post = Post::Statfs { at: x[1], old: code == d::FSTATFS };
                set(nr::FCNTL, &[x[0], 1])
            }
            // The four from before 64-bit inodes, which an old program
            // still makes: the same calls, their structure the old one
            // ([`Post::OldStat`]). What lies past it is the caller's.
            d::STAT | d::LSTAT | d::FSTAT | d::FSTATAT => {
                let at = if code == d::FSTATAT { x[2] } else { x[1] };
                let mut was = [0u64; 3];
                for (i, w) in was.iter_mut().enumerate() {
                    *w = m.read_u64(at + abi::STAT_OLD as u64 + 8 * i as u64).unwrap_or(0);
                }
                self.darwin_post = Post::OldStat { at, was };
                match code {
                    d::STAT => set(nr::NEWFSTATAT, &[CWD, x[0], x[1], 0]),
                    d::LSTAT => set(nr::NEWFSTATAT, &[CWD, x[0], x[1], K_NOFOLLOW]),
                    d::FSTAT => set(nr::FSTAT, &x[..2]),
                    _ => set(nr::NEWFSTATAT, &[dirfd(x[0]), x[1], x[2], at_flags(x[3])]),
                }
            }
            // Darwin's calls carry a position (for a resource fork, which
            // nothing here has) and options: XATTR_NOFOLLOW (1), and for a
            // set XATTR_CREATE (2) and XATTR_REPLACE (4), the kernel's 1
            // and 2.
            d::GETXATTR => {
                self.darwin_post = Post::Xattr;
                set(if x[5] & 1 != 0 { nr::LGETXATTR } else { nr::GETXATTR }, &x[..4])
            }
            d::FGETXATTR => {
                self.darwin_post = Post::Xattr;
                set(nr::FGETXATTR, &x[..4])
            }
            d::SETXATTR => {
                self.darwin_post = Post::Xattr;
                set(if x[5] & 1 != 0 { nr::LSETXATTR } else { nr::SETXATTR }, &[x[0], x[1], x[2], x[3], (x[5] >> 1) & 3])
            }
            d::FSETXATTR => {
                self.darwin_post = Post::Xattr;
                set(nr::FSETXATTR, &[x[0], x[1], x[2], x[3], (x[5] >> 1) & 3])
            }
            d::LISTXATTR => set(if x[3] & 1 != 0 { nr::LLISTXATTR } else { nr::LISTXATTR }, &x[..3]),
            d::FLISTXATTR => set(nr::FLISTXATTR, &x[..3]),
            d::REMOVEXATTR => {
                self.darwin_post = Post::Xattr;
                set(if x[2] & 1 != 0 { nr::LREMOVEXATTR } else { nr::REMOVEXATTR }, &x[..2])
            }
            d::FREMOVEXATTR => {
                self.darwin_post = Post::Xattr;
                set(nr::FREMOVEXATTR, &x[..2])
            }
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
            d::GETGROUPS => NATIVE + d::GETGROUPS,
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
            // (idtype, id, siginfo, options). The kernel's `siginfo_t` is
            // longer than Darwin's: what lies past the caller's is kept.
            // WNOHANG and WEXITED are the same bits; WUNTRACED (2) and
            // WSTOPPED (8) are the kernel's WSTOPPED (2), WCONTINUED (0x10)
            // its 8, WNOWAIT (0x20) its 0x1000000.
            d::WAITID | d::WAITID_NOCANCEL => {
                let mut was = [0u64; 16];
                if x[2] != 0 {
                    for (i, w) in was.iter_mut().enumerate() {
                        *w = m.read_u64(x[2] + 8 * i as u64).unwrap_or(0);
                    }
                }
                self.darwin_post = Post::Waitid { at: x[2], was };
                let o = x[3];
                let options = (o & 5) | if o & 0xa != 0 { 2 } else { 0 } | (o & 0x10) >> 1 | if o & 0x20 != 0 { 0x100_0000 } else { 0 };
                set(nr::WAITID, &[x[0], x[1], x[2], options, 0])
            }
            // (set, signal): wait for one of a set, which is given by
            // number through the second.
            d::SIGWAIT | d::SIGWAIT_NOCANCEL => {
                let at = self.ctx_addr + SCRATCH_SLOT as u64;
                match m.read_u32(x[0]) {
                    Ok(set32) => {
                        let _ = m.write_u64(at, abi::sigset_to_linux(set32));
                        self.darwin_post = Post::Sigwait { at: x[1] };
                        set(nr::RT_SIGTIMEDWAIT, &[at, 0, 0, 8])
                    }
                    Err(_) => NATIVE + d::SIGWAIT,
                }
            }
            // `abort`'s last step, and the same asked of another process:
            // the signal, with a reason nobody is here to read.
            d::ABORT_WITH_PAYLOAD => set(nr::KILL, &[self.pid as u64, signal::sig::SIGABRT as u64]),
            d::TERMINATE_WITH_PAYLOAD => set(nr::KILL, &[x[0] as i32 as i64 as u64, signal::sig::SIGKILL as u64]),
            d::MINHERIT => set(nr::NOOP_OK, &[]),
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
            // (semaphore, mutex's semaphore, whether there is a time,
            // whether it is from now, seconds, nanoseconds): a wait on a
            // Mach semaphore, which is how the libc sleeps (on one nobody
            // signals, for the time).
            d::SEMWAIT_SIGNAL | d::SEMWAIT_SIGNAL_NOCANCEL => {
                let timeout = (x[2] as u32 != 0).then(|| {
                    let ns = x[4].wrapping_mul(1_000_000_000).wrapping_add(x[5] & 0xffff_ffff);
                    if x[3] as u32 != 0 {
                        ns
                    } else {
                        let (secs, nanos) = (self.clock)();
                        ns.saturating_sub(secs * 1_000_000_000 + nanos as u64)
                    }
                });
                return match self.darwin_sem_wait(m, x[0] as u32, timeout, a) {
                    Ok(Some(call)) => {
                        self.darwin_post = Post::SemWait;
                        call
                    }
                    Ok(None) => {
                        *a = [0; 6];
                        nr::NOOP_OK
                    }
                    Err(_) => NATIVE + code,
                };
            }
            d::ULOCK_WAIT => return self.darwin_ulock_wait(m, x, false, a),
            d::ULOCK_WAIT2 => return self.darwin_ulock_wait(m, x, true, a),
            d::ULOCK_WAKE => return self.darwin_ulock_wake(x, a),
            d::POSIX_SPAWN => return self.darwin_posix_spawn(m, x, a),
            d::WORKQ_KERNRETURN => return self.darwin_workq(m, x, a),
            d::KEVENT => return self.darwin_kevent(m, kqueue::Form::Plain, x, a),
            d::KEVENT64 => return self.darwin_kevent(m, kqueue::Form::Wide, x, a),
            d::KEVENT_QOS => return self.darwin_kevent(m, kqueue::Form::Qos, x, a),
            d::PSYNCH_MUTEXWAIT
            | d::PSYNCH_MUTEXDROP
            | d::PSYNCH_CVBROAD
            | d::PSYNCH_CVSIGNAL
            | d::PSYNCH_CVWAIT
            | d::PSYNCH_CVCLRPREPOST
            | d::PSYNCH_RW_RDLOCK
            | d::PSYNCH_RW_WRLOCK
            | d::PSYNCH_RW_UNLOCK
            | d::PSYNCH_RW_UNLOCK2
            | d::PSYNCH_RW_LONGRDLOCK
            | d::PSYNCH_RW_YIELDWRLOCK
            | d::PSYNCH_RW_UPGRADE
            | d::PSYNCH_RW_DOWNGRADE => return self.darwin_psynch(m, code, x, a),
            // (thread port, signal): a signal for one thread of this process.
            d::PTHREAD_KILL => match (super::mach::port_thread(self.pid, x[0] as u32), abi::sig_to_linux(x[1] as u32)) {
                (Some(tid), l) if l != 0 || x[1] == 0 => set(nr::TGKILL, &[self.pid as u64, tid as u64, l as u64]),
                (None, _) => NATIVE + d::PTHREAD_KILL,
                _ => NATIVE + d::KILL,
            },
            _ => NATIVE + code,
        }
    }

    /// BSD call `code`, one with no canonical form.
    pub(super) fn darwin_native(&mut self, m: &mut GuestMem<'_>, code: u64, x: [u64; 8]) -> SysResult {
        match code {
            // A signal number Darwin does not have.
            d::KILL => Err(Errno::EINVAL),
            d::SIGWAIT => Err(Errno::EFAULT),
            d::SYSCTL => self.darwin_sysctl(m, x),
            d::SYSCTLBYNAME => self.darwin_sysctlbyname(m, x),
            d::CSOPS | d::CSOPS_AUDITTOKEN => self.darwin_csops(m, x),
            d::PROC_INFO => self.darwin_proc_info(m, x),
            d::IOCTL => self.darwin_ioctl(m, x[0] as i32, x[1], x[2]),
            // Not a socket.
            d::SHUTDOWN => Err(Errno::ENOTSOCK),
            d::SETSOCKOPT => self.darwin_sockopt(m, false, x),
            d::GETSOCKOPT => self.darwin_sockopt(m, true, x),
            d::GETFSSTAT64 | d::GETFSSTAT => self.darwin_getfsstat(m, x[0], x[1], code == d::GETFSSTAT),
            d::GETDIRENTRIES => self.darwin_getdirentries(m, x[0] as i32, x[1], x[2] & 0xffff_ffff, x[3], true),
            d::PATHCONF => self.darwin_pathconf(x[1]),
            d::FPATHCONF => self.fds.get(x[0] as i32).and_then(|_| self.darwin_pathconf(x[1])),
            // Trace points, which nothing collects.
            d::KDEBUG_TRACE | d::KDEBUG_TRACE64 | d::KDEBUG_TRACE_STRING | d::KDEBUG_TYPEFILTER => Ok(0),
            // PT_DENY_ATTACH: there is no debugger to deny.
            d::PTRACE if x[0] == 31 => Ok(0),
            // The name the session was logged in under.
            d::GETLOGIN => {
                let mut name = if self.creds.ruid() == 0 { "root" } else { "user" }.as_bytes().to_vec();
                name.push(0);
                if (x[1] as usize) < name.len() {
                    return Err(Errno::ERANGE);
                }
                m.write(x[0], &name)?;
                Ok(0)
            }
            // The machine's identifier: one every gex machine has.
            d::GETHOSTUUID => {
                m.write(x[0], b"gex\0\0\0\0\0\0\0\0\0\0\0\0\x01")?;
                Ok(0)
            }
            // The calling thread's own user and group, when it has taken
            // on another's: none has.
            d::GETTID => Err(Errno::ESRCH),
            // Whether the thread has been cancelled (it has not), and its
            // cancellation switched on or off.
            d::PTHREAD_CANCELED => if x[0] == 0 { Err(Errno::EINVAL) } else { Ok(0) },
            // No copy-on-write clones, and no exchange of two names: the
            // caller does without.
            d::CLONEFILEAT | d::RENAMEATX_NP => Err(Errno::EOPNOTSUPP),
            d::FSCTL | d::FFSCTL => Err(Errno::ENOTTY),
            // A port that is no thread's.
            d::PTHREAD_KILL => Err(Errno::ESRCH),
            d::BSDTHREAD_REGISTER => self.darwin_bsdthread_register(m, x),
            // An operation `ulock` does not have.
            d::ULOCK_WAIT => Err(Errno::EINVAL),
            // A `posix_spawn` that failed before anything was made.
            d::POSIX_SPAWN => self.darwin_answer.take().unwrap_or(Err(Errno::EINVAL)),
            // (A guarded descriptor is one the program promises to close
            // only by saying its guard again; the promise is not held it
            // to here.)
            d::KQUEUE | d::GUARDED_KQUEUE_NP => self.darwin_kqueue(),
            d::GUARDED_CLOSE_NP => self.dispatch(m, nr::CLOSE, [x[0], 0, 0, 0, 0, 0]).unwrap_or(Err(Errno::EBADF)),
            // A work queue is there for the asking.
            d::WORKQ_OPEN => Ok(0),
            d::WORKQ_KERNRETURN => self.darwin_answer.take().unwrap_or(Err(Errno::EINVAL)),
            // A `kevent` that did not wait: its answer.
            d::KEVENT => self.darwin_answer.take().unwrap_or(Err(Errno::EINVAL)),
            // A psynch call that did not wait: its answer.
            d::PSYNCH_RW_LONGRDLOCK..=d::PSYNCH_RW_UNLOCK2 | d::PSYNCH_CVCLRPREPOST => self.darwin_answer.take().unwrap_or(Err(Errno::EINVAL)),
            // Quality-of-service requests: there is one class of thread.
            d::BSDTHREAD_CTL => Ok(0),
            // Signals are delivered to whichever thread does not block
            // them; a thread that wants none says so at its end, when there
            // is nothing left to deliver to.
            d::DISABLE_THREADSIGNAL => Ok(0),
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
            d::GETDIRENTRIES64 => self.darwin_getdirentries(m, x[0] as i32, x[1], x[2], x[3], false),
            // The groups a process is in: the kernel's list, which is of
            // the ones beside its own and empty until given; Darwin's
            // always has the process's own group, first.
            d::GETGROUPS => {
                let n = self.dispatch(m, nr::GETGROUPS, [x[0], x[1], 0, 0, 0, 0]).unwrap_or(Err(Errno::EINVAL))?;
                if n == 0 && x[0] as i32 > 0 {
                    m.write_u32(x[1], self.creds.gid())?;
                }
                Ok(n.max(1))
            }
            // System Integrity Protection: on, and letting nothing past
            // (`csr_check` is refused, the configuration allows nothing).
            d::CSRCTL => match x[0] {
                0 => Err(Errno::EPERM),
                1 if x[2] >= 4 => m.write_u32(x[1], 0).map(|_| 0),
                _ => Err(Errno::EINVAL),
            },
            d::GETATTRLIST => self.darwin_getattrlist(m, CWD as i32, x[0], x[1], x[2], x[3], x[4]),
            d::FGETATTRLIST => self.darwin_getattrlist(m, x[0] as i32, 0, x[1], x[2], x[3], x[4]),
            d::GETATTRLISTBULK => self.darwin_getattrlistbulk(m, x[0] as i32, x[1], x[2], x[3], x[4]),
            d::GETATTRLISTAT => self.darwin_getattrlist(m, dirfd(x[0]) as i32, x[1], x[2], x[3], x[4], x[5]),
            d::SETATTRLIST => self.darwin_setattrlist(m, CWD as i32, x[0], x[1], x[2], x[3], x[4]),
            d::FSETATTRLIST => self.darwin_setattrlist(m, x[0] as i32, 0, x[1], x[2], x[3], x[4]),
            d::SETATTRLISTAT => self.darwin_setattrlist(m, dirfd(x[0]) as i32, x[1], x[2], x[3], x[4], x[5]),
            d::STAT64_EXTENDED | d::LSTAT64_EXTENDED | d::FSTAT64_EXTENDED => self.darwin_stat_extended(m, code, x),
            d::CHMOD_EXTENDED => self.darwin_chmod_extended(m, None, x[0], x[1] as u32, x[2] as u32, x[3] as u32, x[4]),
            d::FCHMOD_EXTENDED => self.darwin_chmod_extended(m, Some(x[0] as i32), 0, x[1] as u32, x[2] as u32, x[3] as u32, x[4]),
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

    /// `getfsstat`: the mounted filesystems, of which there is one. No
    /// buffer asks how many.
    fn darwin_getfsstat(&mut self, m: &mut GuestMem<'_>, buf: u64, size: u64, old: bool) -> SysResult {
        if buf == 0 {
            return Ok(1);
        }
        if (size as usize) < if old { abi::STATFS_OLD } else { abi::STATFS } {
            return Ok(0);
        }
        // The kernel's answer is the same for every file: asked of the
        // caller's buffer, and respelled there.
        crate::syscalls::statfs(m, false, buf)?;
        let k = m.read(buf, 120)?.to_vec();
        m.write(buf, &if old { abi::statfs_old(&k) } else { abi::statfs(&k) })?;
        Ok(1)
    }

    /// `pathconf` and `fpathconf`: a limit of the filesystem, by Darwin's
    /// numbers (`<sys/unistd.h>`).
    fn darwin_pathconf(&mut self, name: u64) -> SysResult {
        Ok(match name {
            1 => 32767,       // _PC_LINK_MAX
            2 | 3 => 1024,    // _PC_MAX_CANON, _PC_MAX_INPUT
            4 => 255,         // _PC_NAME_MAX
            5 => 1024,        // _PC_PATH_MAX
            6 => 512,         // _PC_PIPE_BUF
            7 | 8 => 200112,  // _PC_CHOWN_RESTRICTED, _PC_NO_TRUNC
            9 => 255,         // _PC_VDISABLE
            11 | 12 => 1,     // _PC_CASE_SENSITIVE, _PC_CASE_PRESERVING
            18 => 64,         // _PC_FILESIZEBITS
            24 => 1024,       // _PC_SYMLINK_MAX
            26 => 64,         // _PC_XATTR_SIZE_BITS
            27 => 8,          // _PC_MIN_HOLE_SIZE: not reported
            _ => return Err(Errno::EINVAL),
        })
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
    fn darwin_getdirentries(&mut self, m: &mut GuestMem<'_>, fd: i32, buf: u64, len: u64, basep: u64, old: bool) -> SysResult {
        let file = self.fds.get(fd)?;
        let before = file.seek(0, 1).unwrap_or(0);
        // What is no directory is an invalid argument to this call (as is
        // a buffer with no room for one record, which the read says).
        let linux = file.read_dirents(abi::dirents_room(len) as usize).map_err(|e| if e == Errno::ENOTDIR { Errno::EINVAL } else { e })?;
        // `old`: the records of before 64-bit inodes (`getdirentries`).
        let out = if old { abi::dirents_old(&linux) } else { abi::dirents(&linux) };
        m.write(buf, &out)?;
        if basep != 0 {
            m.write_u64(basep, before)?;
        }
        Ok(out.len() as u64)
    }
}
