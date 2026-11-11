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
//! What it does not do: share a mapping made after the fork (a child's own
//! `shm_open` of the same name, a file mapped again in another process),
//! which is still each process's copy, met at `msync`, `munmap` and file
//! locks as before; share a host page (16 KiB here) only partly inside a
//! shared mapping -- the kernel places one on whole host pages for this
//! (`MapIo::place_shared`), but one a program placed itself (`MAP_FIXED`)
//! may begin or end inside one, and that page is the child's copy; move a
//! shared mapping with `mremap` and keep it shared; share between the
//! processes of the page (each a wasm memory in a Worker of its own, which
//! cannot be made one another's); and Darwin's own process-shared locks
//! (psynch, `__ulock` with the shared flag), whose queues are each
//! process's: memory and atomics are shared for a Mac program, but a
//! `PTHREAD_PROCESS_SHARED` mutex contended across processes is not.

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
