//! Deferred reclamation: memory a task has no use for now and may want
//! again, which it leaves mapped and lists in a ring it shares with the
//! kernel, for the kernel to take back when memory is short
//! (`mach_vm_reclaim`, XNU's `vm_reclaim.c`). libmalloc keeps its freed
//! large regions this way, and says so on the terminal when the kernel has
//! no ring to give it.
//!
//! The ring is the task's memory: a header and entries, each an address, a
//! size and what to do with it (free its pages, or unmap it). The task
//! adds at `tail`; the kernel takes from `head`, having said in `busy` how
//! far it means to go, so a task that wants an entry back (it moves `tail`
//! down to it, then looks at `busy`) and a kernel taking it cannot both
//! have it.
//!
//! Memory is never short here in the way XNU means, so entries are taken
//! back when the task asks (a ring that is full, a ring resized) and each
//! time it comes to say how much it holds
//! (`mach_vm_reclaim_update_kernel_accounting_trap`), which libmalloc does
//! every so often: what is in the ring longer than that is given back.
//!
//! The kernel is told nothing of where the ring is after it made it: the
//! calls name the task. A kernel that does not know (a child made by `fork`
//! on a host that builds the child a kernel of its own) finds it by the
//! mark this leaves in the two header words XNU no longer uses.
use super::*;
use crate::syscalls::{clock_ns, nr};

/// `struct mach_vm_reclaim_ring_s`: where its fields are, and its entries.
mod ring {
    /// The mark, and the ring's own address (XNU's `va_in_buffer` and
    /// `last_accounting_given_to_kernel`, neither used any more).
    pub const MARK: u64 = 0;
    pub const SELF: u64 = 8;
    pub const LEN: u64 = 16;
    pub const HEAD: u64 = 88;
    pub const BUSY: u64 = 96;
    pub const TAIL: u64 = 104;
    pub const ENTRIES: u64 = 128;
    /// An entry: address, 32 bits of size, a byte of what to do.
    pub const ENTRY: u64 = 16;
    pub const DEALLOCATE: u8 = 2;
}

const MARK: u64 = 0x6765_785f_7265_636c;
/// The largest ring (`VM_RECLAIM_MAX_BUFFER_SIZE`).
const MOST: u64 = 128 << 20;
/// How long until the task is next to say what it holds.
const PERIOD: u64 = 10_000_000_000;

impl Kernel {
    /// When the task should next come to say how much it holds
    /// (`mach_absolute_time`'s clock).
    pub(super) fn darwin_reclaim_deadline(&self) -> u64 {
        clock_ns(self.clock, 1) + PERIOD
    }

    /// `mach_vm_deferred_reclamation_buffer_allocate(task, out address, out
    /// deadline, len, max_len)`: the task's ring, with room for `max_len`
    /// entries. A task has one.
    pub(super) fn darwin_reclaim_allocate(&mut self, m: &mut GuestMem<'_>, len: u64, max_len: u64) -> Result<u64, u64> {
        if len == 0 || max_len < len {
            return Err(mach::KERN_INVALID_ARGUMENT);
        }
        let size = (ring::ENTRIES + max_len * ring::ENTRY).next_multiple_of(abi::PAGE_SIZE);
        if size > MOST {
            return Err(mach::KERN_INVALID_ARGUMENT);
        }
        if self.darwin_reclaim_ring(m).is_some() {
            return Err(mach::KERN_NO_SPACE);
        }
        let at = self.mem.mmap_anon(m, 0, size, 3, 0, false, "[reclaim ring]").map_err(|_| mach::KERN_NO_SPACE)?;
        let _ = m.write_u64(at + ring::MARK, MARK);
        let _ = m.write_u64(at + ring::SELF, at);
        self.darwin_reclaim = Some(at);
        Ok(at)
    }

    /// The task's ring, if it has one: where this kernel made it, or where
    /// the mark says one is.
    pub(super) fn darwin_reclaim_ring(&mut self, m: &GuestMem<'_>) -> Option<u64> {
        let marked = |at: u64| m.read_u64(at + ring::MARK) == Ok(MARK) && m.read_u64(at + ring::SELF) == Ok(at);
        if let Some(at) = self.darwin_reclaim.filter(|at| marked(*at)) {
            return Some(at);
        }
        let starts: Vec<u64> = self.mem.lock().vmas().iter().filter(|v| v.prot & 3 == 3 && v.end - v.start >= abi::PAGE_SIZE).map(|v| v.start).collect();
        self.darwin_reclaim = starts.into_iter().find(|at| marked(*at));
        self.darwin_reclaim
    }

    /// The ring's address and its size, for whoever asks
    /// (`mach_vm_deferred_reclamation_buffer_query`).
    pub(super) fn darwin_reclaim_query(&mut self, m: &GuestMem<'_>) -> Option<(u64, u64)> {
        let at = self.darwin_reclaim_ring(m)?;
        let end = self.mem.lock().vma_at(at).map(|v| v.end)?;
        Some((at, end - at))
    }

    /// Take back up to `most` of the ring's entries, oldest first: the
    /// bytes they were. `None`: the task has no ring.
    pub(super) fn darwin_reclaim_take(&mut self, m: &mut GuestMem<'_>, most: u64) -> Option<u64> {
        let at = self.darwin_reclaim_ring(m)?;
        let len = m.read_u32(at + ring::LEN).ok()? as u64;
        let (head, tail) = (m.read_u64(at + ring::HEAD).ok()?, m.read_u64(at + ring::TAIL).ok()?);
        if len == 0 || tail <= head {
            return Some(0);
        }
        // How far this means to go is said first, and where the entries
        // end looked at again after: an entry the task took back in
        // between is the task's.
        let want = head + most.min(tail - head);
        m.write_u64(at + ring::BUSY, want).ok()?;
        let upto = want.min(m.read_u64(at + ring::TAIL).ok()?).max(head);
        if upto != want {
            m.write_u64(at + ring::BUSY, upto).ok()?;
        }
        let mut bytes = 0;
        for id in head..upto {
            let e = at + ring::ENTRIES + (id % len) * ring::ENTRY;
            let (Ok(addr), Ok(size)) = (m.read_u64(e), m.read_u32(e + 8)) else { continue };
            let what = m.read(e + 12, 1).map_or(0, |b| b[0]);
            let size = size as u64;
            // (An entry of nothing was taken back by the task.)
            if addr == 0 || size == 0 || addr % abi::PAGE_SIZE != 0 {
                continue;
            }
            let size = size.next_multiple_of(abi::PAGE_SIZE);
            const MADV_DONTNEED: u64 = 4;
            let call = if what == ring::DEALLOCATE { (nr::MUNMAP, [addr, size, 0, 0, 0, 0]) } else { (nr::MADVISE, [addr, size, MADV_DONTNEED, 0, 0, 0]) };
            if matches!(self.map_io().call(m, call.0, call.1), Some(Ok(_))) {
                bytes += size;
            }
        }
        m.write_u64(at + ring::HEAD, upto).ok()?;
        Some(bytes)
    }
}
