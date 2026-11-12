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
//!
//! # Two views of one memory
//!
//! On macOS the host reads and writes the guest's memory through a second
//! mapping of the same pages ([`GuestMemory::bytes`]), made at the start and
//! kept readable and writable, while the wasm memory itself -- what
//! translated code addresses -- carries the guest's own protections
//! ([`GuestMemory::protect`]): a guest store into a read-only or
//! `PROT_NONE` page faults in translated code, at no cost to any store, and
//! the kernel's own copies (a `read` into a buffer, a signal frame, a fork's
//! copy, the loader) never fault the host. A range remapped onto other pages
//! (a share) is remapped in both views.

use std::sync::{Arc, Mutex};
use wasmtime::SharedMemory;

/// The guest's linear memory, viewable as bytes.
#[derive(Clone)]
pub struct GuestMemory {
    inner: SharedMemory,
    /// The host's view of the same pages (see the module comment), or the
    /// memory itself where there is none.
    view: u64,
    /// Its place in [`LIVE`] while any handle of it is.
    _live: Arc<Live>,
}

/// The memories alive, by base, with their lengths and host views: what
/// [`unshare_pages`] checks the memory it is named is one.
static LIVE: Mutex<Vec<(u64, u64, u64)>> = Mutex::new(Vec::new());

/// The same, for a signal handler, which may take no lock: up to
/// [`LIVE_SLOTS`] memories' base and length, a slot empty at base 0.
const LIVE_SLOTS: usize = 64;
static LIVE_RANGES: [(std::sync::atomic::AtomicU64, std::sync::atomic::AtomicU64); LIVE_SLOTS] =
    [const { (std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0)) }; LIVE_SLOTS];

struct Live(u64, u64, u64);

impl Drop for Live {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::*;
        LIVE.lock().unwrap_or_else(|e| e.into_inner()).retain(|&(b, _, _)| b != self.0);
        for (b, _) in &LIVE_RANGES {
            let _ = b.compare_exchange(self.0, 0, AcqRel, Relaxed);
        }
        #[cfg(target_os = "macos")]
        if self.2 != self.0 {
            // SAFETY: the view was mapped by `new` for this memory, and no
            // handle of the memory, so no borrow of its bytes, is left.
            unsafe { libc::munmap(self.2 as *mut libc::c_void, self.1 as usize) };
        }
    }
}

impl GuestMemory {
    pub fn new(inner: SharedMemory) -> GuestMemory {
        let base = inner.data().as_ptr() as u64;
        let len = inner.data_size() as u64;
        let view = make_view(base, len).unwrap_or(base);
        LIVE.lock().unwrap_or_else(|e| e.into_inner()).push((base, len, view));
        {
            use std::sync::atomic::Ordering::*;
            // (memories are made one at a time: a slot seen empty is this
            // one's, its length written before its base is)
            for (b, l) in &LIVE_RANGES {
                if b.load(Acquire) != 0 {
                    continue;
                }
                l.store(len, Relaxed);
                if b.compare_exchange(0, base, AcqRel, Relaxed).is_ok() {
                    break;
                }
            }
        }
        GuestMemory { inner, view, _live: Arc::new(Live(base, len, view)) }
    }

    /// Whether the host has a view of its own ([`Self::bytes`]): then
    /// [`Self::protect`] may protect the wasm memory's pages.
    pub fn has_view(&self) -> bool {
        self.view != self.base()
    }

    /// Give `[start, end)` of the wasm memory -- what translated code
    /// addresses -- the protection `prot` (`PROT_READ`, `PROT_WRITE`, as
    /// `libc`'s), on whole host pages; the host's view keeps its own. Nothing
    /// without a view of the host's: the host would fault on its own
    /// accesses.
    pub fn protect(&self, start: u64, end: u64, prot: i32) {
        protect_pages(self.base(), self.len() as u64, self.view != self.base(), start, end, prot);
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
        self.share_pages_from(from, start, start, end.wrapping_sub(start))
    }

    /// [`Self::share_pages`] of `len` bytes at `start` here onto those at
    /// `from_start` of `from`'s: a mapping a process made of a file another
    /// process has mapped shared already, at another address.
    pub fn share_pages_from(&self, from: &GuestMemory, from_start: u64, start: u64, len: u64) -> Result<(), i32> {
        let page = host_page();
        let end = start.wrapping_add(len);
        if start % page != 0 || from_start % page != 0 || len % page != 0 || len == 0 || end > self.len() as u64 || from_start.saturating_add(len) > from.len() as u64 {
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
                    from.base() + from_start,
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
            if self.view != self.base() {
                realias(self.base(), self.view, start, len);
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

    /// The whole address space as a mutable slice: the host's view of it,
    /// readable and writable whatever the guest's protections. See the
    /// module comment for why this is not `unsafe`. The lifetime is tied to
    /// the borrow of `self`, in practice one host call.
    #[allow(clippy::mut_from_ref)]
    pub fn bytes(&self) -> &mut [u8] {
        let cells = self.inner.data();
        // SAFETY: the view (or `cells` itself) is the guest's address space,
        // which the guest may be writing to from another thread. That is the
        // point: see above. The pointer is valid and the length correct for
        // as long as the `SharedMemory` lives (the view is unmapped only
        // when the last handle goes), and this borrow cannot outlive `self`.
        unsafe { std::slice::from_raw_parts_mut(self.view as *mut u8, cells.len()) }
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
    let Some(&(_, _, view)) = live.iter().find(|&&(b, n, _)| b == base && hi <= n) else {
        return;
    };
    if lo >= hi {
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
        if view != base {
            realias(base, view, at, page);
        }
        at += page;
    }
}

/// The host's view of the memory at `base`: the same pages mapped a second
/// time, shared (macOS: `mach_vm_remap` with no copy). `None` elsewhere, or
/// with `GEX_NO_PROTECT` set, where the host uses the memory itself and no
/// protection is mirrored.
fn make_view(base: u64, len: u64) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        if std::env::var_os("GEX_NO_PROTECT").is_some() || len == 0 {
            return None;
        }
        let mut at = 0u64;
        let (mut cur, mut max) = (0, 0);
        // SAFETY: `[base, base + len)` is the live wasm memory just made; the
        // new mapping is placed anywhere free and shares its pages.
        let r = unsafe {
            mach::mach_vm_remap(mach::mach_task_self_, &mut at, len, 0, mach::VM_FLAGS_ANYWHERE, mach::mach_task_self_, base, 0, &mut cur, &mut max, mach::VM_INHERIT_NONE)
        };
        if r != 0 || at == 0 {
            return None;
        }
        // SAFETY: the view just mapped, all of it ours.
        unsafe { libc::mprotect(at as *mut libc::c_void, len as usize, libc::PROT_READ | libc::PROT_WRITE) };
        Some(at)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (base, len);
        None
    }
}

/// `[start, start + len)` of the view at `view` made the pages the memory
/// at `base` has there now (they were remapped), readable and writable.
fn realias(base: u64, view: u64, start: u64, len: u64) {
    #[cfg(target_os = "macos")]
    {
        let mut at = view + start;
        let (mut cur, mut max) = (0, 0);
        // SAFETY: both ranges are inside one live memory and its view.
        let r = unsafe {
            mach::mach_vm_remap(
                mach::mach_task_self_,
                &mut at,
                len,
                0,
                mach::VM_FLAGS_FIXED | mach::VM_FLAGS_OVERWRITE,
                mach::mach_task_self_,
                base + start,
                0,
                &mut cur,
                &mut max,
                mach::VM_INHERIT_NONE,
            )
        };
        if r != 0 {
            eprintln!("gex: the host's view of {start:#x}+{len:#x} could not follow its pages: {r}");
        }
        // SAFETY: as above, the view's own range.
        unsafe { libc::mprotect((view + start) as *mut libc::c_void, len as usize, libc::PROT_READ | libc::PROT_WRITE) };
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (base, view, start, len);
    }
}

/// [`GuestMemory::protect`] for the memory at host address `base` (a live
/// memory's [`GuestMemory::base`]): the kernel's line, which names a memory
/// by its base. Nothing for one that is no live memory's.
pub fn protect_live(base: u64, start: u64, end: u64, read: bool, write: bool) {
    let prot = if write { libc::PROT_READ | libc::PROT_WRITE } else if read { libc::PROT_READ } else { libc::PROT_NONE };
    let live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&(b, len, view)) = live.iter().find(|&&(b, _, _)| b == base) {
        protect_pages(b, len, view != b, start, end, prot);
    }
}

fn protect_pages(base: u64, len: u64, viewed: bool, start: u64, end: u64, prot: i32) {
    let page = host_page();
    let (lo, hi) = (start & !(page - 1), end.saturating_add(page - 1) & !(page - 1));
    if !viewed || lo >= hi || hi > len {
        return;
    }
    // SAFETY: whole host pages of the live wasm memory; the host reaches
    // them through its view, so only translated code sees the change.
    let r = unsafe { libc::mprotect((base + lo) as *mut libc::c_void, (hi - lo) as usize, prot) };
    if r != 0 {
        eprintln!("gex: protecting {lo:#x}..{hi:#x}: {}", std::io::Error::last_os_error());
    }
}

/// Faults the host itself took in a protected page of a guest's memory (a
/// copy of wasm's own, `memory.copy`, outside translated code): each page
/// was made readable and writable again so the copy could go on, and the
/// kernel puts its protection back ([`lifted`]).
static LIFTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// For a signal handler: whether `addr` is in a live memory's wasm range,
/// and if so make its host page readable and writable and count it
/// ([`LIFTED`]). Lock-free and allocation-free.
pub fn lift_fault(addr: u64) -> bool {
    use std::sync::atomic::Ordering::*;
    for (b, l) in &LIVE_RANGES {
        let base = b.load(Acquire);
        if base != 0 && addr >= base && addr < base + l.load(Relaxed) {
            let page = host_page_cached();
            // SAFETY: a host page of a live wasm memory.
            unsafe { libc::mprotect((addr & !(page - 1)) as *mut libc::c_void, page as usize, libc::PROT_READ | libc::PROT_WRITE) };
            LIFTED.fetch_add(1, Relaxed);
            return true;
        }
    }
    false
}

/// The handlers of `SIGSEGV` and `SIGBUS` before [`install_fault_lifter`]'s.
static PREV: [std::sync::atomic::AtomicUsize; 2] = [const { std::sync::atomic::AtomicUsize::new(0) }; 2];
static PREV_FLAGS: [std::sync::atomic::AtomicI32; 2] = [const { std::sync::atomic::AtomicI32::new(0) }; 2];

/// Catch the host's own faults in a guest's protected pages: a copy the
/// engine makes outside translated code (`memory.copy`, `memory.fill`)
/// into a page the guest made read-only would otherwise end the whole
/// host. The page is made writable and the copy goes on ([`lift_fault`]);
/// any other fault goes to the handler there was. Installed before the
/// engine installs its own, which hands on the faults that are not in
/// translated code. Once; nothing where there is no view to protect by.
pub fn install_fault_lifter() {
    #[cfg(target_os = "macos")]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        if std::env::var_os("GEX_NO_PROTECT").is_some() {
            return;
        }
        ONCE.call_once(|| {
            for (i, sig) in [libc::SIGSEGV, libc::SIGBUS].into_iter().enumerate() {
                // SAFETY: a handler of our own for the two fault signals,
                // the previous one kept to hand on to.
                unsafe {
                    let mut sa: libc::sigaction = std::mem::zeroed();
                    sa.sa_sigaction = lifter as usize;
                    sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK | libc::SA_NODEFER;
                    libc::sigemptyset(&mut sa.sa_mask);
                    let mut old: libc::sigaction = std::mem::zeroed();
                    if libc::sigaction(sig, &sa, &mut old) == 0 {
                        PREV[i].store(old.sa_sigaction, std::sync::atomic::Ordering::Relaxed);
                        PREV_FLAGS[i].store(old.sa_flags, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
        });
    }
}

#[cfg(target_os = "macos")]
extern "C" fn lifter(sig: libc::c_int, info: *mut libc::siginfo_t, ctx: *mut libc::c_void) {
    use std::sync::atomic::Ordering::Relaxed;
    // SAFETY: the kernel's siginfo for this fault.
    let addr = unsafe { (*info).si_addr } as u64;
    if lift_fault(addr) {
        return;
    }
    let i = if sig == libc::SIGSEGV { 0 } else { 1 };
    let prev = PREV[i].load(Relaxed);
    if prev == libc::SIG_DFL || prev == libc::SIG_IGN {
        // (the default: put it back, and the fault, taken again on return,
        // ends the process as it would have)
        // SAFETY: restoring the default disposition of this signal.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = libc::SIG_DFL;
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
        return;
    }
    // SAFETY: the handler that was installed before ours, called as it
    // asked to be.
    unsafe {
        if PREV_FLAGS[i].load(Relaxed) & libc::SA_SIGINFO != 0 {
            let f: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) = std::mem::transmute(prev);
            f(sig, info, ctx);
        } else {
            let f: extern "C" fn(libc::c_int) = std::mem::transmute(prev);
            f(sig);
        }
    }
}

/// How many pages [`lift_fault`] has made writable so far: a kernel that
/// sees the count move puts its protections back.
pub fn lifted() -> u64 {
    LIFTED.load(std::sync::atomic::Ordering::Relaxed)
}

/// [`host_page`] without a system call, for a signal handler.
fn host_page_cached() -> u64 {
    static PAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    match PAGE.load(std::sync::atomic::Ordering::Relaxed) {
        0 => {
            let p = host_page();
            PAGE.store(p, std::sync::atomic::Ordering::Relaxed);
            p
        }
        p => p,
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
    pub const VM_FLAGS_ANYWHERE: i32 = 1;
    pub const VM_FLAGS_OVERWRITE: i32 = 0x4000;
    pub const VM_INHERIT_SHARE: u32 = 0;
    pub const VM_INHERIT_NONE: u32 = 2;
}
