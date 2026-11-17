//! IOKit: the registry of devices and the drivers' user clients, as a task
//! reaches them from IOKit.framework -- through the master port
//! (`host_get_io_master`) and a port for each object it is given, by the
//! `iokit` MIG subsystem (`osfmk/device/device.defs`, routines 2800 on).
//!
//! An object is a registry entry ([`registry`]), an iterator over some, a
//! connection to a user client (`IOServiceOpen`), or a notification's
//! handle; each has one port of the kernel's, and a task given it again
//! has the same name for it. The registry is the system's: natively every
//! process sees the one table, and on the page each process has its own.
//!
//! What a user client does is its driver's: the HID system's ([`hid`]),
//! the framebuffer's. Memory a client maps into its task
//! (`IOConnectMapMemory`) is the task's own, at an address the kernel
//! keeps with the connection ([`Connect::maps`]), which is how the kernel
//! (and a host, through it) finds a framebuffer's pixels or the HID
//! system's queue again.
//!
//! With `GEX_IOKIT_TRACE`, each request is said: the routine, the object,
//! what was asked and the answer.

pub mod display;
pub mod gpuw;
pub mod hid;
pub mod plist;
pub mod png;
pub mod registry;
pub mod surface;

use super::port::{get32, post, put32, Kind, Msg, Port, Space, MIG_BAD_ID, NDR};
use super::*;
use plist::Value;
use registry::{Opens, Registry};
use std::collections::HashMap;
use std::sync::Mutex;

/// `IOReturn`s.
pub const SUCCESS: i32 = 0;
pub const ERROR: i32 = 0xe000_02bc_u32 as i32;
pub const BAD_ARGUMENT: i32 = 0xe000_02c2_u32 as i32;
pub const UNSUPPORTED: i32 = 0xe000_02c7_u32 as i32;
pub const NO_DEVICE: i32 = 0xe000_02c0_u32 as i32;
pub const NOT_FOUND: i32 = 0xe000_02f0_u32 as i32;
pub const NOT_PERMITTED: i32 = 0xe000_02e2_u32 as i32;

/// `io_server_version`'s answer: XNU 4903's `IOKIT_SERVER_VERSION`, which
/// has IOKit's user side serialize to the binary form.
const SERVER_VERSION: u64 = 20150715;

/// A connection to a user client.
pub struct Connect {
    pub entry: usize,
    /// The task that opened it.
    pub pid: i32,
    pub client: Client,
    /// The port given by `IOConnectSetNotificationPort`, and its type.
    pub notify: Option<(u32, Arc<Port>)>,
    /// The memory mapped into the task: its type, address and size.
    pub maps: Vec<(u32, u64, u64)>,
}

/// What a user client is.
pub enum Client {
    Hid(hid::Hid),
    Fb(display::Fb),
    /// The GPU wrangler's: the GPUs, and the framebuffer of each.
    Gpu(gpuw::Gpu),
    /// `IOSurfaceRoot`'s: the surfaces the task has.
    Surface(surface::Client),
    /// A client with no methods of its own (a family's that nothing here
    /// drives yet): every method is unsupported.
    Plain,
    /// AppleMobileFileIntegrity's: told a code directory's hash to trust
    /// (method 3, CVMServer's, for the code an OpenCL build makes), which
    /// it takes. Nothing here checks a signature, so there is nothing to
    /// trust it for.
    Amfi,
}

enum Obj {
    Entry(usize),
    /// What an iterator gives, and how far it has.
    Iter(Vec<usize>, usize),
    Connect(Connect),
    /// A notification's handle (`IOServiceAddMatchingNotification`,
    /// `IOServiceAddInterestNotification`): nothing is ever told by it, as
    /// no service comes or goes here.
    Note,
    /// A send right to a surface (`IOSurfaceSendRight`), by its id.
    SurfaceRight(u32),
}

struct Object {
    obj: Obj,
    port: Arc<Port>,
}

pub struct IoKit {
    reg: Registry,
    objects: HashMap<u32, Object>,
    next: u32,
    /// Each entry's object, once it has one.
    entries: HashMap<usize, u32>,
}

static IOKIT: Mutex<Option<IoKit>> = Mutex::new(None);

impl IoKit {
    fn new() -> IoKit {
        IoKit { reg: Registry::new(), objects: HashMap::new(), next: 1, entries: HashMap::new() }
    }

    fn add(&mut self, obj: Obj) -> u32 {
        let h = self.next;
        self.next += 1;
        self.objects.insert(h, Object { obj, port: Port::new(Kind::IoObject(h)) });
        h
    }

    /// The port of entry `i`'s object.
    fn entry_port(&mut self, i: usize) -> Arc<Port> {
        let h = match self.entries.get(&i) {
            Some(&h) => h,
            None => {
                let h = self.add(Obj::Entry(i));
                self.entries.insert(i, h);
                h
            }
        };
        Arc::clone(&self.objects[&h].port)
    }

    fn iterator(&mut self, items: Vec<usize>) -> Arc<Port> {
        let h = self.add(Obj::Iter(items, 0));
        Arc::clone(&self.objects[&h].port)
    }

    /// The registry entry an object is, or a connection's service.
    fn entry_of(&self, h: Option<u32>) -> Option<usize> {
        match &self.objects.get(&h?)?.obj {
            Obj::Entry(i) => Some(*i),
            Obj::Connect(c) => Some(c.entry),
            _ => None,
        }
    }

    fn describe(&self, h: Option<u32>) -> String {
        match h.and_then(|h| self.objects.get(&h)) {
            None => "master".to_string(),
            Some(o) => match &o.obj {
                Obj::Entry(i) => format!("{} {}", self.reg.entries[*i].class, self.reg.entries[*i].name),
                Obj::Iter(items, at) => format!("iterator {at}/{}", items.len()),
                Obj::Connect(c) => format!("connection to {}", self.reg.entries[c.entry].class),
                Obj::Note => "notification".to_string(),
                Obj::SurfaceRight(id) => format!("surface {id}'s send right"),
            },
        }
    }
}

/// Kick the HID system's consumer: the notification port of each server
/// connection to it, told there is input (a message of no body, as
/// `IOHIDSystem::kickEventConsumer` sends). The task takes the events when
/// it next calls IOKit or receives ([`Kernel::darwin_hid_drain`]).
pub fn kick_hid() {
    let io = IOKIT.lock().unwrap_or_else(|e| e.into_inner());
    let Some(kit) = io.as_ref() else { return };
    let kind = kit.hid_consumer();
    for o in kit.objects.values() {
        if let Obj::Connect(Connect { client: Client::Hid(h), notify: Some((_, port)), pid, .. }) = &o.obj {
            if h.kind == kind {
                let told = post(port, Msg::kernel(0, Vec::new(), Vec::new(), Vec::new()));
                if std::env::var_os("GEX_IOKIT_TRACE").is_some() {
                    eprintln!("[gex:{pid}] IOKit HID: input kicked at the type {kind} connection's port{}", if told { "" } else { ", which is dead" });
                }
            }
        }
    }
}

impl IoKit {
    /// Which of the HID system's connections takes what is posted: the
    /// event system's (3), once it has a queue mapped and a port to be
    /// told at, or else the server's (0).
    fn hid_consumer(&self) -> u32 {
        let ready = self.objects.values().any(|o| {
            matches!(&o.obj, Obj::Connect(Connect { client: Client::Hid(h), notify: Some(_), maps, .. })
                if h.kind == 3 && h.queue.is_some_and(|(t, _)| maps.iter().any(|m| m.0 == t)))
        });
        if ready {
            3
        } else {
            0
        }
    }
}

/// Where a framebuffer's pixels are, as the kernel knows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pixels {
    /// The task that mapped them, and the address there.
    pub pid: i32,
    pub address: u64,
    pub width: u32,
    pub height: u32,
    pub row_bytes: u64,
}

/// The pixels of the framebuffer its server (WindowServer) has mapped, if
/// it has: what a host reads to show the display.
pub fn framebuffer() -> Option<Pixels> {
    let io = IOKIT.lock().unwrap_or_else(|e| e.into_inner());
    let kit = io.as_ref()?;
    kit.objects.values().find_map(|o| match &o.obj {
        Obj::Connect(Connect { client: Client::Fb(fb), maps, pid, .. }) if fb.kind == 0 => maps
            .iter()
            .find(|(t, _, _)| *t != display::CURSOR_MEMORY)
            .map(|&(_, address, _)| Pixels { pid: *pid, address, width: fb.mode.width, height: fb.mode.height, row_bytes: fb.row_bytes() }),
        _ => None,
    })
}

/// The framebuffer's pixels: the object WindowServer maps, its size, the
/// mode's width, height and bytes a row, and where a task has it mapped
/// (the server's mapping when there is one): its memory, and the offset
/// there.
#[allow(clippy::type_complexity)]
static VRAM: Mutex<Option<(Arc<crate::file::OpenFile>, u64, (u32, u32, u64), Option<(Arc<crate::proc::PeerVm>, u64)>)>> = Mutex::new(None);

/// What the framebuffer shows now, as a PNG, once its server has mapped
/// it, by any thread. The pixels are read where they are, in the memory
/// of the task that mapped them: what a task stores in a shared mapping
/// is in its pages (and those of the tasks that share them), and in the
/// object behind it only once it is written back, which nothing does for
/// a framebuffer. Without a task's memory to read (a host that keeps none),
/// the object's own bytes.
pub fn framebuffer_png() -> Option<Vec<u8>> {
    let (w, h, row, buf) = framebuffer_pixels()?;
    Some(png::encode(w, h, row as usize, &buf))
}

/// What the framebuffer shows now, as it holds it: its width, height and
/// bytes a row, and the rows (each pixel a little-endian word `xxRRGGBB`),
/// read as [`framebuffer_png`] reads them. What a host shows live.
pub fn framebuffer_pixels() -> Option<(u32, u32, u64, Vec<u8>)> {
    let (file, (w, h, row), live) = {
        let v = VRAM.lock().unwrap_or_else(|e| e.into_inner());
        let (f, _, shape, live) = v.as_ref()?;
        (Arc::clone(f), *shape, live.clone())
    };
    let mut buf = vec![0u8; (row * h as u64) as usize];
    if !live.is_some_and(|(vm, at)| vm.mem.read(at, &mut buf)) {
        file.handle.read_at(0, &mut buf).ok()?;
    }
    draw_cursor(w, h, row, &mut buf);
    Some((w, h, row, buf))
}

/// Where the framebuffer's server has its shared cursor (`StdFBShmem_t`
/// and the frames' images): its memory, the address, the size, and the
/// widest frame asked room for, and the server's pid.
#[allow(clippy::type_complexity)]
static CURSOR: Mutex<Option<(Arc<crate::proc::PeerVm>, u64, u64, u32, i32)>> = Mutex::new(None);

/// What gex last said of the shared cursor (GEX_IOKIT_TRACE), so as to
/// say it again only when it changes.
static CURSOR_SAID: Mutex<Option<(i32, u8, u8, (u16, u16), (i32, i32))>> = Mutex::new(None);

/// The shared cursor drawn over the pixels, as IOFramebuffer draws it on a
/// framebuffer with no cursor of its own (its software cursor): the frame
/// `StdFBShmem_t` names, of the size and hot spot CoreDisplay put in it,
/// its image at 236 + frame * (widest * widest * 4), 32-bit premultiplied
/// pixels a row its width; at the pointer, where the HID system has it;
/// while it is shown (`cursorShow` 0) and not obscured.
fn draw_cursor(w: u32, h: u32, row: u64, buf: &mut [u8]) {
    let Some((vm, at, len, widest, _)) = CURSOR.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
    let mut head = [0u8; 236];
    if len < 236 || !vm.mem.read(at, &mut head) {
        return;
    }
    let frame = i32::from_le_bytes(head[4..8].try_into().unwrap());
    let (show, obscured) = (head[8], head[9]);
    let slot = if frame != 0 { 1 } else { 0 };
    let size = (u16::from_le_bytes([head[204 + 4 * slot], head[205 + 4 * slot]]), u16::from_le_bytes([head[206 + 4 * slot], head[207 + 4 * slot]]));
    let hot = (i16::from_le_bytes([head[220 + 4 * slot], head[221 + 4 * slot]]) as i32, i16::from_le_bytes([head[222 + 4 * slot], head[223 + 4 * slot]]) as i32);
    let pointer = {
        let io = IOKIT.lock().unwrap_or_else(|e| e.into_inner());
        io.as_ref().and_then(|kit| {
            kit.objects.values().find_map(|o| match &o.obj {
                Obj::Connect(Connect { client: Client::Hid(hc), .. }) if hc.kind == 0 => Some(hc.at),
                _ => None,
            })
        })
    };
    let Some(pointer) = pointer else { return };
    if std::env::var_os("GEX_IOKIT_TRACE").is_some() {
        let now = (frame, show, obscured, size, pointer);
        let mut said = CURSOR_SAID.lock().unwrap_or_else(|e| e.into_inner());
        if *said != Some(now) {
            eprintln!("[gex] IOKit cursor: frame {frame}, show {show}, obscured {obscured}, {}x{} hot {},{}, at {},{}", size.0, size.1, hot.0, hot.1, pointer.0, pointer.1);
            *said = Some(now);
        }
    }
    let (cw, ch) = (size.0 as u64, size.1 as u64);
    if show != 0 || obscured != 0 || cw == 0 || ch == 0 || cw > widest as u64 || ch > widest as u64 || frame < 0 {
        return;
    }
    let frame_bytes = widest as u64 * widest as u64 * 4;
    let start = 236 + frame as u64 * frame_bytes;
    if start + cw * ch * 4 > len {
        return;
    }
    let mut img = vec![0u8; (cw * ch * 4) as usize];
    if !vm.mem.read(at + start, &mut img) {
        return;
    }
    let (x0, y0) = (pointer.0 - hot.0, pointer.1 - hot.1);
    for y in 0..ch as i32 {
        let sy = y0 + y;
        if sy < 0 || sy >= h as i32 {
            continue;
        }
        for x in 0..cw as i32 {
            let sx = x0 + x;
            if sx < 0 || sx >= w as i32 {
                continue;
            }
            let s = &img[((y as u64 * cw + x as u64) * 4) as usize..][..4];
            let a = s[3] as u32;
            if a == 0 {
                continue;
            }
            let d = &mut buf[(sy as u64 * row + sx as u64 * 4) as usize..][..4];
            for c in 0..3 {
                d[c] = (s[c] as u32 + d[c] as u32 * (255 - a) / 255).min(255) as u8;
            }
        }
    }
}

/// Surface `id`'s pixels as a PNG, when they are 32-bit.
pub fn surface_png(id: u32) -> Option<Vec<u8>> {
    let (shape, buf) = surface::pixels(id)?;
    (shape.bytes_per_element == 4 && shape.planes.is_empty()).then(|| png::encode(shape.width, shape.height, shape.bytes_per_row as usize, &buf[shape.offset as usize..]))
}

/// A request's arguments after its descriptors and NDR record, read in
/// order as MIG packs them: a variable array takes only the room its count
/// says, rounded to four bytes.
struct Rd<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Rd<'a> {
    fn of(req: &'a Msg) -> Rd<'a> {
        let b = &req.body;
        let mut at = 0;
        if req.complex {
            let n = get32(b, 0) as usize;
            at = 4;
            for _ in 0..n {
                at += if b.get(at + 11).copied().unwrap_or(0) == 0 { 12 } else { 16 };
            }
        }
        // (A request with no arguments has no NDR record either.)
        if b.len() > at {
            at += 8;
        }
        Rd { b, at }
    }

    fn u32(&mut self) -> u32 {
        let v = get32(self.b, self.at);
        self.at += 4;
        v
    }

    fn u64(&mut self) -> u64 {
        let v = self.b.get(self.at..self.at + 8).map_or(0, |s| u64::from_le_bytes(s.try_into().unwrap()));
        self.at += 8;
        v
    }

    /// An inline array of bytes: its count, then the bytes.
    fn bytes(&mut self) -> &'a [u8] {
        let n = self.u32() as usize;
        let s = self.b.get(self.at..(self.at + n).min(self.b.len())).unwrap_or(&[]);
        self.at += n.next_multiple_of(4);
        s
    }

    /// A C string (`io_name_t` and the like): an offset MIG does not use,
    /// the count with its terminator, the bytes.
    fn cstr(&mut self) -> String {
        self.at += 4;
        let s = self.bytes();
        let s = s.split(|&c| c == 0).next().unwrap_or(&[]);
        String::from_utf8_lossy(s).into_owned()
    }

    fn scalars(&mut self) -> Vec<u64> {
        let n = self.u32() as usize;
        (0..n.min(16)).map(|_| self.u64()).collect()
    }
}

/// `{ NDR, RetCode }` and what follows it.
fn simple(id: u32, code: i32, rest: &[u8]) -> Msg {
    let mut b = NDR.to_vec();
    b.extend_from_slice(&code.to_le_bytes());
    b.extend_from_slice(rest);
    Msg::kernel(id + 100, b, Vec::new(), Vec::new())
}

/// A C string as a reply carries it: offset, count, bytes, padding.
fn cstr(s: &str) -> Vec<u8> {
    let mut b = 0u32.to_le_bytes().to_vec();
    b.extend_from_slice(&(s.len() as u32 + 1).to_le_bytes());
    b.extend_from_slice(s.as_bytes());
    b.push(0);
    while b.len() % 4 != 0 {
        b.push(0);
    }
    b
}

/// An inline array of bytes as a reply carries it.
fn inline(s: &[u8]) -> Vec<u8> {
    let mut b = (s.len() as u32).to_le_bytes().to_vec();
    b.extend_from_slice(s);
    while b.len() % 4 != 0 {
        b.push(0);
    }
    b
}

/// A reply with one right sent (a port descriptor), and `{ NDR, rest }`
/// after it if the routine has more to say.
fn with_port(id: u32, port: Arc<Port>, rest: Option<&[u8]>) -> Msg {
    let mut b = vec![0u8; 16];
    put32(&mut b, 0, 1);
    b[4 + 10] = super::port::disp::MOVE_SEND as u8;
    if let Some(rest) = rest {
        b.extend_from_slice(&NDR);
        b.extend_from_slice(rest);
    }
    Msg::kernel(id + 100, b, vec![(4, port, super::port::disp::MOVE_SEND)], Vec::new())
}

/// A reply with a null port where a right would be, and `{ NDR, rest }`.
fn no_port(id: u32, rest: Option<&[u8]>) -> Msg {
    let mut b = vec![0u8; 16];
    put32(&mut b, 0, 1);
    b[4 + 10] = super::port::disp::MOVE_SEND as u8;
    if let Some(rest) = rest {
        b.extend_from_slice(&NDR);
        b.extend_from_slice(rest);
    }
    Msg::kernel(id + 100, b, Vec::new(), Vec::new()).complex_anyway()
}

/// A reply with memory out of line (the bytes, which arrive in memory new
/// to the task), then `{ NDR, count }`.
fn with_ool(id: u32, bytes: Vec<u8>) -> Msg {
    let mut b = vec![0u8; 20];
    put32(&mut b, 0, 1);
    b[4 + 11] = 1; // MACH_MSG_OOL_DESCRIPTOR
    put32(&mut b, 4 + 12, bytes.len() as u32);
    b.extend_from_slice(&NDR);
    b.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    Msg::kernel(id + 100, b, Vec::new(), vec![(4, bytes)])
}

/// A matching dictionary or properties as a request sends them: the binary
/// form, or the XML of the older routines.
fn unserialize(b: &[u8]) -> Option<Value> {
    if b.starts_with(&plist::SIGNATURE) {
        plist::from_binary(b)
    } else {
        let s = String::from_utf8_lossy(b.split(|&c| c == 0).next().unwrap_or(&[])).into_owned();
        plist::from_xml(&s)
    }
}

/// The routines' names, for the trace.
fn routine(id: u32) -> &'static str {
    match id {
        2800 => "io_object_get_class",
        2801 => "io_object_conforms_to",
        2802 => "io_iterator_next",
        2803 => "io_iterator_reset",
        2804 => "io_service_get_matching_services",
        2805 => "io_registry_entry_get_property",
        2806 => "io_registry_create_iterator",
        2809 => "io_registry_entry_from_path",
        2810 => "io_registry_entry_get_name",
        2811 => "io_registry_entry_get_properties",
        2812 => "io_registry_entry_get_property_bytes",
        2813 => "io_registry_entry_get_child_iterator",
        2814 => "io_registry_entry_get_parent_iterator",
        2816 => "io_service_close",
        2817 => "io_connect_get_service",
        2820 => "io_connect_add_client",
        2821 => "io_connect_set_properties",
        2826 => "io_registry_entry_get_path",
        2827 => "io_registry_get_root_entry",
        2828 => "io_registry_entry_set_properties",
        2829 => "io_registry_entry_in_plane",
        2830 => "io_object_get_retain_count",
        2831 => "io_service_get_busy_state",
        2832 => "io_service_wait_quiet",
        2833 => "io_registry_entry_create_iterator",
        2834 => "io_iterator_is_valid",
        2842 => "io_service_request_probe",
        2843 => "io_registry_entry_get_name_in_plane",
        2844 => "io_service_match_property_table",
        2852 => "io_connect_get_notification_semaphore",
        2854 => "io_registry_entry_get_location_in_plane",
        2855 => "io_registry_entry_get_property_recursively",
        2856 => "io_service_get_state",
        2857 => "io_service_get_matching_services_ool",
        2858 => "io_service_match_property_table_ool",
        2860 => "io_object_get_superclass",
        2861 => "io_object_get_bundle_identifier",
        2862 => "io_service_open_extended",
        2863 => "io_connect_map_memory_into_task",
        2864 => "io_connect_unmap_memory_from_task",
        2865 => "io_connect_method",
        2866 => "io_connect_async_method",
        2867 => "io_connect_set_notification_port",
        2868 => "io_service_add_notification",
        2869 => "io_service_add_interest_notification",
        2870 => "io_service_add_notification_ool",
        2871 => "io_registry_entry_get_registry_entry_id",
        2872 => "io_connect_method_var_output",
        2873 => "io_service_get_matching_service",
        2874 => "io_service_get_matching_service_ool",
        2875 => "io_service_get_authorization_id",
        2876 => "io_service_set_authorization_id",
        2877 => "io_server_version",
        2878 => "io_registry_entry_get_properties_bin",
        2879 => "io_registry_entry_get_property_bin",
        2880 => "io_service_get_matching_service_bin",
        2881 => "io_service_get_matching_services_bin",
        2882 => "io_service_match_property_table_bin",
        2884 => "io_service_add_notification_bin",
        2885 => "io_registry_entry_get_path_ool",
        2886 => "io_registry_entry_from_path_ool",
        2887 => "io_device_tree_entry_exists_with_name",
        _ => "?",
    }
}

/// A value short enough for a line of the trace.
fn brief(v: &Value) -> String {
    let s = plist::to_xml(v);
    if s.len() > 200 {
        format!("{}...", &s[..200])
    } else {
        s
    }
}

impl Kernel {
    /// A request to IOKit's master port or to one of its objects, answered
    /// as XNU's `iokit` server answers.
    /// What has been posted to the HID system ([`hid::post`]) and not yet
    /// taken, into the event queue of this task's server connection to it
    /// if it has one (WindowServer's), as `IOHIDSystem` posts each event:
    /// done when the task next calls IOKit or receives a message (the kick
    /// [`kick_hid`] sent it among them), since the kernel writes a task's
    /// memory only while it runs for that task. Only the bytes the events
    /// change are written: the task moves the queue's head meanwhile.
    pub(crate) fn darwin_hid_drain(&mut self, m: &mut GuestMem<'_>) {
        if !hid::pending() {
            return;
        }
        let now = self.darwin_abstime();
        let mut io = IOKIT.lock().unwrap_or_else(|e| e.into_inner());
        let Some(kit) = io.as_mut() else { return };
        let consumer = kit.hid_consumer();
        let sender = kit.reg.entries.iter().find(|e| e.class == "IOHIDSystem").map_or(0, |e| e.id);
        for o in kit.objects.values_mut() {
            let Obj::Connect(c) = &mut o.obj else { continue };
            if c.pid != self.pid {
                continue;
            }
            let Client::Hid(h) = &mut c.client else { continue };
            if h.kind != consumer {
                continue;
            }
            // The event system's queue: each event an entry at its tail,
            // written before the tail is moved past it (its reader may be
            // taking from the head meanwhile).
            if h.kind == 3 {
                let Some((token, _)) = h.queue else { continue };
                let Some(&(_, at, _)) = c.maps.iter().find(|(t, _, _)| *t == token) else { continue };
                let Ok(head) = m.read(at, hid::QUEUE_HEADER).map(|b| b.to_vec()) else { continue };
                let word = |i: usize| u32::from_le_bytes(head[4 * i..4 * i + 4].try_into().unwrap());
                let (size, first, tail) = (word(0), word(1), word(2));
                let entries = h.queue_entries(&hid::take(), now, sender);
                let (places, tail) = hid::Hid::enqueue_at(size, first, tail, &entries);
                for (to, i) in places {
                    let _ = m.write(at + hid::QUEUE_HEADER as u64 + to as u64, &entries[i]);
                }
                let _ = m.write(at + 8, &tail.to_le_bytes());
                if self.trace || std::env::var_os("GEX_IOKIT_TRACE").is_some() {
                    eprintln!("[gex:{}] IOKit HID: {} events into the event system's queue (head {first}, tail {tail})", self.pid, entries.len());
                }
                return;
            }
            let Some(&(_, at, size)) = c.maps.iter().find(|(t, _, _)| *t == 0) else { continue };
            let Ok(before) = m.read(at, size as usize).map(|b| b.to_vec()) else { continue };
            let mut shm = before.clone();
            if !h.drain(&mut shm, &hid::take(), now) {
                return;
            }
            let mut i = 0;
            while i < shm.len() {
                if shm[i] == before[i] {
                    i += 1;
                    continue;
                }
                let j = (i..shm.len()).find(|&j| shm[j] == before[j]).unwrap_or(shm.len());
                let _ = m.write(at + i as u64, &shm[i..j]);
                i = j;
            }
            return;
        }
    }

    pub(super) fn darwin_iokit(&mut self, m: &mut GuestMem<'_>, sp: &mut Space, kind: Kind, req: &Msg) -> Msg {
        self.darwin_hid_drain(m);
        let h = match kind {
            Kind::IoObject(h) => Some(h),
            _ => None,
        };
        let mut io = IOKIT.lock().unwrap_or_else(|e| e.into_inner());
        let kit = io.get_or_insert_with(IoKit::new);
        let mut said = String::new();
        let reply = self.darwin_iokit_routine(m, sp, kit, h, req, &mut said);
        if self.trace || std::env::var_os("GEX_IOKIT_TRACE").is_some() {
            // (An error reply is `{ NDR, code }` and not complex.)
            let code = if reply.complex { 0 } else { get32(&reply.body, 8) as i32 };
            let answer = if code == 0 { String::new() } else { format!(" -> {:#x}", code as u32) };
            eprintln!("[gex:{}] IOKit {} {} on {}{}{}", self.pid, req.id, routine(req.id), kit.describe(h), if said.is_empty() { String::new() } else { format!(": {said}") }, answer);
        }
        reply
    }

    fn darwin_iokit_routine(&mut self, m: &mut GuestMem<'_>, sp: &mut Space, kit: &mut IoKit, h: Option<u32>, req: &Msg, said: &mut String) -> Msg {
        let id = req.id;
        let mut rd = Rd::of(req);
        let entry = kit.entry_of(h);
        let fail = |code: i32| simple(id, code, &[]);
        // An entry asked for of an object that is none.
        macro_rules! entry {
            () => {
                match entry {
                    Some(e) => e,
                    None => return fail(BAD_ARGUMENT),
                }
            };
        }
        // A matching dictionary: from the request's bytes.
        let matching = |bytes: &[u8], said: &mut String| -> Option<Value> {
            let v = unserialize(bytes);
            *said = v.as_ref().map_or_else(|| format!("unreadable matching ({} bytes)", bytes.len()), brief);
            v
        };
        match id {
            2877 => simple(id, SUCCESS, &SERVER_VERSION.to_le_bytes()),
            // io_object_get_class, io_object_conforms_to,
            // io_object_get_superclass, io_object_get_bundle_identifier
            2800 => match h.and_then(|h| kit.objects.get(&h)) {
                Some(o) => {
                    let class = match &o.obj {
                        Obj::Entry(i) => kit.reg.entries[*i].class,
                        Obj::Connect(c) => match c.client {
                            Client::Hid(ref hc) if hc.kind == 1 => "IOHIDParamUserClient",
                            Client::Hid(ref hc) if hc.kind == 3 => "IOHIDEventSystemUserClient",
                            Client::Hid(_) => "IOHIDUserClient",
                            Client::Fb(ref fb) if fb.kind == 1 => "IOFramebufferSharedUserClient",
                            Client::Fb(_) => "IOFramebufferUserClient",
                            Client::Gpu(_) => "AppleGPUWranglerUserClient",
                            Client::Surface(_) => "IOSurfaceRootUserClient",
                            Client::Plain => "IOUserClient",
                            Client::Amfi => "AppleMobileFileIntegrityUserClient",
                        },
                        Obj::Iter(..) => "IOUserIterator",
                        Obj::Note => "IOServiceUserNotification",
                        Obj::SurfaceRight(_) => "IOSurfaceSendRight",
                    };
                    simple(id, SUCCESS, &cstr(class))
                }
                None => fail(BAD_ARGUMENT),
            },
            2801 => {
                let want = rd.cstr();
                *said = want.clone();
                let class = entry.map(|e| kit.reg.entries[e].class).unwrap_or("OSObject");
                simple(id, SUCCESS, &(registry::conforms(class, &want) as u32).to_le_bytes())
            }
            2860 => {
                let name = rd.cstr();
                *said = name.clone();
                match registry::superclass(&name) {
                    Some(s) => simple(id, SUCCESS, &cstr(s)),
                    None => fail(BAD_ARGUMENT),
                }
            }
            2861 => simple(id, SUCCESS, &cstr("com.apple.kernel")),
            2830 => simple(id, SUCCESS, &1u32.to_le_bytes()),
            // Iterators: the next, from the start again, still good.
            2802 => {
                let next = match h.and_then(|h| kit.objects.get_mut(&h)) {
                    Some(Object { obj: Obj::Iter(items, at), .. }) => {
                        let n = items.get(*at).copied();
                        *at += 1;
                        n
                    }
                    _ => return fail(BAD_ARGUMENT),
                };
                match next {
                    Some(i) => {
                        *said = kit.reg.path(i);
                        with_port(id, kit.entry_port(i), None)
                    }
                    None => fail(NO_DEVICE),
                }
            }
            2803 => match h.and_then(|h| kit.objects.get_mut(&h)) {
                Some(Object { obj: Obj::Iter(_, at), .. }) => {
                    *at = 0;
                    fail(SUCCESS)
                }
                _ => fail(BAD_ARGUMENT),
            },
            2834 => simple(id, SUCCESS, &1u32.to_le_bytes()),
            2807 | 2808 => fail(SUCCESS),
            // Matching: every service, or the first, a dictionary matches;
            // whether one entry does.
            2804 | 2881 | 2857 | 2873 | 2880 | 2874 => {
                let bytes = match id {
                    2804 | 2873 => {
                        rd.at += 4;
                        rd.bytes()
                    }
                    2881 | 2880 => rd.bytes(),
                    _ => req.memory.first().map_or(&[][..], |(_, b)| &b[..]),
                };
                let Some(dict) = matching(bytes, said) else { return fail(BAD_ARGUMENT) };
                let found = kit.reg.matching(&dict);
                said.push_str(&format!(" -> {} found", found.len()));
                let ool = matches!(id, 2857 | 2874);
                let tail = 0i32.to_le_bytes();
                if matches!(id, 2873 | 2880 | 2874) {
                    match found.first() {
                        Some(&i) => with_port(id, kit.entry_port(i), ool.then_some(&tail[..])),
                        None => fail(NOT_FOUND),
                    }
                } else if found.is_empty() {
                    // (No iterator at all for nothing found, as
                    // `IOService::getMatchingServices` gives none.)
                    no_port(id, ool.then_some(&tail[..]))
                } else {
                    let it = kit.iterator(found);
                    with_port(id, it, ool.then_some(&tail[..]))
                }
            }
            2844 | 2882 | 2858 => {
                let e = entry!();
                let bytes = match id {
                    2844 => {
                        rd.at += 4;
                        rd.bytes()
                    }
                    2882 => rd.bytes(),
                    _ => req.memory.first().map_or(&[][..], |(_, b)| &b[..]),
                };
                let Some(dict) = matching(bytes, said) else { return fail(BAD_ARGUMENT) };
                let yes = kit.reg.matches(e, &dict) as u32;
                if id == 2858 {
                    let mut b = 0i32.to_le_bytes().to_vec();
                    b.extend_from_slice(&yes.to_le_bytes());
                    simple(id, SUCCESS, &b)
                } else {
                    simple(id, SUCCESS, &yes.to_le_bytes())
                }
            }
            // Notifications of services matched or published, and of a
            // service's interest: what matches now is in the iterator the
            // handle is (and is all there will ever be).
            2868 | 2870 | 2884 => {
                let kind = rd.cstr();
                let bytes = match id {
                    2868 => {
                        rd.at += 4;
                        rd.bytes()
                    }
                    2884 => rd.bytes(),
                    _ => req.memory.first().map_or(&[][..], |(_, b)| &b[..]),
                };
                let Some(dict) = matching(bytes, said) else { return fail(BAD_ARGUMENT) };
                *said = format!("{kind} {said}");
                let found = if kind.contains("Terminat") { Vec::new() } else { kit.reg.matching(&dict) };
                let it = kit.iterator(found);
                let tail = 0i32.to_le_bytes();
                with_port(id, it, (id == 2870).then_some(&tail[..]))
            }
            2869 => {
                *said = rd.cstr();
                let n = kit.add(Obj::Note);
                with_port(id, Arc::clone(&kit.objects[&n].port), None)
            }
            // The registry: its root, an entry by path, iterators over it.
            2827 => with_port(id, kit.entry_port(registry::ROOT), None),
            2809 | 2886 => {
                let path = if id == 2809 {
                    rd.cstr()
                } else {
                    let inline = rd.cstr();
                    if inline.is_empty() {
                        req.memory.first().map(|(_, b)| String::from_utf8_lossy(b.split(|&c| c == 0).next().unwrap_or(&[])).into_owned()).unwrap_or_default()
                    } else {
                        inline
                    }
                };
                *said = path.clone();
                let tail = 0i32.to_le_bytes();
                // (A path to nothing is no entry, and no error: a null
                // port, as XNU answers.)
                match kit.reg.lookup(&path) {
                    Some(i) => with_port(id, kit.entry_port(i), (id == 2886).then_some(&tail[..])),
                    None => no_port(id, (id == 2886).then_some(&tail[..])),
                }
            }
            2806 => {
                let plane = rd.cstr();
                let options = rd.u32();
                *said = format!("{plane} options {options:#x}");
                let all = kit.reg.below(registry::ROOT);
                let items = if options & 1 != 0 { [vec![registry::ROOT], all].concat() } else { kit.reg.entries[registry::ROOT].children.clone() };
                with_port(id, kit.iterator(items), None)
            }
            2833 => {
                let e = entry!();
                let plane = rd.cstr();
                let options = rd.u32();
                *said = format!("{plane} options {options:#x}");
                // kIORegistryIterateRecursively, kIORegistryIterateParents
                let items = match (options & 1 != 0, options & 2 != 0) {
                    (_, true) => {
                        let mut up = Vec::new();
                        let mut at = kit.reg.entries[e].parent;
                        while let Some(p) = at {
                            up.push(p);
                            at = kit.reg.entries[p].parent;
                        }
                        if options & 1 == 0 {
                            up.truncate(1);
                        }
                        up
                    }
                    (true, false) => kit.reg.below(e),
                    (false, false) => kit.reg.entries[e].children.clone(),
                };
                with_port(id, kit.iterator(items), None)
            }
            2813 | 2814 => {
                let e = entry!();
                *said = rd.cstr();
                let items = if id == 2813 { kit.reg.entries[e].children.clone() } else { kit.reg.entries[e].parent.into_iter().collect() };
                with_port(id, kit.iterator(items), None)
            }
            2810 | 2843 => {
                let e = entry!();
                simple(id, SUCCESS, &cstr(&kit.reg.entries[e].name))
            }
            2854 => {
                let e = entry!();
                let loc = kit.reg.entries[e].location.clone();
                if loc.is_empty() { fail(NOT_FOUND) } else { simple(id, SUCCESS, &cstr(&loc)) }
            }
            2826 | 2885 => {
                let e = entry!();
                let path = kit.reg.path(e);
                if id == 2826 {
                    simple(id, SUCCESS, &cstr(&path))
                } else {
                    // The path inline, and no memory out of line.
                    let mut b = vec![0u8; 20];
                    put32(&mut b, 0, 1);
                    b[4 + 11] = 1;
                    b.extend_from_slice(&NDR);
                    b.extend_from_slice(&cstr(&path));
                    b.extend_from_slice(&0u32.to_le_bytes());
                    Msg::kernel(id + 100, b, Vec::new(), vec![(4, Vec::new())])
                }
            }
            2829 => {
                let plane = rd.cstr();
                simple(id, SUCCESS, &((entry.is_some() && plane == "IOService") as u32).to_le_bytes())
            }
            2871 => {
                let e = entry!();
                simple(id, SUCCESS, &kit.reg.entries[e].id.to_le_bytes())
            }
            2887 => simple(id, SUCCESS, &0u32.to_le_bytes()),
            // Properties: all of them, one, one found up the tree.
            2878 | 2811 => {
                let e = entry!();
                let props = kit.reg.entries[e].properties();
                with_ool(id, if id == 2878 { plist::to_binary(&props) } else { plist::to_xml(&props).into_bytes() })
            }
            2879 | 2805 | 2855 => {
                let e = entry!();
                let (name, options) = match id {
                    2805 => (rd.cstr(), 0),
                    _ => {
                        let _plane = rd.cstr();
                        let name = rd.cstr();
                        (name, rd.u32())
                    }
                };
                *said = name.clone();
                // kIORegistryIterateRecursively, up (kIORegistryIterateParents)
                // or down.
                let mut found = kit.reg.entries[e].prop(&name).cloned();
                if found.is_none() && options & 1 != 0 {
                    let order: Vec<usize> = if options & 2 != 0 {
                        std::iter::successors(kit.reg.entries[e].parent, |&p| kit.reg.entries[p].parent).collect()
                    } else {
                        kit.reg.below(e)
                    };
                    found = order.iter().find_map(|&i| kit.reg.entries[i].prop(&name).cloned());
                }
                match found {
                    Some(v) => with_ool(id, if id == 2879 { plist::to_binary(&v) } else { plist::to_xml(&v).into_bytes() }),
                    None => fail(NOT_FOUND),
                }
            }
            2812 => {
                let e = entry!();
                let name = rd.cstr();
                let room = rd.u32() as usize;
                *said = name.clone();
                match kit.reg.entries[e].prop(&name) {
                    Some(Value::Data(d)) if d.len() <= room => simple(id, SUCCESS, &inline(d)),
                    Some(Value::Str(s)) if s.len() < room => simple(id, SUCCESS, &inline(&[s.as_bytes(), &[0]].concat())),
                    Some(Value::Num(n, bits)) => simple(id, SUCCESS, &inline(&n.to_le_bytes()[..(*bits as usize / 8).min(room)])),
                    Some(Value::Bool(b)) => simple(id, SUCCESS, &inline(&[*b as u8])),
                    Some(_) => fail(BAD_ARGUMENT),
                    None => fail(NOT_FOUND),
                }
            }
            // io_registry_entry_set_properties, io_connect_set_properties:
            // what is set is kept with the entry.
            2828 | 2821 => {
                let e = entry!();
                let props = req.memory.first().and_then(|(_, b)| unserialize(b));
                *said = props.as_ref().map_or_else(String::new, brief);
                if let Some(Value::Dict(d)) = props {
                    for (k, v) in d {
                        kit.reg.entries[e].set_prop(&k, v);
                    }
                }
                simple(id, SUCCESS, &SUCCESS.to_le_bytes())
            }
            // A service's state: registered and matched, never busy;
            // waiting for quiet is done at once.
            2831 => simple(id, SUCCESS, &0u32.to_le_bytes()),
            2832 | 2842 => fail(SUCCESS),
            2856 => {
                let mut b = 0x0000_0006u64.to_le_bytes().to_vec(); // kIOServiceRegisteredState | kIOServiceMatchedState
                b.extend_from_slice(&0u32.to_le_bytes());
                b.extend_from_slice(&0u64.to_le_bytes());
                simple(id, SUCCESS, &b)
            }
            2875 => simple(id, SUCCESS, &0u64.to_le_bytes()),
            2876 => fail(SUCCESS),
            // Opening a service: a connection to the client its driver
            // makes for a task, of the type asked.
            2862 => {
                let e = entry!();
                let kind = rd.u32();
                let class = kit.reg.entries[e].class;
                *said = format!("type {kind}");
                let client = match kit.reg.entries[e].opens {
                    Opens::HidSystem if matches!(kind, 0 | 1 | 3) => Client::Hid(hid::Hid::new(kind)),
                    Opens::Framebuffer if matches!(kind, 0 | 1) => Client::Fb(display::Fb::new(kind)),
                    Opens::GpuWrangler => {
                        let id = |class: &str| kit.reg.entries.iter().find(|x| x.class == class).map_or(0, |x| x.id);
                        Client::Gpu(gpuw::Gpu { device: id("IOPCIDevice"), framebuffer: id("IONDRVFramebuffer") })
                    }
                    Opens::SurfaceRoot if kind == 0 => Client::Surface(surface::Client::default()),
                    Opens::Plain => Client::Plain,
                    Opens::Amfi => Client::Amfi,
                    Opens::HidSystem | Opens::Framebuffer | Opens::SurfaceRoot | Opens::Nothing => {
                        *said = format!("type {kind}: {class} has no such user client");
                        return no_port(id, Some(&UNSUPPORTED.to_le_bytes()));
                    }
                };
                let c = kit.add(Obj::Connect(Connect { entry: e, pid: self.pid, client, notify: None, maps: Vec::new() }));
                let port = Arc::clone(&kit.objects[&c].port);
                with_port(id, port, Some(&SUCCESS.to_le_bytes()))
            }
            2816 => {
                if let Some(Object { obj: Obj::Connect(Connect { client: Client::Surface(sc), .. }), .. }) = h.and_then(|h| kit.objects.get_mut(&h)) {
                    let held: Vec<(u32, surface::Held)> = sc.held.drain().collect();
                    self.darwin_surfaces_let_go(m, held);
                }
                fail(SUCCESS)
            }
            2817 => {
                let e = entry!();
                with_port(id, kit.entry_port(e), None)
            }
            2820 => fail(SUCCESS),
            2867 => {
                let kind = rd.u32();
                let port = req.ports.first().map(|(_, p, _)| Arc::clone(p));
                *said = format!("type {kind}");
                match h.and_then(|h| kit.objects.get_mut(&h)) {
                    Some(Object { obj: Obj::Connect(c), .. }) => {
                        c.notify = port.map(|p| (kind, p));
                        fail(SUCCESS)
                    }
                    _ => fail(BAD_ARGUMENT),
                }
            }
            2852 => fail(UNSUPPORTED),
            // Memory a client shares with its task, mapped into it.
            2863 => {
                let kind = rd.u32();
                let want = rd.u64();
                let _size = rd.u64();
                let flags = rd.u32();
                *said = format!("type {kind} flags {flags:#x}");
                let Some(Object { obj: Obj::Connect(c), .. }) = h.and_then(|h| kit.objects.get_mut(&h)) else { return fail(BAD_ARGUMENT) };
                // (The framebuffer's aperture and its VRAM are the one
                // memory.)
                let same = |t: u32| t == kind || matches!(c.client, Client::Fb(_)) && t != display::CURSOR_MEMORY && kind != display::CURSOR_MEMORY;
                if let Some(&(_, at, size)) = c.maps.iter().find(|(t, _, _)| same(*t)) {
                    let mut b = at.to_le_bytes().to_vec();
                    b.extend_from_slice(&size.to_le_bytes());
                    return simple(id, SUCCESS, &b);
                }
                let initial = match &c.client {
                    Client::Hid(hc) if hc.kind == 0 && kind == 0 => hid::Hid::shmem(),
                    Client::Hid(hc) if hc.kind == 3 && hc.queue.is_some_and(|(t, _)| t == kind) => hc.queue_memory().unwrap_or_default(),
                    Client::Fb(fb) => match fb.memory(kind) {
                        Some(b) => b,
                        None => return fail(BAD_ARGUMENT),
                    },
                    _ => return fail(BAD_ARGUMENT),
                };
                // The framebuffer's pixels are the system's shared memory,
                // mapped shared, which gex reads without the task
                // ([`framebuffer_png`]).
                if let Client::Fb(fb) = &c.client {
                    if kind != display::CURSOR_MEMORY && flags & 1 != 0 {
                        let page = super::abi::PAGE_SIZE;
                        let size = (initial.len() as u64).next_multiple_of(crate::mem::share_granule().max(page));
                        let file = {
                            let mut v = VRAM.lock().unwrap_or_else(|e| e.into_inner());
                            if v.as_ref().is_none_or(|(_, s, _, _)| *s != size) {
                                match super::mement::new_object(size) {
                                    Ok(f) => *v = Some((f, size, (fb.mode.width, fb.mode.height, fb.row_bytes()), None)),
                                    Err(_) => return fail(0xe000_02bd_u32 as i32),
                                }
                            }
                            Arc::clone(&v.as_ref().unwrap().0)
                        };
                        let e = super::mement::Entry { file, offset: 0, size, prot: 3, data: 0 };
                        let at = match self.darwin_map_entry(m, &e, 0, size, 0, 1, 0, false, 3) {
                            Ok(at) => at,
                            Err(_) => return fail(0xe000_02bd_u32 as i32),
                        };
                        // (Where the pixels are read: the server's
                        // mapping, or the first there is.)
                        if let Ok(vm) = self.procs.vm(self.pid) {
                            let mut v = VRAM.lock().unwrap_or_else(|e| e.into_inner());
                            if let Some((.., live)) = v.as_mut() {
                                if fb.kind == 0 || live.is_none() {
                                    *live = Some((vm, gex_abi::canon(at)));
                                }
                            }
                        }
                        c.maps.push((kind, at, initial.len() as u64));
                        *said = format!("{said} -> {at:#x}+{:#x} (shared)", initial.len());
                        let mut b = at.to_le_bytes().to_vec();
                        b.extend_from_slice(&(initial.len() as u64).to_le_bytes());
                        return simple(id, SUCCESS, &b);
                    }
                }
                // kIOMapAnywhere, or at the address asked.
                let anywhere = flags & 1 != 0;
                let at = match self.darwin_vm_map_at(m, if anywhere { 0 } else { want }, initial.len() as u64, 0, anywhere as u64, 3) {
                    Ok(at) => at,
                    Err(_) => return fail(0xe000_02bd_u32 as i32), // kIOReturnNoMemory
                };
                if m.write(at, &initial).is_err() {
                    return fail(ERROR);
                }
                // (The server's shared cursor: where gex reads it, to draw
                // it as IOFramebuffer's software cursor does.)
                if let Client::Fb(fb) = &c.client {
                    if kind == display::CURSOR_MEMORY && fb.kind == 0 {
                        if let Ok(vm) = self.procs.vm(self.pid) {
                            *CURSOR.lock().unwrap_or_else(|e| e.into_inner()) = Some((vm, gex_abi::canon(at), initial.len() as u64, fb.cursor.0, self.pid));
                        }
                    }
                }
                c.maps.push((kind, at, initial.len() as u64));
                *said = format!("{said} -> {at:#x}+{:#x}", initial.len());
                let mut b = at.to_le_bytes().to_vec();
                b.extend_from_slice(&(initial.len() as u64).to_le_bytes());
                simple(id, SUCCESS, &b)
            }
            2864 => fail(SUCCESS),
            // A client's methods: selector, scalars in, a structure in
            // (inline, or out of line at an address of the task's), room
            // for scalars and a structure out.
            2865 | 2866 | 2872 => {
                if id == 2866 {
                    let n = rd.u32() as usize;
                    rd.at += 8 * n;
                }
                let selector = rd.u32();
                let scalars = rd.scalars();
                let inband = rd.bytes().to_vec();
                let ool_in = rd.u64();
                let ool_in_size = rd.u64();
                let inband_room = rd.u32() as usize;
                let scalar_room = rd.u32() as usize;
                // Room for a structure out of line, at an address of the
                // task's: what a structure larger than a message holds
                // (IOConnectCallMethod's past 4096 bytes) is written there.
                let (ool_out, ool_out_room) = if id == 2865 { (rd.u64(), rd.u64()) } else { (0, 0) };
                let inband = if ool_in != 0 && ool_in_size != 0 { m.read(ool_in, ool_in_size as usize).map(|b| b.to_vec()).unwrap_or_default() } else { inband };
                *said = format!("selector {selector} scalars {scalars:x?} struct {} bytes", inband.len());
                // (IOSurfaceRoot's client is taken out while its method
                // runs: it makes objects of the table's.)
                if let Some(Object { obj: Obj::Connect(Connect { client: Client::Surface(sc), .. }), .. }) = h.and_then(|h| kit.objects.get_mut(&h)) {
                    let mut sc = std::mem::take(sc);
                    *said = format!("{} {said}", surface::method_name(selector));
                    let room = inband_room.max(ool_out_room as usize);
                    let result = self.darwin_surface_method(m, sp, kit, &mut sc, selector, &scalars, &inband, room, said);
                    if let Some(Object { obj: Obj::Connect(Connect { client: Client::Surface(slot), .. }), .. }) = h.and_then(|h| kit.objects.get_mut(&h)) {
                        *slot = sc;
                    }
                    return method_reply(m, id, result, inband_room, scalar_room, ool_out, ool_out_room);
                }
                let Some(Object { obj: Obj::Connect(c), .. }) = h.and_then(|h| kit.objects.get_mut(&h)) else { return fail(BAD_ARGUMENT) };
                let result = match &mut c.client {
                    Client::Hid(hc) => {
                        let was = hc.cursor;
                        let r = hc.method(selector, &scalars, &inband);
                        // The cursor enabled or disabled (setCursorEnable):
                        // the framebuffer's shared cursor shown or hidden
                        // by one, as IOHIDSystem has IOFramebuffer do --
                        // `cursorShow` is a count of hidings, WindowServer
                        // keeps its own in it, and it is shown at 0.
                        if selector == 2 && hc.kind == 0 && was != hc.cursor {
                            let mine = CURSOR.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|c| c.4 == self.pid).map(|c| c.1);
                            if let Some(at) = mine {
                                if let Ok(b) = m.read(at + 8, 1).map(|b| b[0]) {
                                    let _ = m.write(at + 8, &[if hc.cursor { b.saturating_sub(1) } else { b.saturating_add(1) }]);
                                }
                            }
                        }
                        // (Where the pointer is put: WindowServer's cursor,
                        // having taken an event.)
                        if selector == 4 && hc.kind <= 1 {
                            *said = format!("{said}: the pointer to {},{}", hc.at.0, hc.at.1);
                        }
                        r
                    }
                    Client::Fb(fb) => fb.method(selector, &scalars, &inband),
                    Client::Gpu(g) => gpuw::method(selector, &inband, g),
                    Client::Amfi if selector == 3 => Ok((Vec::new(), Vec::new())),
                    Client::Surface(_) | Client::Plain | Client::Amfi => Err(UNSUPPORTED),
                };
                method_reply(m, id, result, inband_room, scalar_room, ool_out, ool_out_room)
            }
            // The catalogue of drivers: nothing to load, nothing changed.
            2836..=2841 => fail(SUCCESS),
            _ => {
                *said = format!("routine {id} is not one gex answers");
                fail(MIG_BAD_ID)
            }
        }
    }
}

impl Msg {
    /// A reply whose descriptors say it carries a right, with none: a
    /// failed open's `{ null port, NDR, result }`.
    fn complex_anyway(mut self) -> Msg {
        self.complex = true;
        self
    }
}

/// `io_connect_method`'s reply: the structure out (inline, or written where
/// the task has room out of line when larger than the reply holds) and the
/// scalars out; or the method's error.
#[allow(clippy::too_many_arguments)]
fn method_reply(m: &mut GuestMem<'_>, id: u32, result: Result<(Vec<u64>, Vec<u8>), i32>, inband_room: usize, scalar_room: usize, ool_out: u64, ool_out_room: u64) -> Msg {
    let fail = |code: i32| simple(id, code, &[]);
    match result {
        Ok((mut so, mut sb)) => {
            so.truncate(scalar_room);
            let mut ool_wrote = 0u64;
            if sb.len() > inband_room && ool_out != 0 && ool_out_room != 0 {
                let n = sb.len().min(ool_out_room as usize);
                if m.write(ool_out, &sb[..n]).is_err() {
                    return fail(BAD_ARGUMENT);
                }
                ool_wrote = n as u64;
                sb.clear();
            }
            sb.truncate(inband_room);
            let mut b = inline(&sb);
            b.extend_from_slice(&(so.len() as u32).to_le_bytes());
            so.iter().for_each(|v| b.extend_from_slice(&v.to_le_bytes()));
            if id == 2872 {
                // (no output out of line)
                let mut out = vec![0u8; 20];
                put32(&mut out, 0, 1);
                out[4 + 11] = 1;
                out.extend_from_slice(&NDR);
                out.extend_from_slice(&b);
                out.extend_from_slice(&0u32.to_le_bytes());
                Msg::kernel(id + 100, out, Vec::new(), vec![(4, Vec::new())])
            } else {
                b.extend_from_slice(&ool_wrote.to_le_bytes());
                simple(id, SUCCESS, &b)
            }
        }
        Err(code) => fail(code),
    }
}

/// The graphics comm page every client of `IOSurfaceRoot` maps: one page
/// of zeroes, the system's.
static COMM_PAGE: Mutex<Option<Arc<crate::file::OpenFile>>> = Mutex::new(None);

impl Kernel {
    /// Surface `s`'s memory mapped into this task, shared: its address.
    fn darwin_map_surface(&mut self, m: &mut GuestMem<'_>, file: &Arc<crate::file::OpenFile>, total: u64) -> Result<u64, i32> {
        let e = super::mement::Entry { file: Arc::clone(file), offset: 0, size: total, prot: 3, data: 0 };
        self.darwin_map_entry(m, &e, 0, total, 0, 1, 0, false, 3).map_err(|_| surface::NO_MEMORY)
    }

    /// Surfaces this task no longer has: unmapped, and let go of.
    fn darwin_surfaces_let_go(&mut self, m: &mut GuestMem<'_>, held: Vec<(u32, surface::Held)>) {
        for (id, h) in held {
            let total = surface::table().as_ref().and_then(|t| t.all.get(&id).map(|s| s.total));
            if let Some(total) = total {
                let _ = self.map_io().call(m, crate::syscalls::nr::MUNMAP, [h.at, total, 0, 0, 0, 0]);
            }
            if let Some(t) = surface::table().as_mut() {
                if h.uses > 0 {
                    if let Some(s) = t.all.get_mut(&id) {
                        s.use_count = s.use_count.saturating_sub(h.uses);
                        let ro = s.shared_ro();
                        let _ = s.file.handle.write_at(s.pixels, &ro);
                    }
                }
                t.let_go(id);
            }
        }
    }

    /// The shared parts of surface `id` written where its memory is (the
    /// object, and this task's mapping of it).
    fn darwin_surface_share(&mut self, m: &mut GuestMem<'_>, id: u32, at: Option<u64>) {
        let t = surface::table();
        let Some(s) = t.as_ref().and_then(|t| t.all.get(&id)) else { return };
        let ro = s.shared_ro();
        let _ = s.file.handle.write_at(s.pixels, &ro);
        if let Some(at) = at {
            let _ = m.write(surface::ro_at(s, at, 0), &ro);
        }
    }

    /// Surface `id` added to this task's table: mapped, held, and its lock
    /// result.
    fn darwin_surface_take(&mut self, m: &mut GuestMem<'_>, sc: &mut surface::Client, id: u32, created: bool) -> Result<Vec<u8>, i32> {
        let (file, total) = {
            let mut t = surface::table();
            let Some(s) = t.as_mut().and_then(|t| t.all.get_mut(&id)) else { return Err(BAD_ARGUMENT) };
            if !created {
                s.holders += 1;
            }
            (Arc::clone(&s.file), s.total)
        };
        let at = match self.darwin_map_surface(m, &file, total) {
            Ok(at) => at,
            Err(e) => {
                if let Some(t) = surface::table().as_mut() {
                    t.let_go(id);
                }
                return Err(e);
            }
        };
        sc.held.insert(id, surface::Held { at, uses: 0, created });
        // (Its pixels read where its creator, or the first task to map it,
        // draws.)
        if let Ok(vm) = self.procs.vm(self.pid) {
            if let Some(s) = surface::table().as_mut().and_then(|t| t.all.get_mut(&id)) {
                if created || s.live.is_none() {
                    s.live = Some((vm, gex_abi::canon(at)));
                }
            }
        }
        let t = surface::table();
        let s = t.as_ref().and_then(|t| t.all.get(&id)).ok_or(BAD_ARGUMENT)?;
        Ok(surface::lock_result(s, at))
    }

    /// `IOSurfaceRootUserClient`'s methods, as IOSurface.framework calls
    /// them: the scalars and the structure out, or an `IOReturn`.
    #[allow(clippy::too_many_arguments)]
    fn darwin_surface_method(&mut self, m: &mut GuestMem<'_>, sp: &mut Space, kit: &mut IoKit, sc: &mut surface::Client, selector: u32, scalars: &[u64], inband: &[u8], room: usize, said: &mut String) -> Result<(Vec<u64>, Vec<u8>), i32> {
        let none = Ok((Vec::new(), Vec::new()));
        let arg = |i: usize| scalars.get(i).copied().unwrap_or(0);
        // A surface this task has, by the id a request names.
        let held = |sc: &surface::Client, id: u32| sc.held.get(&id).cloned().ok_or(BAD_ARGUMENT);
        let id_in = get32(inband, 0);
        // A C string at 8 of a structure in (a value's key): none when empty.
        let key = || -> Option<String> {
            let s = inband.get(8..)?;
            let s = s.split(|&c| c == 0).next()?;
            (!s.is_empty()).then(|| String::from_utf8_lossy(s).into_owned())
        };
        let page = super::abi::PAGE_SIZE;
        match selector {
            // create_surface(properties): a new surface, in this task.
            0 => {
                let Some(dict) = unserialize(inband) else { return Err(BAD_ARGUMENT) };
                let Some(shape) = surface::Shape::parse(&dict) else {
                    *said = format!("{said}: refused {}", brief(&dict));
                    return Err(BAD_ARGUMENT);
                };
                let pixels = shape.pixel_bytes(page);
                let total = (pixels + page).next_multiple_of(crate::mem::share_granule().max(page));
                let file = super::mement::new_object(total).map_err(|_| surface::NO_MEMORY)?;
                let id = {
                    let mut t = surface::table();
                    let t = t.get_or_insert_with(surface::Surfaces::get);
                    let id = t.new_id();
                    *said = format!("{said}: {}x{} {:?} {} bytes a row -> surface {id}", shape.width, shape.height, String::from_utf8_lossy(&shape.pixel_format.to_be_bytes()), shape.bytes_per_row);
                    let s = surface::Surface {
                        id,
                        shape,
                        file: Arc::clone(&file),
                        pixels,
                        total,
                        creator: self.pid,
                        values: vec![("CreationProperties".to_string(), dict)],
                        generation: 1,
                        seed: 0,
                        use_count: 0,
                        holders: 1,
                        attachments: Vec::new(),
                        registry_id: 0x1_0001_0000 + id as u64,
                        live: None,
                    };
                    let _ = file.handle.write_at(pixels, &s.shared_ro());
                    t.all.insert(id, s);
                    id
                };
                Ok((Vec::new(), self.darwin_surface_take(m, sc, id, true)?))
            }
            // release_surface(id)
            1 => {
                let id = arg(0) as u32;
                let h = sc.held.remove(&id).ok_or(BAD_ARGUMENT)?;
                self.darwin_surfaces_let_go(m, vec![(id, h)]);
                none
            }
            // lock_surface({id, options}), unlock_surface: the lock result
            // with the seed; the seed, which a write's unlock moves on.
            2 | 3 => {
                let h = held(sc, id_in)?;
                let options = get32(inband, 4);
                *said = format!("{said}: surface {id_in} options {options:#x}");
                let out = {
                    let mut t = surface::table();
                    let s = t.as_mut().and_then(|t| t.all.get_mut(&id_in)).ok_or(BAD_ARGUMENT)?;
                    if selector == 3 && options & 1 == 0 {
                        s.seed = s.seed.wrapping_add(1);
                    }
                    if selector == 2 { surface::lock_result(s, h.at) } else { s.seed.to_le_bytes().to_vec() }
                };
                if selector == 3 {
                    self.darwin_surface_share(m, id_in, Some(h.at));
                }
                Ok((Vec::new(), out))
            }
            // lookup_surface(id): one this task made, a global one, or one
            // it has been given.
            4 | 27 => {
                let id = if selector == 4 {
                    arg(0) as u32
                } else {
                    let port = sp.port_named(self.pid, arg(0) as u32).ok_or(BAD_ARGUMENT)?;
                    let Kind::IoObject(o) = port.kind else { return Err(BAD_ARGUMENT) };
                    match kit.objects.get(&o).map(|o| &o.obj) {
                        Some(Obj::SurfaceRight(id)) => *id,
                        _ => return Err(BAD_ARGUMENT),
                    }
                };
                *said = format!("{said}: surface {id}");
                if sc.held.contains_key(&id) {
                    if selector == 4 {
                        return Err(BAD_ARGUMENT);
                    }
                    // (Had already: the framework finds it by its id.)
                    let mut b = vec![0u8; surface::LOCK_RESULT];
                    b[0..8].copy_from_slice(&u64::MAX.to_le_bytes());
                    b[24..28].copy_from_slice(&id.to_le_bytes());
                    return Ok((Vec::new(), b));
                }
                let allowed = surface::table().as_ref().and_then(|t| t.all.get(&id)).is_some_and(|s| selector == 27 || s.creator == self.pid || s.shape.global);
                if !allowed {
                    return Err(BAD_ARGUMENT);
                }
                Ok((Vec::new(), self.darwin_surface_take(m, sc, id, false)?))
            }
            // set_ycbcrmatrix(id, matrix), get_ycbcrmatrix(id)
            5 | 8 => {
                let id = arg(0) as u32;
                let h = held(sc, id)?;
                let matrix = {
                    let mut t = surface::table();
                    let s = t.as_mut().and_then(|t| t.all.get_mut(&id)).ok_or(BAD_ARGUMENT)?;
                    if selector == 5 {
                        s.shape.ycbcr = arg(1) as u32;
                    }
                    s.shape.ycbcr
                };
                if selector == 5 {
                    self.darwin_surface_share(m, id, Some(h.at));
                    none
                } else {
                    Ok((vec![matrix as u64], Vec::new()))
                }
            }
            // set_value({id, [value, key]}), remove_value({id, key}): the
            // values' new generation.
            9 | 11 => {
                let h = held(sc, id_in)?;
                let generation = {
                    let mut t = surface::table();
                    let s = t.as_mut().and_then(|t| t.all.get_mut(&id_in)).ok_or(BAD_ARGUMENT)?;
                    if selector == 9 {
                        let Some(Value::Array(pair)) = inband.get(8..).and_then(unserialize) else { return Err(BAD_ARGUMENT) };
                        let value = pair.first().cloned().ok_or(BAD_ARGUMENT)?;
                        match pair.get(1) {
                            Some(Value::Str(k)) => {
                                *said = format!("{said}: surface {id_in} {k}");
                                if k != "CreationProperties" {
                                    match s.values.iter_mut().find(|(n, _)| n == k) {
                                        Some(slot) => slot.1 = value,
                                        None => s.values.push((k.clone(), value)),
                                    }
                                }
                            }
                            None => {
                                let Value::Dict(d) = value else { return Err(BAD_ARGUMENT) };
                                *said = format!("{said}: surface {id_in} {} values", d.len());
                                for (k, v) in d {
                                    if k == "CreationProperties" {
                                        continue;
                                    }
                                    match s.values.iter_mut().find(|(n, _)| *n == k) {
                                        Some(slot) => slot.1 = v,
                                        None => s.values.push((k, v)),
                                    }
                                }
                            }
                            Some(_) => return Err(BAD_ARGUMENT),
                        }
                    } else {
                        match key() {
                            Some(k) if k != "CreationProperties" => s.values.retain(|(n, _)| *n != k),
                            Some(_) => {}
                            None => s.values.clear(),
                        }
                    }
                    s.generation = (s.generation + 2) | 1;
                    s.generation
                };
                self.darwin_surface_share(m, id_in, Some(h.at));
                Ok((Vec::new(), generation.to_le_bytes().to_vec()))
            }
            // get_value({id, key}): {0, generation, the value serialized};
            // get_data_value: a data value's bytes.
            10 | 21 => {
                held(sc, id_in)?;
                let key = key();
                *said = format!("{said}: surface {id_in} {}", key.as_deref().unwrap_or("(all)"));
                let t = surface::table();
                let s = t.as_ref().and_then(|t| t.all.get(&id_in)).ok_or(BAD_ARGUMENT)?;
                let value = match &key {
                    Some(k) => s.values.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()),
                    None => Some(Value::Dict(s.values.clone())),
                };
                let Some(value) = value else { return none };
                if selector == 21 {
                    let Value::Data(d) = value else { return Err(0xe000_02c7_u32 as i32) };
                    return if d.len() > room { Err(surface::NO_SPACE) } else { Ok((Vec::new(), d)) };
                }
                let mut b = 0u32.to_le_bytes().to_vec();
                b.extend_from_slice(&s.generation.to_le_bytes());
                b.extend_from_slice(&plist::to_binary(&value));
                if b.len() > room {
                    return Err(surface::NO_SPACE);
                }
                Ok((Vec::new(), b))
            }
            // bind_accel, set_surface_notify, remove_surface_notify, log,
            // set_gpu_policy_dict, the shared events' signals: taken.
            12 | 17 | 18 | 19 | 30 | 32 | 33 | 34 | 35 | 36 => none,
            13 => Ok((Vec::new(), surface::limits())),
            // increment_surface_use_count, decrement_, get_
            14 | 15 | 16 => {
                let id = arg(0) as u32;
                let mut h = held(sc, id)?;
                let count = {
                    let mut t = surface::table();
                    let s = t.as_mut().and_then(|t| t.all.get_mut(&id)).ok_or(BAD_ARGUMENT)?;
                    match selector {
                        14 => {
                            s.use_count += 1;
                            h.uses += 1;
                        }
                        15 if h.uses > 0 => {
                            s.use_count -= 1;
                            h.uses -= 1;
                        }
                        _ => {}
                    }
                    s.use_count
                };
                if selector == 16 {
                    return Ok((vec![count as u64], Vec::new()));
                }
                let at = h.at;
                sc.held.insert(id, h);
                self.darwin_surface_share(m, id, Some(at));
                none
            }
            // set_purgeable(id, state): the state it was, never volatile.
            20 => {
                held(sc, arg(0) as u32)?;
                Ok((vec![0], Vec::new()))
            }
            // set_bulk_attachments({id, attachments}), get_bulk_attachments(id)
            22 => {
                let h = held(sc, id_in)?;
                {
                    let mut t = surface::table();
                    let s = t.as_mut().and_then(|t| t.all.get_mut(&id_in)).ok_or(BAD_ARGUMENT)?;
                    s.attachments = inband.get(8..).unwrap_or(&[]).to_vec();
                }
                self.darwin_surface_share(m, id_in, Some(h.at));
                none
            }
            23 => {
                let id = arg(0) as u32;
                held(sc, id)?;
                let t = surface::table();
                let s = t.as_ref().and_then(|t| t.all.get(&id)).ok_or(BAD_ARGUMENT)?;
                let mut b = vec![0u8; 0x80];
                let n = s.attachments.len().min(0x78);
                b[8..8 + n].copy_from_slice(&s.attachments[..n]);
                Ok((Vec::new(), b))
            }
            // gather_iosurface_data: nothing gathered.
            24 => none,
            // get_graphics_comm_page_address: the page, mapped once.
            25 => {
                if sc.comm_page == 0 {
                    let file = {
                        let mut c = COMM_PAGE.lock().unwrap_or_else(|e| e.into_inner());
                        if c.is_none() {
                            *c = Some(super::mement::new_object(page).map_err(|_| surface::NO_MEMORY)?);
                        }
                        Arc::clone(c.as_ref().unwrap())
                    };
                    let size = page.next_multiple_of(crate::mem::share_granule().max(page));
                    let e = super::mement::Entry { file, offset: 0, size, prot: 1, data: 0 };
                    sc.comm_page = self.darwin_map_entry(m, &e, 0, size, 0, 1, 0, false, 1).map_err(|_| surface::NO_MEMORY)?;
                }
                Ok((vec![sc.comm_page], Vec::new()))
            }
            // set_indexed_timestamp(id, index, value)
            26 => {
                let id = arg(0) as u32;
                let h = held(sc, id)?;
                if arg(1) >= 4 {
                    return Err(BAD_ARGUMENT);
                }
                let t = surface::table();
                let s = t.as_ref().and_then(|t| t.all.get(&id)).ok_or(BAD_ARGUMENT)?;
                let at = surface::rw_at(s, h.at, 8 * arg(1));
                drop(t);
                m.write(at, &arg(2).to_le_bytes()).map_err(|_| BAD_ARGUMENT)?;
                none
            }
            // create_port_from_surface(id): a send right naming it, which
            // keeps it.
            28 => {
                let id = arg(0) as u32;
                held(sc, id)?;
                if let Some(s) = surface::table().as_mut().and_then(|t| t.all.get_mut(&id)) {
                    s.holders += 1;
                }
                let o = kit.add(Obj::SurfaceRight(id));
                let name = sp.give_send(Arc::clone(&kit.objects[&o].port));
                *said = format!("{said}: surface {id} -> name {name:#x}");
                Ok((vec![name as u64], Vec::new()))
            }
            // create_shared_event, query_shared_event
            29 => {
                sc.next_event += 1;
                Ok((vec![sc.next_event], Vec::new()))
            }
            31 => Ok((vec![0, 0], Vec::new())),
            // get_pid_gpu_policy_dict: none kept.
            37 => Err(UNSUPPORTED),
            // set_detach_mode_code(id, mask, value, time)
            38 => {
                let id = arg(0) as u32;
                let h = held(sc, id)?;
                let t = surface::table();
                let s = t.as_ref().and_then(|t| t.all.get(&id)).ok_or(BAD_ARGUMENT)?;
                let at = surface::rw_at(s, h.at, surface::DETACH);
                drop(t);
                let mut cur = [0u8; 8];
                if let Ok(b) = m.read(at, 8) {
                    cur.copy_from_slice(b);
                }
                let v = u64::from_le_bytes(cur) & !arg(1) | arg(2) & arg(1);
                m.write(at, &v.to_le_bytes()).map_err(|_| BAD_ARGUMENT)?;
                none
            }
            // create_surface_fast_path, create_surface_client_mem: the
            // framework of 10.14 does not use them.
            _ => Err(UNSUPPORTED),
        }
    }
}
