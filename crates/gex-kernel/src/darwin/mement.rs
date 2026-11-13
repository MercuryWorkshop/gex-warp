//! Named memory entries: `mach_make_memory_entry_64` and the `mach_vm_map`
//! of one, which is how a Darwin task names memory for another to map and
//! see the same pages (hidd's event queues, which WindowServer maps; the
//! buffer a client of logd's firehose hands it).
//!
//! An entry is a port that names a range of a memory object, with the
//! protection it may be mapped with. Here the object is the kernel's own
//! shared memory, a `memfd` ([`crate::unix::MemFd`]), and mapping an entry
//! is a `MAP_SHARED` mapping of it: the mechanism `shm_open`'s objects have,
//! so what a task maps of an entry is shared with the other tasks that map
//! it as a shared file is ([`crate::syscalls::MapIo::join_shared_file`]:
//! natively one set of host pages; on the page, copies kept in step where
//! the kernel sees them, and an entry does not leave its task there, since
//! a port does not).
//!
//! - `MAP_MEM_NAMED_CREATE` makes a new object of the size asked, zeroed.
//! - An entry of memory the task has already is of the object that memory
//!   is a shared mapping of, when it is one (a mapping of an entry, of
//!   `shm_open`'s); and when it is the task's private memory, that memory
//!   is made a shared mapping of a new object in place: its bytes copied
//!   into the object and the object mapped over them, at the same address
//!   with the same protections. Anonymous memory cannot be made shared after
//!   the fact otherwise (the host's pages of a wasm memory are its own), and
//!   so the task's memory is from then on what Darwin's is once named: an
//!   object others may map. The object's offsets are laid out as the task's
//!   addresses are on the host's pages (16 KiB on an Apple processor, a
//!   guest's may be 4 KiB), and where the host pages around the range are
//!   the task's own anonymous memory they are taken in too, so that the
//!   whole range is on pages a host can share.
//! - `MAP_MEM_VM_COPY` names a copy of the memory as it is now.
//! - A parent entry: a range of its object.
//!
//! A mapping of an entry is placed so that its address is on the host's
//! page where its offset in the object is, and covers the object's host
//! pages around it, so that another task's mapping of it is joined page for
//! page. A mapping of an entry a task maps already is that mapping again.

use super::abi;
use super::mach::*;
use super::port::{Kind, Port};
use super::*;
use crate::file::OpenFile;
use crate::syscalls::nr;
use std::collections::HashMap;
use std::sync::Mutex;

/// `MAP_MEM_*`, the flags beside a protection in `permission`.
const MAP_MEM_ONLY: u32 = 0x01_0000;
const MAP_MEM_NAMED_CREATE: u32 = 0x02_0000;
const MAP_MEM_PURGABLE: u32 = 0x04_0000;
const MAP_MEM_USE_DATA_ADDR: u32 = 0x10_0000;
const MAP_MEM_VM_COPY: u32 = 0x20_0000;
const MAP_MEM_4K_DATA_ADDR: u32 = 0x80_0000;
/// The flags XNU 4903 knows (`MAP_MEM_FLAGS_ALL`); any other is refused.
const MAP_MEM_FLAGS_ALL: u32 = 0x00ff_e000;
const MAP_MEM_FLAGS_MASK: u32 = 0x00ff_ff00;
/// `VM_PROT_IS_MASK`: the protections are the most asked for, not all.
const VM_PROT_IS_MASK: u32 = 0x40;
/// `KERN_INVALID_OBJECT`: a port that names no memory to map.
pub(super) const KERN_INVALID_OBJECT: u64 = 29;

/// One entry: `size` bytes of `file` from `offset`, to be mapped with no
/// more than `prot`.
pub(super) struct Entry {
    pub file: Arc<OpenFile>,
    pub offset: u64,
    pub size: u64,
    pub prot: u32,
}

/// The entries, by the handle their port carries ([`Kind::MemEntry`]); one
/// goes when its port does ([`forget`]).
static ENTRIES: Mutex<Option<(u32, HashMap<u32, Arc<Entry>>)>> = Mutex::new(None);

fn table() -> std::sync::MutexGuard<'static, Option<(u32, HashMap<u32, Arc<Entry>>)>> {
    ENTRIES.lock().unwrap_or_else(|e| e.into_inner())
}

/// A port for a new entry.
fn port_for(e: Entry) -> Arc<Port> {
    let mut t = table();
    let (next, map) = t.get_or_insert_with(|| (1, HashMap::new()));
    let h = *next;
    *next = next.wrapping_add(1).max(1);
    map.insert(h, Arc::new(e));
    Port::new(Kind::MemEntry(h))
}

/// The entry a port's handle names.
pub(super) fn entry(h: u32) -> Option<Arc<Entry>> {
    table().as_ref().and_then(|(_, m)| m.get(&h).cloned())
}

/// The entry's port is gone: the entry with it (what is mapped of its
/// object stays mapped).
pub(super) fn forget(h: u32) {
    if let Some((_, m)) = table().as_mut() {
        m.remove(&h);
    }
}

/// A new object of `len` zeroes, and more to the end of the host's page,
/// so that what is mapped of it is whole host pages a host can share.
fn new_object(len: u64) -> Result<Arc<OpenFile>, u64> {
    let memfd = Arc::new(crate::unix::MemFd::new("mach memory entry"));
    memfd.reserve(len.next_multiple_of(crate::mem::share_granule().max(abi::PAGE_SIZE))).map_err(|_| KERN_RESOURCE_SHORTAGE)?;
    let handle: Arc<dyn crate::vfs::FileHandle> = memfd;
    Ok(OpenFile::new(handle, "/memfd:mach memory entry".to_string(), crate::fs::oflags::O_RDWR, true, true))
}

/// `KERN_RESOURCE_SHORTAGE`
const KERN_RESOURCE_SHORTAGE: u64 = 6;

impl Kernel {
    /// `mach_make_memory_entry_64(task, in-out size, offset, permission,
    /// out entry, parent)` of this task: the entry's port and its size, or
    /// the routine's error.
    pub(super) fn darwin_make_memory_entry(&mut self, m: &mut GuestMem<'_>, size: u64, offset: u64, permission: u32, parent: Option<&Arc<Port>>) -> Result<(Arc<Port>, u64), u64> {
        if (permission & MAP_MEM_FLAGS_MASK) & !MAP_MEM_FLAGS_ALL != 0 {
            return Err(KERN_INVALID_VALUE);
        }
        let page = abi::PAGE_SIZE;
        let mut prot = permission & 7;
        let masked = permission & VM_PROT_IS_MASK != 0;
        if permission & (MAP_MEM_ONLY | MAP_MEM_USE_DATA_ADDR | MAP_MEM_4K_DATA_ADDR) != 0 {
            // (A change of caching, and an entry that keeps the data's own
            // offset in its page: none asks for these yet.)
            return Err(KERN_INVALID_ARGUMENT);
        }
        let start = offset & !(page - 1);
        let end = offset.checked_add(size).and_then(|e| e.checked_next_multiple_of(page)).ok_or(KERN_INVALID_ARGUMENT)?;
        let len = end - start;
        if permission & MAP_MEM_NAMED_CREATE != 0 {
            if permission & MAP_MEM_PURGABLE != 0 && prot & crate::mem::prot::WRITE == 0 {
                return Err(KERN_INVALID_ARGUMENT);
            }
            let file = new_object(len)?;
            return Ok((port_for(Entry { file, offset: 0, size: len, prot }), len));
        }
        // A range of a parent entry's object.
        if let Some(parent) = parent {
            let Kind::MemEntry(h) = parent.kind else { return Err(KERN_INVALID_ARGUMENT) };
            let p = entry(h).ok_or(KERN_INVALID_ARGUMENT)?;
            if end > p.size {
                return Err(KERN_INVALID_ARGUMENT);
            }
            if masked {
                prot &= p.prot;
            } else if prot & p.prot != prot {
                return Err(KERN_PROTECTION_FAILURE);
            }
            let e = Entry { file: p.file.clone(), offset: p.offset + start, size: len, prot };
            return Ok((port_for(e), len));
        }
        // The task's own memory: every page of it mapped, with what the
        // entry is to allow.
        if len == 0 {
            return Err(KERN_INVALID_ARGUMENT);
        }
        let (lo, hi) = (gex_abi::canon(start), gex_abi::canon(start) + len);
        let parts: Vec<(u64, u64, u32, bool)> = {
            let mm = self.mem.lock();
            let mut parts = Vec::new();
            let mut at = lo;
            while at < hi {
                let Some(v) = mm.vmas().iter().find(|v| v.start <= at && at < v.end) else { return Err(KERN_INVALID_ADDRESS) };
                parts.push((at, v.end.min(hi), v.prot & 7, v.origin.is_none()));
                at = v.end;
            }
            parts
        };
        let have = parts.iter().fold(7, |p, &(_, _, q, _)| p & q);
        if masked {
            prot &= have;
        } else if prot & have != prot {
            return Err(KERN_PROTECTION_FAILURE);
        }
        // Of an object the memory is a mapping of already.
        let mapped = {
            let mm = self.mem.lock();
            mm.log.shared_maps.iter().find(|s| s.start <= lo && hi <= s.start + s.len).map(|s| (s.file.clone(), s.offset + (lo - s.start)))
        };
        if let Some((file, offset)) = mapped {
            return Ok((port_for(Entry { file, offset, size: len, prot }), len));
        }
        let bytes = m.read(lo, len as usize).map_err(|_| KERN_INVALID_ADDRESS)?.to_vec();
        if permission & MAP_MEM_VM_COPY != 0 {
            let file = new_object(len)?;
            file.handle.write_at(0, &bytes).map_err(|_| KERN_RESOURCE_SHORTAGE)?;
            return Ok((port_for(Entry { file, offset: 0, size: len, prot }), len));
        }
        let (file, offset) = self.darwin_share_in_place(m, lo, hi, &parts)?;
        Ok((port_for(Entry { file, offset, size: len, prot }), len))
    }

    /// The task's private memory `[lo, hi)` (its mappings `parts`: start,
    /// end, protection, anonymous) made a shared mapping of a new object in
    /// place, its bytes kept: the object, and where `lo` is in it.
    fn darwin_share_in_place(&mut self, m: &mut GuestMem<'_>, lo: u64, hi: u64, parts: &[(u64, u64, u32, bool)]) -> Result<(Arc<OpenFile>, u64), u64> {
        let g = crate::mem::share_granule().max(abi::PAGE_SIZE);
        // The host's pages around the range too, where they are the task's
        // anonymous memory and nothing's code: shared whole, or not at all.
        let (glo, ghi) = (lo & !(g - 1), hi.next_multiple_of(g));
        let (slo, shi) = {
            let mm = self.mem.lock();
            let plain = |a: u64, b: u64| {
                let mut at = a;
                while at < b {
                    match mm.vmas().iter().find(|v| v.start <= at && at < v.end) {
                        Some(v) if v.origin.is_none() && v.code.is_none() && v.prot & crate::mem::prot::EXEC == 0 => at = v.end,
                        _ => return false,
                    }
                }
                !mm.log.shared_maps.iter().any(|s| s.start < b && a < s.start + s.len)
            };
            if plain(glo, ghi) { (glo, ghi) } else { (lo, hi) }
        };
        let mut parts = parts.to_vec();
        if (slo, shi) != (lo, hi) {
            let mm = self.mem.lock();
            parts = mm.vmas().iter().filter(|v| v.start < shi && slo < v.end).map(|v| (v.start.max(slo), v.end.min(shi), v.prot & 7, true)).collect();
        }
        // The object's offsets are the host page's: `slo` is at its offset
        // in its host page.
        let at = slo % g;
        let file = new_object((at + (shi - slo)).next_multiple_of(g))?;
        let bytes = m.read(slo, (shi - slo) as usize).map_err(|_| KERN_INVALID_ADDRESS)?.to_vec();
        file.handle.write_at(at, &bytes).map_err(|_| KERN_RESOURCE_SHORTAGE)?;
        let rw = (crate::mem::prot::READ | crate::mem::prot::WRITE) as u64;
        let fd = self.fds.insert(file.clone(), true).map_err(|_| KERN_RESOURCE_SHORTAGE)? as i32;
        let r = self.map_io().call(m, nr::MMAP, [slo, shi - slo, rw, crate::mem::flags::SHARED | crate::mem::flags::FIXED, fd as u64, at]);
        let _ = self.fds.close(fd);
        r.unwrap_or(Err(Errno::EINVAL)).map_err(|e| kern(Err(e)))?;
        for &(a, b, p, _) in &parts {
            if p as u64 != rw {
                let _ = self.map_io().call(m, nr::MPROTECT, [a, b - a, p as u64, 0, 0, 0]);
            }
        }
        Ok((file, at + (lo - slo)))
    }

    /// `mach_vm_map` (and `vm_map`, `vm_map_64`) of the entry `e`: `size`
    /// bytes of it from `offset` (all of it from there for none), at `want`
    /// or, with `VM_FLAGS_ANYWHERE`, where there is room on a boundary of
    /// `mask + 1`; shared, or a copy (`copy`), with `cur` as its protection.
    /// The task's address of it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn darwin_map_entry(&mut self, m: &mut GuestMem<'_>, e: &Entry, want: u64, size: u64, mask: u64, flags: u64, offset: u64, copy: bool, cur: u32) -> Result<u64, u64> {
        const VM_FLAGS_ANYWHERE: u64 = 1;
        const VM_FLAGS_OVERWRITE: u64 = 0x4000;
        let page = abi::PAGE_SIZE;
        if offset % page != 0 || offset > e.size {
            return Err(KERN_INVALID_ARGUMENT);
        }
        let size = if size == 0 { e.size - offset } else { size };
        let len = size.checked_next_multiple_of(page).ok_or(KERN_INVALID_ARGUMENT)?;
        if len == 0 || offset + len > e.size {
            return Err(KERN_INVALID_ARGUMENT);
        }
        // (No more than the entry allows, unless as a copy.)
        if !copy && cur & e.prot != cur {
            return Err(KERN_INVALID_RIGHT);
        }
        let foff = e.offset + offset;
        let g = crate::mem::share_granule().max(page);
        // A copy is of what the object holds: what this task has stored in
        // its mappings of it is written back first.
        if copy {
            let mine: Vec<(u64, u64)> = {
                let mm = self.mem.lock();
                mm.log.shared_maps.iter().filter(|s| s.file.same_file(&e.file)).map(|s| (s.start, s.len)).collect()
            };
            for (at, n) in mine {
                let _ = self.map_io().call(m, nr::MSYNC, [at, n, 4, 0, 0, 0]);
            }
        }
        let fd = self.fds.insert(e.file.clone(), true).map_err(|_| KERN_RESOURCE_SHORTAGE)? as i32;
        let r = (|| {
            let kind = if copy { crate::mem::flags::PRIVATE } else { crate::mem::flags::SHARED };
            if flags & VM_FLAGS_ANYWHERE == 0 {
                let at = gex_abi::canon(want) & !(page - 1);
                if at.checked_add(len).is_none_or(|end| end > m.limit() as u64) {
                    return Err(KERN_INVALID_ADDRESS);
                }
                let fixed = if flags & VM_FLAGS_OVERWRITE != 0 { crate::mem::flags::FIXED } else { crate::mem::flags::FIXED_NOREPLACE };
                let r = self.map_io().call(m, nr::MMAP, [at, len, cur as u64, kind | fixed, fd as u64, foff]);
                return r.unwrap_or(Err(Errno::EINVAL)).map_err(|e| kern(Err(e)));
            }
            // What this task maps of the object already: that mapping again.
            let again = !copy && {
                let mm = self.mem.lock();
                mm.log.shared_maps.iter().any(|s| s.file.same_file(&e.file) && s.offset <= foff && foff + len <= s.offset + s.len)
            };
            if again {
                let r = self.map_io().call(m, nr::MMAP, [0, len, cur as u64, kind, fd as u64, foff]);
                return r.unwrap_or(Err(Errno::EINVAL)).map_err(|e| kern(Err(e)));
            }
            // The object's host pages around the range, at an address on
            // the host's page where they are in the object, so that what
            // another task maps of them is these pages
            // (`MapIo::join_shared_file`); the range's address in them is
            // the answer. (A boundary coarser than the host's page is kept
            // instead, where it is asked for and cannot be both.)
            let (flo, fhi) = (foff & !(g - 1), (foff + len).next_multiple_of(g).min(e.file.handle.metadata().map(|md| md.size).unwrap_or(0).max(foff + len)));
            let whole = !copy && g > page && (mask.wrapping_add(1)).max(page) <= g;
            let (flo, fhi) = if whole { (flo, fhi) } else { (foff, foff + len) };
            let align = if whole { g - 1 } else { mask };
            let at = self.darwin_place(gex_abi::canon(want), fhi - flo, cur, false, align).map_err(|e| kern(Err(e)))?;
            let r = self.map_io().call(m, nr::MMAP, [at, fhi - flo, cur as u64, kind | crate::mem::flags::FIXED_NOREPLACE, fd as u64, flo]);
            r.unwrap_or(Err(Errno::EINVAL)).map_err(|e| kern(Err(e)))?;
            Ok(at + (foff - flo))
        })();
        let _ = self.fds.close(fd);
        r.map(gex_abi::guest_addr)
    }
}
