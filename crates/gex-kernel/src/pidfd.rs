//! Process descriptors (`pidfd`): `clone`'s `CLONE_PIDFD`, `clone3`'s
//! `pidfd` field and `pidfd_open` make one; `waitid(P_PIDFD)` waits for its
//! process, `pidfd_send_signal` signals it, and `poll`/`epoll` report it
//! readable once the process has exited (a zombie or reaped) and hung up
//! once it is reaped, as Linux does. Qt's `QProcess` (forkfd) starts its
//! children with `CLONE_VFORK | CLONE_VM | CLONE_PIDFD` when the kernel has
//! them, and with a plain `fork` otherwise.
//!
//! The descriptor names a process by its pid in the system's table
//! ([`ProcTable`]). Its readiness changes when the process exits, which
//! notifies the process's own wait queue (and its parent's): the
//! descriptor's queue is the process's, taken when it is made.

use crate::errno::{Errno, SysResult};
use crate::proc::ProcTable;
use crate::vfs::{FileHandle, FileType, Metadata};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The name a process descriptor has (`/proc/self/fd`'s link).
pub const PATH: &str = "anon_inode:[pidfd]";

/// `waitid`'s `P_PIDFD`.
pub const P_PIDFD: u32 = 3;

/// One process descriptor.
pub struct PidFd {
    pid: i32,
    procs: Arc<ProcTable>,
    queue: u32,
    /// Once seen exited it stays so: an exited process does not come back.
    exited: AtomicBool,
    /// Whether the process has ended (its status filled), taken when the
    /// descriptor is made (none for a process not in the table then, or a
    /// table elsewhere): until it is set the process is alive, and neither
    /// readiness asks the table, whose lock every system call of every
    /// process takes too (libdispatch's process sources poll theirs at each
    /// wakeup of a kqueue).
    ended: Option<Arc<AtomicBool>>,
}

impl std::fmt::Debug for PidFd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PidFd").field("pid", &self.pid).finish()
    }
}

impl PidFd {
    pub fn new(pid: i32, procs: Arc<ProcTable>) -> PidFd {
        let queue = procs.queue(pid);
        let ended = procs.ended_flag(pid);
        PidFd { pid, procs, queue, exited: AtomicBool::new(false), ended }
    }

    /// The process it names.
    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// (exited, reaped): whether the process has exited, and whether it is
    /// gone from the table too.
    fn state(&self) -> (bool, bool) {
        match self.procs.exited(self.pid) {
            None => (true, true),
            Some(zombie) => (zombie || self.exited.load(Ordering::Acquire), false),
        }
    }

    fn exited(&self) -> bool {
        if self.exited.load(Ordering::Acquire) {
            return true;
        }
        if self.ended.as_ref().is_some_and(|e| !e.load(Ordering::Acquire)) {
            return false;
        }
        let (exited, _) = self.state();
        if exited {
            self.exited.store(true, Ordering::Release);
        }
        exited
    }
}

impl FileHandle for PidFd {
    /// Linux has no read of a process descriptor.
    fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> SysResult {
        Err(Errno::EINVAL)
    }

    fn write_at(&self, _offset: u64, _buf: &[u8]) -> SysResult {
        Err(Errno::EINVAL)
    }

    fn metadata(&self) -> Result<Metadata, Errno> {
        // An anonymous inode, reported as a fifo like eventfd's: the browser
        // scheduler keys "a wait another Worker satisfies" off fifo.
        Ok(Metadata { owner: None, file_type: FileType::Fifo, size: 0, mode: 0o600, ino: 0, mtime: 0, nlink: 1, rdev: 0, times: Default::default() })
    }

    fn is_stream(&self) -> bool {
        true
    }

    fn poll_readable(&self) -> bool {
        self.exited()
    }

    fn poll_writable(&self) -> bool {
        false
    }

    fn poll_hup(&self) -> bool {
        // (Not reaped before it has exited.)
        self.exited() && self.state().1
    }

    fn wait_queue(&self) -> u32 {
        self.queue
    }

    fn pidfd(&self) -> Option<i32> {
        Some(self.pid)
    }
}

/// A new process descriptor's open file for `pid`: always close-on-exec,
/// `O_NONBLOCK` from `flags`.
pub fn open_file(pid: i32, procs: &Arc<ProcTable>, nonblock: bool) -> Arc<crate::file::OpenFile> {
    use crate::fs::oflags;
    crate::file::OpenFile::new(
        Arc::new(PidFd::new(pid, Arc::clone(procs))),
        PATH.to_string(),
        oflags::O_RDWR | if nonblock { oflags::O_NONBLOCK } else { 0 },
        true,
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_once_the_process_exits_and_hung_up_once_reaped() {
        let procs = Arc::new(ProcTable::new());
        let child = procs.spawn(crate::proc::INIT_PID);
        let p = PidFd::new(child, Arc::clone(&procs));
        assert_eq!(p.pid(), child);
        assert!(!p.poll_readable());
        assert!(!p.poll_hup());
        procs.exit(child, 0);
        assert!(p.poll_readable());
        assert!(!p.poll_hup());
        procs.reap(child);
        assert!(p.poll_readable());
        assert!(p.poll_hup());
        let mut b = [0u8; 8];
        assert_eq!(p.read_at(0, &mut b), Err(Errno::EINVAL));
    }
}
