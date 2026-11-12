//! `signalfd4`: a process's pending signals as a descriptor to read and
//! poll. bubblewrap's monitor waits on one for its child's `SIGCHLD` (with
//! the signal blocked), and an event loop (systemd's, GLib's) can too.
//!
//! The descriptor is readable while a signal of its set is pending for the
//! thread that made it or for its process; a read takes each such signal
//! once, lowest first, as a `struct signalfd_siginfo` (128 bytes), the way
//! `sigtimedwait` takes one ([`crate::signal::Signals::claim_in`]). A
//! waiter wakes at each signal raised for the process
//! ([`crate::signal::ProcSig::raised_queue`]). What it does not do: a
//! thread-directed signal of another thread of the process is not seen
//! (only the maker's are), and a descriptor passed to another process reads
//! its maker's signals, not the reader's.

use crate::errno::{Errno, SysResult};
use crate::signal::Signals;
use crate::vfs::{FileHandle, FileType, Metadata};
use std::sync::atomic::{AtomicU64, Ordering};

/// The size of one `struct signalfd_siginfo`.
pub const SIGINFO: usize = 128;

pub struct SignalFd {
    signals: Signals,
    /// The set (bit `n - 1` for signal `n`); `SIGKILL` and `SIGSTOP` are
    /// never in it.
    mask: AtomicU64,
}

impl std::fmt::Debug for SignalFd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SignalFd({:#x})", self.mask.load(Ordering::Relaxed))
    }
}

/// `SIGKILL` and `SIGSTOP`: never a signalfd's, as Linux drops them.
const UNWATCHABLE: u64 = (1 << 8) | (1 << 18);

impl SignalFd {
    pub fn new(signals: Signals, mask: u64) -> SignalFd {
        SignalFd { signals, mask: AtomicU64::new(mask & !UNWATCHABLE) }
    }

    /// `signalfd4` on a descriptor that is one already: its new set.
    pub fn set_mask(&self, mask: u64) {
        self.mask.store(mask & !UNWATCHABLE, Ordering::Release);
    }

    fn ready(&self) -> u64 {
        self.signals.pending_raw() & self.mask.load(Ordering::Acquire)
    }
}

impl FileHandle for SignalFd {
    fn read_at(&self, _offset: u64, buf: &mut [u8]) -> SysResult {
        if buf.len() < SIGINFO {
            return Err(Errno::EINVAL);
        }
        let mut done = 0;
        while buf.len() - done >= SIGINFO {
            let Some((signal, source)) = self.signals.claim_in(self.mask.load(Ordering::Acquire)) else { break };
            // struct signalfd_siginfo: ssi_signo, ssi_errno, ssi_code,
            // ssi_pid, ssi_uid, ... (all little-endian, fixed layout on
            // every ABI).
            let out = &mut buf[done..done + SIGINFO];
            out.fill(0);
            out[0..4].copy_from_slice(&signal.to_le_bytes());
            let (code, pid, uid) = match source {
                Some(s) => (s.code, s.pid, s.uid),
                None => (0x80, 0, 0), // SI_KERNEL
            };
            out[8..12].copy_from_slice(&code.to_le_bytes());
            out[12..16].copy_from_slice(&(pid as u32).to_le_bytes());
            out[16..20].copy_from_slice(&uid.to_le_bytes());
            done += SIGINFO;
        }
        if done == 0 {
            // Taken by another reader between the poll and here.
            return Err(Errno::EAGAIN);
        }
        Ok(done as u64)
    }

    fn write_at(&self, _offset: u64, _buf: &[u8]) -> SysResult {
        Err(Errno::EINVAL)
    }

    fn poll_readable(&self) -> bool {
        self.ready() != 0
    }

    fn wait_queue(&self) -> u32 {
        self.signals.process().raised_queue()
    }

    fn as_signalfd(&self) -> Option<&SignalFd> {
        Some(self)
    }

    fn metadata(&self) -> Result<Metadata, Errno> {
        Ok(Metadata { file_type: FileType::Regular, size: 0, mode: 0o600, ino: 1, mtime: 0, nlink: 1, rdev: 0, times: Default::default() })
    }
}
