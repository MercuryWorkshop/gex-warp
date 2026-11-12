//! `/dev/gex-metal` on a page: a byte stream to the page's Metal replayer.
//!
//! The same node as the native runner's (hosts/native `metal.rs`), for the
//! same guest: a Mac program's Metal, written against the SDK's headers in a
//! repository of its own (mink), turns the program's calls into messages and
//! wants a replayer at the other end. Natively that is a program gexc
//! starts, over a pipe. Here it is whatever the page's embedder supplied
//! (`Metal` in the page runtime), over two rings of shared memory per open
//! (hosts/browser/js `metal-bridge.mjs`). Neither kernel looks at the bytes.
//!
//! - Each `open` is its own connection: the replayer has one set of objects
//!   for it. The connection is claimed at the first `read` or `write`, not
//!   at `open`: devfs asks the factory for a handle to answer a `stat` too.
//! - `write` copies the guest's bytes into the ring; the replayer takes them
//!   without waiting for anything of the guest's, so a full ring is a short
//!   wait inside the import and not a deadlock.
//! - `read` never waits here: an empty ring is `EAGAIN`, `poll_readable`
//!   says when there is something, and the kernel parks the thread on this
//!   process's wait queue. The replayer's side of the page walks that queue
//!   after it writes (`peer_wakes`: the wake comes from another realm, not
//!   from the page's turn of this Worker).
//! - The last close ends the connection; a reader sees the end of the stream
//!   once the replayer has gone and its output is drained.
//!
//! The node is there only when the page has a replayer and there is a global
//! kernel to park in: a page without either has a guest find no node, a
//! clean `ENOENT`, as natively without `--metal`.
//!
//! The device is a [`RingDevice`] over Metal's imports; `/dev/gex-services`
//! (services.rs) is the same device over its own channel's.

use gex_kernel::errno::{Errno, SysResult};
use gex_kernel::vfs::{FileHandle, FileType, Metadata};
use std::sync::atomic::{AtomicI32, Ordering};

#[link(wasm_import_module = "gex_env")]
extern "C" {
    /// A connection for this process: its number, or negative.
    fn metal_open(pid: i32) -> i32;
    /// Up to `len` bytes into this module's memory at `ptr` (an `f64`, as
    /// `gl_submit`'s address is: exact above 4 GiB): how many, 0 for none
    /// yet, negative at the end of the stream.
    fn metal_read(slot: i32, ptr: f64, len: u32) -> i32;
    /// `len` bytes from `ptr`: how many were taken, negative if the
    /// replayer has gone.
    fn metal_write(slot: i32, ptr: f64, len: u32) -> i32;
    /// Bit 0: a read would not wait; bit 1: the stream has ended.
    fn metal_ready(slot: i32) -> i32;
    fn metal_available(slot: i32) -> u32;
    fn metal_close(slot: i32);
}

/// Set when the page said it has a Metal replayer behind the imports above.
static mut HAVE_METAL: bool = false;

/// Say that the page has a Metal replayer.
///
/// # Safety
/// Called from JS before the guest runs, as `gex_set_gl` is.
#[no_mangle]
pub extern "C" fn gex_set_metal(on: i32) {
    unsafe { HAVE_METAL = on != 0 }
}

pub fn have_metal() -> bool {
    unsafe { HAVE_METAL }
}

/// The most one call moves: a ring is far smaller, and the import's length
/// is 32 bits.
const CHUNK: usize = 1 << 24;

/// A channel's imports, as metal-bridge.mjs `MetalProxy` answers them for
/// any channel made by `makeMetalChannel`.
#[derive(Debug)]
pub struct RingOps {
    pub open: unsafe extern "C" fn(pid: i32) -> i32,
    pub read: unsafe extern "C" fn(slot: i32, ptr: f64, len: u32) -> i32,
    pub write: unsafe extern "C" fn(slot: i32, ptr: f64, len: u32) -> i32,
    pub ready: unsafe extern "C" fn(slot: i32) -> i32,
    pub available: unsafe extern "C" fn(slot: i32) -> u32,
    pub close: unsafe extern "C" fn(slot: i32),
}

pub static METAL: RingOps = RingOps { open: metal_open, read: metal_read, write: metal_write, ready: metal_ready, available: metal_available, close: metal_close };

/// One open of a node whose other end is a connection of a ring channel.
#[derive(Debug)]
pub struct RingDevice {
    ops: &'static RingOps,
    /// The connection's number once claimed; -1 before, -2 if none could be.
    slot: AtomicI32,
}

/// One open of /dev/gex-metal.
pub type MetalDevice = RingDevice;

impl RingDevice {
    pub fn new() -> RingDevice {
        RingDevice::over(&METAL)
    }

    pub fn over(ops: &'static RingOps) -> RingDevice {
        RingDevice { ops, slot: AtomicI32::new(-1) }
    }

    fn slot(&self) -> Result<i32, Errno> {
        match self.slot.load(Ordering::Acquire) {
            -1 => {
                let got = unsafe { (self.ops.open)(crate::my_pid()) };
                let slot = if got >= 0 { got } else { -2 };
                // Two threads' first calls at once: one connection, the other given back.
                match self.slot.compare_exchange(-1, slot, Ordering::AcqRel, Ordering::Acquire) {
                    Ok(_) => {}
                    Err(theirs) => {
                        if slot >= 0 {
                            unsafe { (self.ops.close)(slot) };
                        }
                        return if theirs >= 0 { Ok(theirs) } else { Err(Errno::ENXIO) };
                    }
                }
                if slot >= 0 { Ok(slot) } else { Err(Errno::ENXIO) }
            }
            s if s >= 0 => Ok(s),
            _ => Err(Errno::ENXIO),
        }
    }
}

impl FileHandle for RingDevice {
    fn read_at(&self, _offset: u64, buf: &mut [u8]) -> SysResult {
        let slot = self.slot()?;
        if buf.is_empty() {
            return Ok(0);
        }
        let n = unsafe { (self.ops.read)(slot, buf.as_mut_ptr() as usize as f64, buf.len().min(CHUNK) as u32) };
        match n {
            n if n > 0 => Ok(n as u64),
            0 => Err(Errno::EAGAIN),
            _ => Ok(0),
        }
    }

    fn write_at(&self, _offset: u64, buf: &[u8]) -> SysResult {
        let slot = self.slot()?;
        if buf.is_empty() {
            return Ok(0);
        }
        let n = unsafe { (self.ops.write)(slot, buf.as_ptr() as usize as f64, buf.len().min(CHUNK) as u32) };
        match n {
            n if n > 0 => Ok(n as u64),
            0 => Err(Errno::EAGAIN),
            _ => Err(Errno::EPIPE),
        }
    }

    fn metadata(&self) -> Result<Metadata, Errno> {
        Ok(Metadata { file_type: FileType::CharDevice, size: 0, mode: 0o666, ino: 0, mtime: 0, nlink: 1, rdev: 0, times: Default::default() })
    }

    fn is_stream(&self) -> bool {
        true
    }

    fn poll_readable(&self) -> bool {
        match self.slot.load(Ordering::Acquire) {
            // Not claimed yet: nothing was asked, so nothing is coming. (A
            // reader that starts before the first write parks; the write
            // claims the connection and the answer wakes it.)
            -1 => false,
            s if s >= 0 => unsafe { (self.ops.ready)(s) & 1 != 0 },
            _ => true,
        }
    }

    fn available(&self) -> Option<u64> {
        match self.slot.load(Ordering::Acquire) {
            s if s >= 0 => Some(unsafe { (self.ops.available)(s) } as u64),
            _ => Some(0),
        }
    }

    fn poll_hup(&self) -> bool {
        match self.slot.load(Ordering::Acquire) {
            s if s >= 0 => unsafe { (self.ops.ready)(s) & 2 != 0 },
            _ => false,
        }
    }

    /// The replayer's side of the page wakes a parked reader by walking this
    /// process's wait queue.
    fn wait_queue(&self) -> u32 {
        crate::waitq::proc_queue()
    }

    fn peer_wakes(&self) -> bool {
        true
    }
}

impl Drop for RingDevice {
    fn drop(&mut self) {
        let slot = self.slot.load(Ordering::Acquire);
        if slot >= 0 {
            unsafe { (self.ops.close)(slot) };
        }
    }
}
