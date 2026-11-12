//! POSIX named semaphores (`sem_open` and the rest), as XNU keeps them: a
//! name in the system's table of semaphores, and a descriptor for each
//! process that has one open (`sem_open` answers the descriptor's number,
//! which libc hands back as its `sem_t *`).
//!
//! The table is the system's: natively every process of this host is in it,
//! so a semaphore one process opens another opens too, and a post in one
//! ends a wait in the other. On the page each process (a Worker of its own)
//! has a table of its own: a name is not shared between processes there.
//!
//! A wait is a read of one count from the descriptor, which blocks as a
//! read of an empty pipe blocks (and is interrupted by a signal, `EINTR`);
//! a post adds one and wakes the waiters. A Mac's unnamed semaphores
//! (`sem_init`) are `ENOSYS` in its libc and never reach here.

use super::*;
use crate::vfs::{FileHandle, FileType, Metadata};
use std::collections::HashMap;
use std::sync::Mutex;

/// `SEM_VALUE_MAX`.
const VALUE_MAX: u32 = 32767;

/// One semaphore: its count, and who waits for it.
pub struct Psem {
    count: Mutex<u32>,
    queue: crate::wait::Queue,
}

impl std::fmt::Debug for Psem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Psem({})", self.count.lock().map_or(0, |c| *c))
    }
}

impl Psem {
    /// `sem_trywait`: one taken, or `EAGAIN`.
    pub fn take(&self) -> Result<(), Errno> {
        let mut c = self.count.lock().unwrap_or_else(|e| e.into_inner());
        if *c == 0 {
            return Err(Errno::EAGAIN);
        }
        *c -= 1;
        Ok(())
    }

    /// `sem_post`: one more, and the waiters told.
    pub fn post(&self) -> Result<(), Errno> {
        let mut c = self.count.lock().unwrap_or_else(|e| e.into_inner());
        if *c >= VALUE_MAX {
            return Err(Errno::EOVERFLOW);
        }
        *c += 1;
        drop(c);
        self.queue.notify();
        Ok(())
    }
}

impl FileHandle for Psem {
    /// A wait's take: one count, as one byte.
    fn read_at(&self, _offset: u64, buf: &mut [u8]) -> SysResult {
        if buf.is_empty() {
            return Ok(0);
        }
        self.take()?;
        buf[0] = 1;
        Ok(1)
    }
    fn write_at(&self, _offset: u64, _buf: &[u8]) -> SysResult {
        Err(Errno::EBADF)
    }
    fn poll_readable(&self) -> bool {
        *self.count.lock().unwrap_or_else(|e| e.into_inner()) > 0
    }
    fn wait_queue(&self) -> u32 {
        self.queue.id()
    }
    fn as_psem(&self) -> Option<&Psem> {
        Some(self)
    }
    fn metadata(&self) -> Result<Metadata, Errno> {
        Ok(Metadata { file_type: FileType::Regular, size: 0, mode: 0o600, ino: 1, mtime: 0, nlink: 1, rdev: 0, times: Default::default() })
    }
}

/// The system's semaphores, by name.
fn table() -> &'static Mutex<HashMap<Vec<u8>, Arc<Psem>>> {
    static T: std::sync::OnceLock<Mutex<HashMap<Vec<u8>, Arc<Psem>>>> = std::sync::OnceLock::new();
    T.get_or_init(Default::default)
}

/// A semaphore's name as XNU takes it: at most `PSEMNAMLEN` (31) bytes.
fn name(m: &GuestMem<'_>, at: u64) -> Result<Vec<u8>, Errno> {
    let n = m.read_cstr(at, 1024)?;
    if n.is_empty() {
        return Err(Errno::EINVAL);
    }
    if n.len() > 31 {
        return Err(Errno::ENAMETOOLONG);
    }
    Ok(n)
}

impl Kernel {
    /// `sem_open(name, oflag, mode, value)`: a descriptor for the named
    /// semaphore, made with `O_CREAT` (and refused with `O_EXCL` when it
    /// is there).
    pub(super) fn darwin_sem_open(&mut self, m: &GuestMem<'_>, x: [u64; 8]) -> SysResult {
        let n = name(m, x[0])?;
        let (oflag, value) = (x[1], x[3] as u32);
        let sem = {
            let mut t = table().lock().unwrap_or_else(|e| e.into_inner());
            match t.get(&n) {
                Some(_) if oflag & abi::o::CREAT != 0 && oflag & abi::o::EXCL != 0 => return Err(Errno::EEXIST),
                Some(s) => Arc::clone(s),
                None if oflag & abi::o::CREAT == 0 => return Err(Errno::ENOENT),
                None => {
                    if value > VALUE_MAX {
                        return Err(Errno::EINVAL);
                    }
                    let s = Arc::new(Psem { count: Mutex::new(value), queue: crate::wait::Queue::new() });
                    t.insert(n.clone(), Arc::clone(&s));
                    s
                }
            }
        };
        let file = crate::file::OpenFile::new(sem, format!("psem:{}", String::from_utf8_lossy(&n)), crate::fs::oflags::O_RDWR, true, true);
        self.fds.insert(file, true)
    }

    /// `sem_unlink(name)`: the name goes; who has it open keeps it.
    pub(super) fn darwin_sem_unlink(&mut self, m: &GuestMem<'_>, at: u64) -> SysResult {
        let n = name(m, at)?;
        table().lock().unwrap_or_else(|e| e.into_inner()).remove(&n).map(|_| 0).ok_or(Errno::ENOENT)
    }

    /// The semaphore a descriptor is, or `EINVAL` (as XNU answers a
    /// `sem_t` that is none).
    pub(super) fn darwin_psem(&self, fd: u64) -> Result<Arc<crate::file::OpenFile>, Errno> {
        let f = self.fds.get(fd as i32).map_err(|_| Errno::EINVAL)?;
        if f.handle.as_psem().is_none() {
            return Err(Errno::EINVAL);
        }
        Ok(f)
    }
}
