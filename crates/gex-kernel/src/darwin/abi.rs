//! Darwin's numbers and layouts, against the kernel's own (Linux's).
//!
//! The kernel is written in Linux's vocabulary: its error numbers, its
//! signal numbers, its flag bits. A Darwin process spells nearly all of
//! them differently, and every difference is in a table here. The values
//! are XNU's (`bsd/sys/errno.h`, `signal.h`, `fcntl.h`, `mman.h`, `stat.h`).

use crate::errno::Errno;
use crate::fs::{oflags, Stat, StatBytes};

/// Darwin error numbers the kernel names when a Darwin call fails in a way
/// Linux has no number for.
pub mod err {
    pub const ENOTSUP: u32 = 45;
    pub const ENOATTR: u32 = 93;
}

/// `(linux, darwin)` for every error number the two spell differently.
/// 1 to 34 are the same in both, but for 11: Linux's `EAGAIN` is Darwin's
/// `EDEADLK`.
const ERRNO: &[(i32, u32)] = &[
    (11, 35),   // EAGAIN
    (35, 11),   // EDEADLK
    (36, 63),   // ENAMETOOLONG
    (37, 77),   // ENOLCK
    (38, 78),   // ENOSYS
    (39, 66),   // ENOTEMPTY
    (40, 62),   // ELOOP
    (42, 91),   // ENOMSG
    (43, 90),   // EIDRM
    (60, 99),   // ENOSTR
    (61, 96),   // ENODATA
    (62, 101),  // ETIME
    (63, 98),   // ENOSR
    (67, 97),   // ENOLINK
    (71, 100),  // EPROTO
    (72, 95),   // EMULTIHOP
    (74, 94),   // EBADMSG
    (75, 84),   // EOVERFLOW
    (84, 92),   // EILSEQ
    (87, 68),   // EUSERS
    (88, 38),   // ENOTSOCK
    (89, 39),   // EDESTADDRREQ
    (90, 40),   // EMSGSIZE
    (91, 41),   // EPROTOTYPE
    (92, 42),   // ENOPROTOOPT
    (93, 43),   // EPROTONOSUPPORT
    (94, 44),   // ESOCKTNOSUPPORT
    (95, 45),   // EOPNOTSUPP, which is Linux's ENOTSUP too: Darwin's ENOTSUP
    (96, 46),   // EPFNOSUPPORT
    (97, 47),   // EAFNOSUPPORT
    (98, 48),   // EADDRINUSE
    (99, 49),   // EADDRNOTAVAIL
    (100, 50),  // ENETDOWN
    (101, 51),  // ENETUNREACH
    (102, 52),  // ENETRESET
    (103, 53),  // ECONNABORTED
    (104, 54),  // ECONNRESET
    (105, 55),  // ENOBUFS
    (106, 56),  // EISCONN
    (107, 57),  // ENOTCONN
    (108, 58),  // ESHUTDOWN
    (109, 59),  // ETOOMANYREFS
    (110, 60),  // ETIMEDOUT
    (111, 61),  // ECONNREFUSED
    (112, 64),  // EHOSTDOWN
    (113, 65),  // EHOSTUNREACH
    (114, 37),  // EALREADY
    (115, 36),  // EINPROGRESS
    (116, 70),  // ESTALE
    (122, 69),  // EDQUOT
    (125, 89),  // ECANCELED
    (130, 105), // EOWNERDEAD
    (131, 104), // ENOTRECOVERABLE
];

/// The kernel's error number as a Darwin process reads it. One with no
/// Darwin spelling is `EIO`.
pub fn errno_to_darwin(e: Errno) -> u32 {
    let l = e.0;
    if let Some(&(_, d)) = ERRNO.iter().find(|(x, _)| *x == l) {
        return d;
    }
    if (1..=34).contains(&l) { l as u32 } else { 5 }
}

/// A Darwin error number (a macOS host's own) as the kernel spells it.
pub fn errno_from_darwin(d: i32) -> Errno {
    // 45 and 102 are both Linux's 95.
    if d == 102 {
        return Errno::EOPNOTSUPP;
    }
    if let Some(&(l, _)) = ERRNO.iter().find(|(_, x)| *x as i32 == d) {
        return Errno(l);
    }
    if (1..=34).contains(&d) { Errno(d) } else { Errno::EIO }
}

/// Darwin's signal numbers, indexed by Linux's: both number 1 to 31, in
/// different orders. Linux's 16 (`SIGSTKFLT`) and 30 (`SIGPWR`) stand in for
/// Darwin's 7 (`SIGEMT`) and 29 (`SIGINFO`), which Linux has no number for.
const SIG_TO_DARWIN: [u8; 32] = [
    0, 1, 2, 3, 4, 5, 6, 10, 8, 9, 30, 11, 31, 13, 14, 15, 7, 20, 19, 17, 18, 21, 22, 16, 24, 25, 26, 27, 28, 23, 29, 12,
];

/// Darwin's highest signal number.
pub const NSIG: u32 = 31;

/// A Linux signal number as a Darwin process names it; 0 for one it has no
/// name for (the real-time signals).
pub fn sig_to_darwin(l: u32) -> u32 {
    SIG_TO_DARWIN.get(l as usize).map_or(0, |d| *d as u32)
}

/// A Darwin signal number as the kernel names it; 0 for none.
pub fn sig_to_linux(d: u32) -> u32 {
    if d == 0 || d > NSIG {
        return 0;
    }
    SIG_TO_DARWIN.iter().position(|x| *x as u32 == d).unwrap_or(0) as u32
}

/// A Darwin `sigset_t` (32 bits, bit `signal - 1`) as the kernel's mask.
pub fn sigset_to_linux(d: u32) -> u64 {
    (1..=NSIG).filter(|s| d & (1 << (s - 1)) != 0).fold(0, |m, s| m | 1u64 << (sig_to_linux(s) - 1))
}

/// The kernel's mask as a Darwin `sigset_t`.
pub fn sigset_to_darwin(l: u64) -> u32 {
    (1..=31u32).filter(|s| l & (1 << (s - 1)) != 0).fold(0, |m, s| m | 1u32 << (sig_to_darwin(s) - 1))
}

/// A wait status (`wait4`'s word) with its signal numbers as Darwin's: the
/// terminating signal in the low seven bits, the stopping one in the second
/// byte of a stopped status (low byte `0x7f`).
pub fn wait_status_to_darwin(w: u32) -> u32 {
    match w & 0x7f {
        0 => w,
        0x7f => (w & !0xff00) | (sig_to_darwin((w >> 8) & 0xff) << 8),
        s => (w & !0x7f) | sig_to_darwin(s),
    }
}

/// Darwin's `open` flags.
pub mod o {
    pub const NONBLOCK: u64 = 0x4;
    pub const APPEND: u64 = 0x8;
    pub const SHLOCK: u64 = 0x10;
    pub const EXLOCK: u64 = 0x20;
    pub const ASYNC: u64 = 0x40;
    pub const SYNC: u64 = 0x80;
    pub const NOFOLLOW: u64 = 0x100;
    pub const CREAT: u64 = 0x200;
    pub const TRUNC: u64 = 0x400;
    pub const EXCL: u64 = 0x800;
    pub const EVTONLY: u64 = 0x8000;
    pub const NOCTTY: u64 = 0x2_0000;
    pub const DIRECTORY: u64 = 0x10_0000;
    pub const SYMLINK: u64 = 0x20_0000;
    pub const DSYNC: u64 = 0x40_0000;
    pub const CLOEXEC: u64 = 0x100_0000;
    pub const NOFOLLOW_ANY: u64 = 0x2000_0000;
}

/// Each `(darwin, kernel)` pair of open flags both have. `O_SYNC` and
/// `O_DSYNC` have no bit in the kernel's flags (every write is as durable as
/// the next) and are dropped.
const OFLAGS: [(u64, u64); 9] = [
    (o::NONBLOCK, oflags::O_NONBLOCK),
    (o::APPEND, oflags::O_APPEND),
    (o::NOFOLLOW, oflags::O_NOFOLLOW),
    (o::CREAT, oflags::O_CREAT),
    (o::TRUNC, oflags::O_TRUNC),
    (o::EXCL, oflags::O_EXCL),
    (o::NOCTTY, oflags::O_NOCTTY),
    (o::DIRECTORY, oflags::O_DIRECTORY),
    (o::CLOEXEC, oflags::O_CLOEXEC),
];

/// Darwin open flags in the kernel's spelling. `O_SYMLINK` (open the link
/// itself) is a name-only handle that does not follow; `O_EVTONLY` (a
/// descriptor for watching, not reading) opens as asked.
pub fn oflags_to_kernel(f: u64) -> u64 {
    let mut out = f & oflags::ACCMODE;
    for (d, k) in OFLAGS {
        if f & d != 0 {
            out |= k;
        }
    }
    if f & o::SYMLINK != 0 {
        out |= oflags::O_PATH | oflags::O_NOFOLLOW;
    }
    if f & o::NOFOLLOW_ANY != 0 {
        out |= oflags::O_NOFOLLOW;
    }
    out
}

/// The kernel's open flags as Darwin's (`fcntl(F_GETFL)`).
pub fn oflags_to_darwin(f: u64) -> u64 {
    let mut out = f & oflags::ACCMODE;
    for (d, k) in OFLAGS {
        if f & k != 0 {
            out |= d;
        }
    }
    out
}

/// `AT_FDCWD`, which Darwin spells -2 and the kernel -100.
pub const AT_FDCWD: i32 = -2;
const LINUX_AT_FDCWD: u64 = (-100i64) as u64;

/// Darwin's `*at` flags.
pub mod at {
    pub const EACCESS: u64 = 0x10;
    pub const SYMLINK_NOFOLLOW: u64 = 0x20;
    pub const SYMLINK_FOLLOW: u64 = 0x40;
    pub const REMOVEDIR: u64 = 0x80;
}

/// A directory descriptor argument: Darwin's `AT_FDCWD` as the kernel's.
pub fn dirfd(fd: u64) -> u64 {
    if fd as i32 == AT_FDCWD { LINUX_AT_FDCWD } else { fd as i32 as i64 as u64 }
}

/// Darwin `*at` flags as the kernel's: `AT_SYMLINK_NOFOLLOW` 0x100,
/// `AT_REMOVEDIR` and `AT_EACCESS` 0x200, `AT_SYMLINK_FOLLOW` 0x400.
pub fn at_flags(f: u64) -> u64 {
    let mut out = 0;
    if f & at::SYMLINK_NOFOLLOW != 0 {
        out |= 0x100;
    }
    if f & (at::REMOVEDIR | at::EACCESS) != 0 {
        out |= 0x200;
    }
    if f & at::SYMLINK_FOLLOW != 0 {
        out |= 0x400;
    }
    out
}

/// Darwin's `mmap` flags.
pub mod map {
    pub const SHARED: u64 = 0x1;
    pub const PRIVATE: u64 = 0x2;
    pub const FIXED: u64 = 0x10;
    pub const NORESERVE: u64 = 0x40;
    pub const JIT: u64 = 0x800;
    pub const ANON: u64 = 0x1000;
}

/// Darwin `mmap` flags as the kernel's.
pub fn mmap_flags(f: u64) -> u64 {
    use crate::mem::flags as k;
    let mut out = 0;
    if f & map::SHARED != 0 {
        out |= k::SHARED;
    }
    if f & map::PRIVATE != 0 {
        out |= k::PRIVATE;
    }
    if f & map::FIXED != 0 {
        out |= k::FIXED;
    }
    if f & map::ANON != 0 {
        out |= k::ANONYMOUS;
    }
    out
}

/// The page a Darwin process's mappings are made of: 16 KiB on arm64,
/// 4 KiB on x86-64. (`vm_page_size`, what its allocator and its dynamic
/// linker round to.)
pub const PAGE_SIZE: u64 = if gex_abi::X86 { 0x1000 } else { 0x4000 };

/// Darwin's `struct stat` with 64-bit inode numbers, the only one an arm64
/// process has (`stat64` on x86-64), 144 bytes:
///
/// ```text
///   0 st_dev      i32      4 st_mode  u16      6 st_nlink  u16
///   8 st_ino      u64     16 st_uid   u32     20 st_gid    u32
///  24 st_rdev     i32     32 st_atimespec     48 st_mtimespec
///  64 st_ctimespec        80 st_birthtimespec
///  96 st_size     i64    104 st_blocks i64   112 st_blksize i32
/// 116 st_flags    u32    120 st_gen    u32   124 st_lspare, st_qspare[2]
/// ```
pub fn stat(st: &Stat) -> StatBytes {
    let mut b = StatBytes::zeroed(144);
    b[0..4].copy_from_slice(&dev(st.dev).to_le_bytes());
    b[4..6].copy_from_slice(&(st.mode as u16).to_le_bytes());
    b[6..8].copy_from_slice(&(st.nlink.min(u16::MAX as u32) as u16).to_le_bytes());
    b[8..16].copy_from_slice(&st.ino.to_le_bytes());
    b[16..20].copy_from_slice(&st.uid.to_le_bytes());
    b[20..24].copy_from_slice(&st.gid.to_le_bytes());
    b[24..28].copy_from_slice(&dev(st.rdev).to_le_bytes());
    // The birth time is reported as the modification time: the kernel keeps
    // none.
    for (off, (t, ns)) in [32usize, 48, 64, 80].into_iter().zip([
        (st.atime, st.atime_ns),
        (st.mtime, st.mtime_ns),
        (st.ctime, st.ctime_ns),
        (st.mtime, st.mtime_ns),
    ]) {
        b[off..off + 8].copy_from_slice(&t.to_le_bytes());
        b[off + 8..off + 16].copy_from_slice(&(ns as u64).to_le_bytes());
    }
    b[96..104].copy_from_slice(&st.size.to_le_bytes());
    b[104..112].copy_from_slice(&st.blocks.to_le_bytes());
    b[112..116].copy_from_slice(&st.blksize.to_le_bytes());
    b
}

/// A Linux device number (major in bits 8 to 19, minor in the rest) as
/// Darwin's 32-bit `dev_t`: major in the top byte, minor in the low 24 bits.
pub fn dev(d: u64) -> u32 {
    let major = ((d >> 8) & 0xfff) as u32;
    let minor = ((d & 0xff) | ((d >> 12) & 0xfff00)) as u32;
    ((major & 0xff) << 24) | (minor & 0xff_ffff)
}

/// Linux `struct dirent64` records (`getdents64`'s) as Darwin's 64-bit
/// `struct dirent` (`getdirentries64`'s):
///
/// ```text
///   0 d_ino u64    8 d_seekoff u64   16 d_reclen u16
///  18 d_namlen u16 20 d_type u8      21 d_name, NUL-terminated
/// ```
///
/// each padded to eight bytes. The file types number alike (`DT_*`).
pub fn dirents(linux: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(linux.len() + linux.len() / 3);
    let mut at = 0;
    while at + 19 <= linux.len() {
        let reclen = u16::from_le_bytes([linux[at + 16], linux[at + 17]]) as usize;
        if reclen < 19 || at + reclen > linux.len() {
            break;
        }
        let name = &linux[at + 19..at + reclen];
        let name = &name[..name.iter().position(|c| *c == 0).unwrap_or(name.len())];
        let len = (21 + name.len() + 1).next_multiple_of(8);
        let start = out.len();
        out.resize(start + len, 0);
        out[start..start + 16].copy_from_slice(&linux[at..at + 16]);
        out[start + 16..start + 18].copy_from_slice(&(len as u16).to_le_bytes());
        out[start + 18..start + 20].copy_from_slice(&(name.len() as u16).to_le_bytes());
        out[start + 20] = linux[at + 18];
        out[start + 21..start + 21 + name.len()].copy_from_slice(name);
        at += reclen;
    }
    out
}

/// How many bytes of Linux records to ask for so that their Darwin form
/// fits `len`: a Darwin record is at most eight bytes longer, and a Linux
/// one is at least twenty-four.
pub fn dirents_room(len: u64) -> u64 {
    len / 4 * 3
}

/// Darwin's resource limit numbers as the kernel's (`RLIMIT_*`): the first
/// five agree, then Darwin has `AS` (5), `MEMLOCK` (6), `NPROC` (7) and
/// `NOFILE` (8) where Linux has `RSS`, `NPROC`, `NOFILE` and `MEMLOCK`, with
/// `AS` at 9.
pub fn rlimit(which: u64) -> Option<u64> {
    // `_RLIMIT_POSIX_FLAG`: the caller wants POSIX's behaviour, the same one.
    Some(match which & !0x1000 {
        w @ 0..=4 => w,
        5 => 9,
        6 => 8,
        7 => 6,
        8 => 7,
        _ => return None,
    })
}

/// Darwin's `RLIM_INFINITY`: the largest signed value, not all ones.
pub const RLIM_INFINITY: u64 = i64::MAX as u64;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_numbers_map_both_ways() {
        // SIGBUS, SIGUSR1, SIGCHLD, SIGSYS, SIGSTOP by both names.
        for (l, d) in [(7, 10), (10, 30), (12, 31), (17, 20), (31, 12), (19, 17), (18, 19), (20, 18), (23, 16), (29, 23)] {
            assert_eq!(sig_to_darwin(l), d, "linux {l}");
            assert_eq!(sig_to_linux(d), l, "darwin {d}");
        }
        for s in 1..=31 {
            assert_eq!(sig_to_linux(sig_to_darwin(s)), s);
            assert_eq!(sig_to_darwin(sig_to_linux(s)), s);
        }
        assert_eq!(sig_to_darwin(34), 0);
        assert_eq!(sig_to_linux(32), 0);
        // SIGINT and SIGUSR1 as a set.
        assert_eq!(sigset_to_linux(1 << 1 | 1 << 29), 1 << 1 | 1 << 9);
        assert_eq!(sigset_to_darwin(1 << 1 | 1 << 9), 1 << 1 | 1 << 29);
        // Killed by SIGUSR1; stopped by SIGTSTP; exited 3.
        assert_eq!(wait_status_to_darwin(10), 30);
        assert_eq!(wait_status_to_darwin(0x147f), 0x127f);
        assert_eq!(wait_status_to_darwin(0x300), 0x300);
    }

    #[test]
    fn error_numbers_map_both_ways() {
        assert_eq!(errno_to_darwin(Errno::ENOENT), 2);
        assert_eq!(errno_to_darwin(Errno::EAGAIN), 35);
        assert_eq!(errno_to_darwin(Errno::EDEADLK), 11);
        assert_eq!(errno_to_darwin(Errno::ENOSYS), 78);
        assert_eq!(errno_to_darwin(Errno::ENOTEMPTY), 66);
        assert_eq!(errno_to_darwin(Errno::ETIMEDOUT), 60);
        assert_eq!(errno_to_darwin(Errno(999)), 5);
        assert_eq!(errno_from_darwin(35), Errno::EAGAIN);
        assert_eq!(errno_from_darwin(66), Errno::ENOTEMPTY);
        assert_eq!(errno_from_darwin(102), Errno::EOPNOTSUPP);
        for (l, d) in ERRNO {
            assert_eq!(errno_from_darwin(*d as i32), Errno(*l));
        }
    }

    #[test]
    fn open_flags_map_both_ways() {
        let d = 1 | o::CREAT | o::TRUNC | o::CLOEXEC | o::DIRECTORY | o::NONBLOCK;
        let k = oflags::O_WRONLY | oflags::O_CREAT | oflags::O_TRUNC | oflags::O_CLOEXEC | oflags::O_DIRECTORY | oflags::O_NONBLOCK;
        assert_eq!(oflags_to_kernel(d), k);
        assert_eq!(oflags_to_darwin(k), d);
        assert_eq!(dirfd(AT_FDCWD as u32 as u64), LINUX_AT_FDCWD);
        assert_eq!(dirfd(5), 5);
        assert_eq!(at_flags(at::SYMLINK_NOFOLLOW | at::REMOVEDIR), 0x300);
    }

    #[test]
    fn directory_records_are_repacked() {
        // One Linux record: inode 7, offset 1, type 8 (a regular file), "ab".
        let mut l = vec![0u8; 24];
        l[0] = 7;
        l[8] = 1;
        l[16] = 24;
        l[18] = 8;
        l[19] = b'a';
        l[20] = b'b';
        let d = dirents(&l);
        assert_eq!(d.len(), 24);
        assert_eq!((d[0], d[8]), (7, 1));
        assert_eq!(u16::from_le_bytes([d[16], d[17]]), 24);
        assert_eq!(u16::from_le_bytes([d[18], d[19]]), 2);
        assert_eq!(d[20], 8);
        assert_eq!(&d[21..24], b"ab\0");
        assert!(dirents(&l).len() as u64 <= 32 && dirents_room(32) >= 24);
    }
}
