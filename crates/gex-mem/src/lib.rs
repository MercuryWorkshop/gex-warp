//! Access to the guest's linear memory.
//!
//! The guest's memory is a wasm *shared* memory, because each guest thread runs
//! in its own wasmtime store and a store-owned memory would be private to one.
//! wasmtime's only accessor is `SharedMemory::data`, returning
//! `&[UnsafeCell<u8>]`: another thread may be writing any byte right now.
//!
//! # Why this crate exists
//!
//! Guest memory is shared mutable state by construction; no Rust type describes
//! it faithfully, and wrapping every access in an atomic would be both wrong
//! (the guest's own atomics decide ordering) and ruinous. A data race in the
//! guest stays one, observed as whatever the hardware does. So the
//! host reads these bytes as an ordinary slice, isolated here so the host itself
//! keeps `#![forbid(unsafe_code)]`.
//!
//! What the host must not assume: that a value read twice is unchanged (every
//! kernel path reads a guest word once and works from the copy); that a slice
//! stays valid across a guest instruction (the borrow lives one host call).

use std::sync::{Arc, Mutex};
use wasmtime::SharedMemory;

/// The guest's linear memory, viewable as bytes.
#[derive(Clone)]
pub struct GuestMemory {
    inner: SharedMemory,
    /// Its place in [`LIVE`] while any handle of it is.
    _live: Arc<Live>,
}

/// The memories alive, by base, with their lengths: what
/// [`unshare_pages`] checks the memory it is named is one.
static LIVE: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());

struct Live(u64);

impl Drop for Live {
    fn drop(&mut self) {
        LIVE.lock().unwrap_or_else(|e| e.into_inner()).retain(|&(b, _)| b != self.0);
    }
}

impl GuestMemory {
    pub fn new(inner: SharedMemory) -> GuestMemory {
        let base = inner.data().as_ptr() as u64;
        LIVE.lock().unwrap_or_else(|e| e.into_inner()).push((base, inner.data_size() as u64));
        GuestMemory { inner, _live: Arc::new(Live(base)) }
    }

    /// The memory's base address in the host, its name for
    /// [`unshare_pages`].
    pub fn base(&self) -> u64 {
        self.inner.data().as_ptr() as u64
    }

    /// Make `[start, end)` of this memory the same pages as that range of
    /// `from`'s, so that a store in either is in both from now on (a fork's
    /// `MAP_SHARED` mappings): the range on whole host pages ([`host_page`]).
    /// Whatever this memory held there is dropped. `Err` with the host's
    /// answer where it cannot (a host other than macOS: not yet).
    pub fn share_pages(&self, from: &GuestMemory, start: u64, end: u64) -> Result<(), i32> {
        let page = host_page();
        if start % page != 0 || end % page != 0 || start >= end || end > self.len() as u64 || end > from.len() as u64 {
            return Err(-1);
        }
        #[cfg(target_os = "macos")]
        {
            let mut at = self.base() + start;
            let (mut cur, mut max) = (0, 0);
            // SAFETY: both ranges are inside live wasm memories (checked
            // above), whose pages the guest owns; this one's are replaced
            // whole, which is the caller's to ask (its process has not run
            // on them, or gives them up).
            let r = unsafe {
                mach::mach_vm_remap(
                    mach::mach_task_self_,
                    &mut at,
                    end - start,
                    0,
                    mach::VM_FLAGS_FIXED | mach::VM_FLAGS_OVERWRITE,
                    mach::mach_task_self_,
                    from.base() + start,
                    0,
                    &mut cur,
                    &mut max,
                    mach::VM_INHERIT_SHARE,
                )
            };
            if r != 0 {
                return Err(r);
            }
            if at != self.base() + start {
                return Err(-2);
            }
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(-1)
        }
    }

    /// Total size in bytes. Fixed for the life of the process: gex sizes the
    /// address space up front and never grows it.
    pub fn len(&self) -> usize {
        self.inner.data_size()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The whole address space as a mutable slice. See the module comment for
    /// why this is not `unsafe`. The lifetime is tied to the borrow of `self`,
    /// in practice one host call.
    #[allow(clippy::mut_from_ref)]
    pub fn bytes(&self) -> &mut [u8] {
        let cells = self.inner.data();
        // SAFETY: `cells` is the guest's address space, which the guest may be
        // writing to from another thread. That is the point: see above. The
        // pointer is valid and the length correct for as long as the
        // `SharedMemory` lives, and this borrow cannot outlive `self`.
        unsafe { std::slice::from_raw_parts_mut(cells.as_ptr() as *mut u8, cells.len()) }
    }

    pub fn shared(&self) -> &SharedMemory {
        &self.inner
    }
}

/// The host's page: what [`GuestMemory::share_pages`] shares by.
pub fn host_page() -> u64 {
    // SAFETY: sysconf has no preconditions.
    let p = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if p > 0 { p as u64 } else { 4096 }
}

/// `[start, end)` of the memory at host address `base` (a live
/// [`GuestMemory`]'s [`GuestMemory::base`]) is its own again: whole host
/// pages get private zero pages; a host page only partly in the range keeps
/// what the rest of it holds, as this memory's own. Nothing for an address
/// that is no live memory's, or a range past its end.
pub fn unshare_pages(base: u64, start: u64, end: u64) {
    let live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    let page = host_page();
    let (lo, hi) = (start & !(page - 1), end.saturating_add(page - 1) & !(page - 1));
    if !live.iter().any(|&(b, n)| b == base && hi <= n) || lo >= hi {
        return;
    }
    let mut at = lo;
    while at < hi {
        let whole = at >= start && at + page <= end;
        // SAFETY: the page is inside the live memory (checked above, held
        // so by `LIVE`'s lock, which a memory's drop takes).
        let keep = (!whole).then(|| unsafe { std::slice::from_raw_parts((base + at) as *const u8, page as usize) }.to_vec());
        // SAFETY: a private mapping over that page, readable and writable
        // as the memory is, in place of the shared one.
        let r = unsafe {
            libc::mmap(
                (base + at) as *mut libc::c_void,
                page as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_FIXED | libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if r == libc::MAP_FAILED {
            eprintln!("gex: giving back a shared page at {at:#x} failed: {}", std::io::Error::last_os_error());
        } else if let Some(bytes) = keep {
            // SAFETY: as above, the page just mapped.
            let dst = unsafe { std::slice::from_raw_parts_mut((base + at) as *mut u8, page as usize) };
            let (s, e) = ((start.max(at) - at) as usize, (end.min(at + page) - at) as usize);
            dst[..s].copy_from_slice(&bytes[..s]);
            dst[e..].copy_from_slice(&bytes[e..]);
        }
        at += page;
    }
}

#[cfg(target_os = "macos")]
mod mach {
    unsafe extern "C" {
        pub static mach_task_self_: u32;
        pub fn mach_vm_remap(
            target_task: u32,
            target_address: *mut u64,
            size: u64,
            mask: u64,
            flags: i32,
            src_task: u32,
            src_address: u64,
            copy: i32,
            cur_protection: *mut i32,
            max_protection: *mut i32,
            inheritance: u32,
        ) -> i32;
    }
    pub const VM_FLAGS_FIXED: i32 = 0;
    pub const VM_FLAGS_OVERWRITE: i32 = 0x4000;
    pub const VM_INHERIT_SHARE: u32 = 0;
}
