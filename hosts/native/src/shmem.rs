//! Memory a fork shares: the pages of the parent's `MAP_SHARED` mappings
//! (anonymous, `shm_open`, `memfd`, a file; [`gex_kernel::mem::Share`]) are
//! made the child's pages too, so a store by either is the other's at once
//! and atomics and futexes on them work between the two.
//!
//! The processes of this host are one host process, each guest process a
//! wasm memory of its own. At the fork the child's range is remapped onto
//! the parent's pages ([`gex_mem::GuestMemory::share_pages`]: on macOS
//! `mach_vm_remap` with no copy, after which the two ranges are one memory
//! object); an unmap of it, a mapping over it or an exec gives the range
//! private zero pages again first ([`gex_mem::unshare_pages`], the kernel's
//! [`gex_kernel::mem::set_unshare_hook`]). A futex word there is keyed by
//! the share, not the address, in a queue the system has (the kernel's
//! `FutexCx::keyed`), so a wake in one process ends a wait in the other.
//!
//! What it costs: nothing for a process that maps nothing shared (one test
//! of an empty list at a fork, one load at a futex call); at a fork, one
//! remap call per shared mapping, with no copy of its pages; at an unmap of
//! one, an `mmap` per host page.
//!
//! A mapping made later is joined too: a `MAP_SHARED` mapping of a file
//! (`shm_open`'s object, a regular file) that another process already maps
//! shared is remapped onto that process's pages at the `mmap`
//! (`MapIo::join_shared_file`: the same file, a host page of it at the same
//! offset in both), and from then on is one memory with it like a fork's.
//! A forked child's own `shm_open` of its parent's name and a process
//! `posix_spawn` started that opens the name itself (IOSurface's surfaces)
//! both see the other's stores while both run. Only the overlap on whole
//! host pages at the same phase joins; the rest is the process's copy.
//!
//! What it does not do: share a host page (16 KiB here) only partly inside a
//! shared mapping -- the kernel places one on whole host pages for this
//! (`MapIo::place_shared`), but one a program placed itself (`MAP_FIXED`)
//! may begin or end inside one, and that page is the child's copy; move a
//! shared mapping with `mremap` and keep it shared; join a file mapped
//! shared by no other process at the time (it is the first, and is joined
//! by those after it); and share between the processes of the page. There
//! each process is a wasm memory in a Worker of its own, and a memory
//! cannot be mapped into another's: what the page can do for shared memory
//! is copies kept in step at the points the kernel sees (`msync`, `munmap`,
//! exit, a file lock, the shared-memory write-back it does now), not one
//! memory -- a store is not seen by another process until then, and an
//! atomic or a futex between two processes' copies is not one. A page whose
//! processes are threads of one shared `WebAssembly.Memory` could, at the
//! price of one address space for all of them; it is not built. Nor does
//! it share `__ulock` with the shared flag, whose queues are each
//! process's. (Darwin's process-shared psynch objects
//! are the system's, keyed by share: `gex_kernel::darwin::psynch`.)

use gex_mem::GuestMemory;

/// Say what this host needs of the kernel: shared mappings placed on its
/// pages, and the undoing of a share.
pub(crate) fn install() {
    if cfg!(target_os = "macos") {
        gex_kernel::mem::set_share_granule(gex_mem::host_page());
        gex_kernel::mem::set_unshare_hook(gex_mem::unshare_pages);
    }
}

/// The part of `[start, end)` on whole host pages.
fn inner(start: u64, end: u64, page: u64) -> Option<(u64, u64)> {
    let (s, e) = (start.div_ceil(page) * page, end & !(page - 1));
    (s < e).then_some((s, e))
}

/// At a fork, with the parent's threads held and the child not yet run: the
/// child's pages of each of the parent's shares (`shares`) made the
/// parent's own. Which were.
pub(crate) fn share(shares: &[gex_kernel::mem::Share], parent: &GuestMemory, child: &GuestMemory) -> Vec<bool> {
    if !cfg!(target_os = "macos") {
        return vec![false; shares.len()];
    }
    let page = gex_mem::host_page();
    shares
        .iter()
        .map(|s| {
            let Some((a, b)) = inner(s.start, s.end, page) else { return false };
            match child.share_pages(parent, a, b) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("gex: fork: the child could not be given {:#x}..{:#x} shared ({e}): it has a copy", s.start, s.end);
                    false
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_inner_part_is_on_whole_pages() {
        assert_eq!(super::inner(0x4000, 0x14000, 0x4000), Some((0x4000, 0x14000)));
        assert_eq!(super::inner(0x5000, 0x13000, 0x4000), Some((0x8000, 0x10000)));
        assert_eq!(super::inner(0x5000, 0x7000, 0x4000), None);
    }
}
