//! `/dev/gex-services` on a page: the services the page's shell provides to
//! programs (notifications first), each open a connection to one of them.
//!
//! The same node as the native runner's (hosts/native `services.rs`, where
//! the frames and the negotiation are written down), for the same guests.
//! Here the other end is the page runtime's `Services` (page/runtime
//! `services.js`), on the page's thread, over a ring channel of its own of
//! the kind `/dev/gex-metal` uses (metal-bridge.mjs): the device is a
//! [`RingDevice`] over this channel's imports, and the kernel looks at none
//! of the bytes. `/dev/gex-services-bulk` is the same node over the
//! channel's pool of larger rings, for a service's heavy streams. The
//! runtime reads the frames, answers the first with the
//! service's version (0 for one the page does not provide) and hands the
//! rest to the page's provider.
//!
//! The node is there when the page made the channel (every system of the
//! multi-thread build does) and there is a global kernel to park a reader
//! in. The single-thread build has neither, so its programs find no node,
//! which they take as they take a service refused.

use crate::metal::{RingDevice, RingOps};

#[link(wasm_import_module = "gex_env")]
extern "C" {
    fn svc_open(pid: i32) -> i32;
    /// A connection of the channel's larger rings (/dev/gex-services-bulk).
    fn svc_open_bulk(pid: i32) -> i32;
    fn svc_read(slot: i32, ptr: f64, len: u32) -> i32;
    fn svc_write(slot: i32, ptr: f64, len: u32) -> i32;
    fn svc_ready(slot: i32) -> i32;
    fn svc_available(slot: i32) -> u32;
    fn svc_close(slot: i32);
}

/// The nodes' device numbers: misc devices (10) of the local-use range,
/// beside /dev/gex-host's 240, so that no terminal is ever named for them.
pub const RDEV: u64 = (10 << 8) | 241;
pub const RDEV_BULK: u64 = (10 << 8) | 242;

pub static SERVICES: RingOps = RingOps { open: svc_open, read: svc_read, write: svc_write, ready: svc_ready, available: svc_available, close: svc_close, rdev: RDEV };
pub static BULK: RingOps = RingOps { open: svc_open_bulk, read: svc_read, write: svc_write, ready: svc_ready, available: svc_available, close: svc_close, rdev: RDEV_BULK };

static mut HAVE: bool = false;

/// Say that the page serves the services channel.
#[no_mangle]
pub extern "C" fn gex_set_services(on: i32) {
    unsafe { HAVE = on != 0 }
}

pub fn have_services() -> bool {
    unsafe { HAVE }
}

pub fn device() -> RingDevice {
    RingDevice::over(&SERVICES)
}

/// An open of /dev/gex-services-bulk: the same, over the larger rings the
/// page reserved for heavy streams (none, and every open fails `ENXIO`,
/// unless it did).
pub fn bulk_device() -> RingDevice {
    RingDevice::over(&BULK)
}
