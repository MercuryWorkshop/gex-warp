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
pub mod hid;
pub mod plist;
pub mod registry;

use super::port::{get32, post, put32, Kind, Msg, Port, MIG_BAD_ID, NDR};
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
    /// A client with no methods of its own (a family's that nothing here
    /// drives yet): every method is unsupported.
    Plain,
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
    for o in kit.objects.values() {
        if let Obj::Connect(Connect { client: Client::Hid(h), notify: Some((_, port)), .. }) = &o.obj {
            if h.kind == 0 {
                post(port, Msg::kernel(0, Vec::new(), Vec::new(), Vec::new()));
            }
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
        for o in kit.objects.values_mut() {
            let Obj::Connect(c) = &mut o.obj else { continue };
            if c.pid != self.pid {
                continue;
            }
            let Client::Hid(h) = &mut c.client else { continue };
            if h.kind != 0 {
                continue;
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

    pub(super) fn darwin_iokit(&mut self, m: &mut GuestMem<'_>, kind: Kind, req: &Msg) -> Msg {
        self.darwin_hid_drain(m);
        let h = match kind {
            Kind::IoObject(h) => Some(h),
            _ => None,
        };
        let mut io = IOKIT.lock().unwrap_or_else(|e| e.into_inner());
        let kit = io.get_or_insert_with(IoKit::new);
        let mut said = String::new();
        let reply = self.darwin_iokit_routine(m, kit, h, req, &mut said);
        if self.trace || std::env::var_os("GEX_IOKIT_TRACE").is_some() {
            // (An error reply is `{ NDR, code }` and not complex.)
            let code = if reply.complex { 0 } else { get32(&reply.body, 8) as i32 };
            let answer = if code == 0 { String::new() } else { format!(" -> {:#x}", code as u32) };
            eprintln!("[gex:{}] IOKit {} {} on {}{}{}", self.pid, req.id, routine(req.id), kit.describe(h), if said.is_empty() { String::new() } else { format!(": {said}") }, answer);
        }
        reply
    }

    fn darwin_iokit_routine(&mut self, m: &mut GuestMem<'_>, kit: &mut IoKit, h: Option<u32>, req: &Msg, said: &mut String) -> Msg {
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
                            Client::Plain => "IOUserClient",
                        },
                        Obj::Iter(..) => "IOUserIterator",
                        Obj::Note => "IOServiceUserNotification",
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
                    Opens::HidSystem | Opens::Framebuffer | Opens::Nothing => {
                        *said = format!("type {kind}: {class} has no such user client");
                        return no_port(id, Some(&UNSUPPORTED.to_le_bytes()));
                    }
                };
                let c = kit.add(Obj::Connect(Connect { entry: e, pid: self.pid, client, notify: None, maps: Vec::new() }));
                let port = Arc::clone(&kit.objects[&c].port);
                with_port(id, port, Some(&SUCCESS.to_le_bytes()))
            }
            2816 => fail(SUCCESS),
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
                    Client::Fb(fb) => match fb.memory(kind) {
                        Some(b) => b,
                        None => return fail(BAD_ARGUMENT),
                    },
                    _ => return fail(BAD_ARGUMENT),
                };
                // kIOMapAnywhere, or at the address asked.
                let anywhere = flags & 1 != 0;
                let at = match self.darwin_vm_map_at(m, if anywhere { 0 } else { want }, initial.len() as u64, 0, anywhere as u64, 3) {
                    Ok(at) => at,
                    Err(_) => return fail(0xe000_02bd_u32 as i32), // kIOReturnNoMemory
                };
                if m.write(at, &initial).is_err() {
                    return fail(ERROR);
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
                let inband = if ool_in != 0 && ool_in_size != 0 { m.read(ool_in, ool_in_size as usize).map(|b| b.to_vec()).unwrap_or_default() } else { inband };
                *said = format!("selector {selector} scalars {scalars:x?} struct {} bytes", inband.len());
                let Some(Object { obj: Obj::Connect(c), .. }) = h.and_then(|h| kit.objects.get_mut(&h)) else { return fail(BAD_ARGUMENT) };
                let result = match &mut c.client {
                    Client::Hid(hc) => hc.method(selector, &scalars, &inband),
                    Client::Fb(fb) => fb.method(selector, &scalars, &inband),
                    Client::Plain => Err(UNSUPPORTED),
                };
                match result {
                    Ok((mut so, mut sb)) => {
                        so.truncate(scalar_room);
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
                            b.extend_from_slice(&0u64.to_le_bytes());
                            simple(id, SUCCESS, &b)
                        }
                    }
                    Err(code) => fail(code),
                }
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
