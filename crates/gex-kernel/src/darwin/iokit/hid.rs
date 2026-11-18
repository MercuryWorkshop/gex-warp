//! `IOHIDSystem`: the HID system's service, and the user clients
//! WindowServer opens on it (`IOHIDFamily`'s `IOHIDUserClient` and
//! `IOHIDParamUserClient`).
//!
//! The server connection (type 0) has the system's shared memory: an
//! `EvOffsets` and, after it, the `EvGlobals` (`IOHIDShared.h`, version 4):
//! the cursor's place, the state of the buttons and modifier keys, and the
//! low-level event queue (`lleq`), a ring of `NXEQElement`s linked by index
//! that the kernel writes at its tail and WindowServer reads from its head.
//!
//! Input comes into it through [`post`]: an event a host has (a key, the
//! pointer moving, a button) is queued on the system, which kicks the
//! port its consumer gave it (`IOConnectSetNotificationPort`). The queued
//! events go into the consumer's memory in its own task, the next time it
//! calls IOKit or receives the kick: a task's memory is written by the
//! kernel only while it runs for that task, here and on the page alike.
//!
//! The consumer is the HID event system (hidd's), when it has a queue on
//! the system: on Mojave the system is one of hidd's services (its
//! `HIDServiceSupport`, the NX event router its plug-in), whose events
//! come through an event queue made on the event system's connection
//! (type 3, `IOHIDEventQueueCreate`): each an `IOHIDEvent` of Apple's
//! vendor page holding an `NXEvent` ([`Hid::queue_entries`]), which hidd
//! hands to WindowServer, which takes it for the event it says. Without
//! one, the events go into the shared memory's low-level queue
//! ([`Hid::drain`]), which a WindowServer of before reads. A host posts what it is given as
//! lines of text ([`Line::parse`]): a viewer's mouse and keys, or a
//! script's.

use super::plist::Value;
use std::collections::VecDeque;
use std::sync::Mutex;

/// `EvOffsets` then `EvGlobals`, rounded to pages as the kernel maps it.
pub const SHMEM_SIZE: u64 = 0x6000;
const EVG: usize = 8;
const VERSION: i32 = 4;
const EVGLOBALS_SIZE: i32 = 23264;
/// `LLEQSIZE` entries of `NXEQElement` (96 bytes) at offset 224.
const LLEQ: usize = 224;
const LLEQ_SIZE: usize = 240;
const ELEMENT: usize = 96;
/// `INITEVENTNUM`: the first number an event is given.
const INIT_EVENT_NUM: i32 = 13;

/// An event a host gives the HID system.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    /// The pointer is at a place on the desktop.
    Move { x: i32, y: i32 },
    /// A button (0 the left) went down or up.
    Button { button: u8, down: bool },
    /// A key went down or up: its key code (`kVK_*`, the ADB code).
    Key { code: u16, down: bool },
    /// The wheel turned, in lines.
    Scroll { dx: i32, dy: i32 },
}

/// A line of input as a host is given it (`gexc`'s `GEX_HID_INPUT`, a
/// viewer, a script): `move X Y`, `down [BUTTON]`, `up [BUTTON]`,
/// `click X Y [BUTTON]` (a move, then the button down and up), `key CODE
/// down|up`, `press CODE` (down, then up), `scroll DX DY`, `sleep MS` (a
/// pause between the events around it). Buttons count from 0, the left;
/// key codes are the Mac's (`kVK_*`). A blank line or one starting with
/// `#` says nothing.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    Events(Vec<Input>),
    Sleep(u64),
}

impl Line {
    pub fn parse(line: &str) -> Result<Line, String> {
        let w: Vec<&str> = line.split_whitespace().collect();
        let Some(&verb) = w.first() else { return Ok(Line::Events(Vec::new())) };
        if verb.starts_with('#') {
            return Ok(Line::Events(Vec::new()));
        }
        let n = |i: usize| -> Result<i32, String> {
            w.get(i).ok_or_else(|| format!("{verb}: too few words"))?.parse::<i32>().map_err(|e| format!("{verb}: {e}"))
        };
        let button = |i: usize| -> Result<u8, String> {
            match w.get(i) {
                None => Ok(0),
                Some(b) => b.parse::<u8>().map_err(|e| format!("{verb}: {e}")),
            }
        };
        Ok(Line::Events(match verb {
            "move" => vec![Input::Move { x: n(1)?, y: n(2)? }],
            "down" => vec![Input::Button { button: button(1)?, down: true }],
            "up" => vec![Input::Button { button: button(1)?, down: false }],
            "click" => {
                let b = button(3)?;
                vec![Input::Move { x: n(1)?, y: n(2)? }, Input::Button { button: b, down: true }, Input::Button { button: b, down: false }]
            }
            "key" => {
                let down = match w.get(2) {
                    Some(&"down") => true,
                    Some(&"up") => false,
                    _ => return Err("key: down or up?".into()),
                };
                vec![Input::Key { code: n(1)? as u16, down }]
            }
            "press" => vec![Input::Key { code: n(1)? as u16, down: true }, Input::Key { code: n(1)? as u16, down: false }],
            "scroll" => vec![Input::Scroll { dx: n(1)?, dy: n(2)? }],
            "sleep" => return Ok(Line::Sleep(n(1)?.max(0) as u64)),
            _ => return Err(format!("{verb}: not an input")),
        }))
    }
}

/// What the hosts have posted and not yet been delivered, and the port to
/// kick when there is some.
static INPUT: Mutex<VecDeque<Input>> = Mutex::new(VecDeque::new());

/// Give the HID system an event. (What takes it into WindowServer is
/// [`Hid::drain`].)
pub fn post(event: Input) {
    INPUT.lock().unwrap_or_else(|e| e.into_inner()).push_back(event);
    PENDING.store(true, std::sync::atomic::Ordering::Release);
    super::kick_hid();
}

/// Whether anything is posted and not taken: what a task's every receive
/// asks before it looks further ([`super::Kernel::darwin_hid_drain`]).
static PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn pending() -> bool {
    PENDING.load(std::sync::atomic::Ordering::Acquire)
}

/// The events posted and not yet taken.
pub fn take() -> Vec<Input> {
    let mut q = INPUT.lock().unwrap_or_else(|e| e.into_inner());
    PENDING.store(false, std::sync::atomic::Ordering::Release);
    q.drain(..).collect()
}

/// The character an ANSI keyboard's key (`kVK_*`) types unshifted, for
/// those that type one.
fn ascii(code: u16) -> Option<u8> {
    const MAIN: &[u8; 0x33] = b"asdfhgzxcv\0bqweryt123465=97-80]ou[ip\rlj'k;\\,/nm.\t `";
    match code {
        0..=0x32 => Some(MAIN[code as usize]).filter(|&c| c != 0),
        0x33 => Some(0x7f),
        0x35 => Some(0x1b),
        0x41 => Some(b'.'),
        0x43 => Some(b'*'),
        0x45 => Some(b'+'),
        0x4b => Some(b'/'),
        0x4c => Some(3),
        0x4e => Some(b'-'),
        0x51 => Some(b'='),
        0x52..=0x59 => Some(b'0' + (code - 0x52) as u8),
        0x5b => Some(b'8'),
        0x5c => Some(b'9'),
        _ => None,
    }
}

/// A connection to the HID system.
pub struct Hid {
    /// `kIOHIDServerConnectType` (0), `kIOHIDParamConnectType` (1) or
    /// `kIOHIDEventSystemConnectType` (3).
    pub kind: u32,
    pub events: bool,
    pub cursor: bool,
    /// The cursor's place, as the shared memory last said it.
    pub at: (i32, i32),
    pub bounds: [i16; 4],
    /// The event system's queue (type 3): the memory type it is mapped as
    /// and its size, as `IOHIDEventQueueCreate` asked.
    pub queue: Option<(u32, u32)>,
    /// The events made so far (`NXEventData`'s event number).
    pub made: u16,
}

/// The memory type the event system's queue is mapped as: what creating
/// it answers.
pub const QUEUE_TOKEN: u32 = 0x4851;
/// `IODataQueueMemory`'s header: its size, head and tail.
pub const QUEUE_HEADER: usize = 12;
/// `kHIDPage_AppleVendor`, and its usage for an NX event
/// (`kHIDUsage_AppleVendor_NXEvent`): what WindowServer takes from the
/// system's service.
const APPLE_VENDOR: u16 = 0xff00;
const NX_EVENT_USAGE: u16 = 23;
/// `kIOHIDEventTypeVendorDefined`.
const VENDOR_DEFINED: u32 = 1;

/// `NX_MOUSEMOVED`, `NX_LMOUSEDOWN` and the rest of `NXEventData`'s types.
mod nx {
    pub const LMOUSEDOWN: i32 = 1;
    pub const LMOUSEUP: i32 = 2;
    pub const RMOUSEDOWN: i32 = 3;
    pub const RMOUSEUP: i32 = 4;
    pub const MOUSEMOVED: i32 = 5;
    pub const KEYDOWN: i32 = 10;
    pub const KEYUP: i32 = 11;
    pub const SCROLLWHEELMOVED: i32 = 22;
    pub const OMOUSEDOWN: i32 = 25;
    pub const OMOUSEUP: i32 = 26;
}

impl Hid {
    pub fn new(kind: u32) -> Hid {
        Hid { kind, events: false, cursor: false, at: (0, 0), bounds: [0; 4], queue: None, made: 0 }
    }

    /// The shared memory as `IOHIDSystem::initShmem` leaves it: the
    /// offsets, the globals' version and size, the wait cursor's settings,
    /// and an empty queue whose elements are linked in a ring.
    pub fn shmem() -> Vec<u8> {
        let mut b = vec![0u8; SHMEM_SIZE as usize];
        let mut put = |at: usize, v: i32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(0, EVG as i32);
        put(EVG + 4, INIT_EVENT_NUM);
        put(EVG + 44, VERSION);
        put(EVG + 48, EVGLOBALS_SIZE);
        put(EVG + 52, 3);
        put(EVG + 208, 1200);
        for i in 0..LLEQ_SIZE {
            put(EVG + LLEQ + i * ELEMENT, ((i + 1) % LLEQ_SIZE) as i32);
        }
        put(EVG + 212, 1);
        put(EVG + 216, 1);
        put(EVG + 220, 0);
        b[EVG + 205] = 1;
        b[EVG + 206] = 1;
        b
    }

    /// `IOHIDUserClient`'s methods (and `IOHIDParamUserClient`'s, which
    /// are the same numbers from 3 on): the scalars and structure out, or
    /// an `IOReturn`.
    pub fn method(&mut self, selector: u32, scalars: &[u64], inband: &[u8]) -> Result<(Vec<u64>, Vec<u8>), i32> {
        let none = Ok((Vec::new(), Vec::new()));
        match (self.kind, selector) {
            // createShmem(version), setEventsEnable(on), setCursorEnable(on)
            (0, 0) => none,
            (0, 1) => {
                self.events = scalars.first().is_some_and(|&v| v != 0);
                none
            }
            (0, 2) => {
                self.cursor = scalars.first().is_some_and(|&v| v != 0);
                none
            }
            // postEvent(struct evioLLEvent, int): taken, and nowhere to go.
            (0 | 1, 3) => none,
            // setMouseLocation(IOFixedPoint32 or IOGPoint)
            (0 | 1, 4) => {
                if inband.len() >= 8 {
                    let x = i32::from_le_bytes(inband[0..4].try_into().unwrap());
                    let y = i32::from_le_bytes(inband[4..8].try_into().unwrap());
                    self.at = if inband.len() >= 12 { (x >> 8, y >> 8) } else { (x, y) };
                }
                none
            }
            // getButtonEventNum(button) and getStateForSelector(selector):
            // nothing pressed, nothing locked.
            (0 | 1, 5) => Ok((vec![0], Vec::new())),
            // setBounds(IOGBounds) of the server's; setStateForSelector of
            // the parameter connection's.
            (0, 6) => {
                for (i, c) in inband.chunks(2).take(4).enumerate() {
                    if c.len() == 2 {
                        self.bounds[i] = i16::from_le_bytes([c[0], c[1]]);
                    }
                }
                none
            }
            (1, 6) => none,
            // registerVirtualDisplay: a token for it; unregistering and
            // bounding one are done.
            (0 | 1, 7) => Ok((vec![1], Vec::new())),
            (0 | 1, 8 | 9 | 11) => none,
            // getUserHidActivityState: active.
            (0 | 1, 10) => Ok((vec![0], Vec::new())),
            // setOnScreenBounds(IOGPoint, IOGBounds)
            (0, 12) => none,
            // The event system's: createEventQueue(type, size), whose
            // answer is the memory type it is mapped as;
            // destroyEventQueue(type, token); tickle.
            (3, 0) => {
                let size = scalars.get(1).copied().unwrap_or(0x4000).clamp(0x1000, 0x10_0000) as u32;
                self.queue = Some((QUEUE_TOKEN, size));
                Ok((vec![QUEUE_TOKEN as u64], Vec::new()))
            }
            (3, _) => none,
            _ => Err(super::UNSUPPORTED),
        }
    }

    /// What `setMouseLocation` (selector 4) writes into the shared memory's
    /// globals, as IOHIDSystem's `_setCursorPosition` does, at their offsets
    /// in the memory: `cursorLoc` (two shorts) and the cursor's place in
    /// 24.8 fixed point on the screen and on the desktop
    /// (`screenCursorFixed`, `desktopCursorFixed`: one screen, at the
    /// desktop's origin). `at` is the place in whole points; `inband` the
    /// request, whose fixed-point form (12 bytes) is kept to the bit.
    pub fn cursor_globals(at: (i32, i32), inband: &[u8]) -> Vec<(usize, Vec<u8>)> {
        let (fx, fy) = if inband.len() >= 12 {
            (i32::from_le_bytes(inband[0..4].try_into().unwrap()), i32::from_le_bytes(inband[4..8].try_into().unwrap()))
        } else {
            (at.0 << 8, at.1 << 8)
        };
        let mut loc = (at.0 as i16).to_le_bytes().to_vec();
        loc.extend_from_slice(&(at.1 as i16).to_le_bytes());
        let mut fixed = fx.to_le_bytes().to_vec();
        fixed.extend_from_slice(&fy.to_le_bytes());
        vec![(EVG + 20, loc), (EVG + 56, fixed.clone()), (EVG + 64, fixed)]
    }

    /// Write the posted events into the shared memory `shm` (the task's,
    /// as bytes), as `IOHIDSystem::postEvent` would put each in the queue:
    /// the cursor moved, the buttons' state, an element at the tail. False
    /// if there was nothing.
    pub fn drain(&mut self, shm: &mut [u8], events: &[Input], now_ns: u64) -> bool {
        if events.is_empty() || shm.len() < EVG + LLEQ + LLEQ_SIZE * ELEMENT {
            return false;
        }
        let rd = |b: &[u8], at: usize| i32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        let wr = |b: &mut [u8], at: usize, v: i32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        for ev in events {
            // The globals: the cursor's place, the buttons down.
            match *ev {
                Input::Move { x, y } => {
                    shm[EVG + 20..EVG + 22].copy_from_slice(&(x as i16).to_le_bytes());
                    shm[EVG + 22..EVG + 24].copy_from_slice(&(y as i16).to_le_bytes());
                    wr(shm, EVG + 64, x << 8);
                    wr(shm, EVG + 68, y << 8);
                }
                Input::Button { button, down } => {
                    let buttons = rd(shm, EVG + 8);
                    wr(shm, EVG + 8, if down { buttons | 1 << button } else { buttons & !(1 << button) });
                }
                _ => {}
            }
            // The tail element is filled, and the tail moves on unless the
            // ring is full (then the oldest is written over in place).
            let tail = rd(shm, EVG + 216) as usize % LLEQ_SIZE;
            let el = EVG + LLEQ + tail * ELEMENT;
            let num = rd(shm, EVG + 4);
            wr(shm, EVG + 4, num.wrapping_add(1));
            let mut nx = self.nx_event(ev, now_ns);
            nx[20..24].copy_from_slice(&shm[EVG + 12..EVG + 16]);
            shm[el + 8..el + 8 + 88].copy_from_slice(&nx);
            wr(shm, EVG + 220, tail as i32);
            let next = rd(shm, el) as usize % LLEQ_SIZE;
            if next != rd(shm, EVG + 212) as usize % LLEQ_SIZE {
                wr(shm, EVG + 216, next as i32);
            }
        }
        true
    }

    /// The `NXEvent` an input is (88 bytes, packed to four): its type,
    /// where the pointer is, the time (in the Mach clock's ticks), the
    /// modifier flags, the window, the service and the pid, then its
    /// `NXEventData`.
    pub fn nx_event(&mut self, ev: &Input, now: u64) -> [u8; 88] {
        let mut e = [0u8; 88];
        // What `NXEventData` holds for it, as bytes at their offsets.
        self.made = self.made.wrapping_add(1);
        let (what, data): (i32, Vec<(usize, Vec<u8>)>) = match *ev {
            Input::Move { x, y } => {
                self.at = (x, y);
                (nx::MOUSEMOVED, Vec::new())
            }
            Input::Button { button, down } => {
                let what = match (button, down) {
                    (0, true) => nx::LMOUSEDOWN,
                    (0, false) => nx::LMOUSEUP,
                    (1, true) => nx::RMOUSEDOWN,
                    (1, false) => nx::RMOUSEUP,
                    (_, true) => nx::OMOUSEDOWN,
                    (_, false) => nx::OMOUSEUP,
                };
                // mouse: the event's number, the click state (one click),
                // the pressure (all of it while down), the button
                (what, vec![(2, self.made.to_le_bytes().to_vec()), (4, 1i32.to_le_bytes().to_vec()), (8, vec![if down { 255 } else { 0 }, button])])
            }
            // key: the character it is (ASCII, as the keyboard's map says
            // it unshifted, as both the original and the one meant), the
            // key code, on an ANSI keyboard
            Input::Key { code, down } => {
                let c = ascii(code).unwrap_or(0) as u16;
                (if down { nx::KEYDOWN } else { nx::KEYUP }, vec![(6, c.to_le_bytes().to_vec()), (8, code.to_le_bytes().to_vec()), (10, c.to_le_bytes().to_vec()), (16, 40u32.to_le_bytes().to_vec())])
            }
            // scrollWheel: the axes, vertical first
            Input::Scroll { dx, dy } => (nx::SCROLLWHEELMOVED, vec![(0, (dy as i16).to_le_bytes().to_vec()), (2, (dx as i16).to_le_bytes().to_vec())]),
        };
        e[0..4].copy_from_slice(&what.to_le_bytes());
        e[4..8].copy_from_slice(&self.at.0.to_le_bytes());
        e[8..12].copy_from_slice(&self.at.1.to_le_bytes());
        e[12..20].copy_from_slice(&now.to_le_bytes());
        for (at, bytes) in data {
            e[40 + at..40 + at + bytes.len()].copy_from_slice(&bytes);
        }
        e
    }

    /// The events as the event system's queue holds them, each an
    /// `IODataQueueEntry` (its size, then its bytes) whose bytes are an
    /// `IOHIDEvent` as `IOHIDEventCreateWithBytes` reads one: the time,
    /// the sender (the system's registry id), options, the attributes'
    /// length and the number of events (one), then the event, a
    /// vendor-defined one (its size, type, options and depth, the usage
    /// page and usage, a version, the data's length, the data): Apple's
    /// NX event, the `NXEvent`.
    pub fn queue_entries(&mut self, events: &[Input], now: u64, sender: u64) -> Vec<Vec<u8>> {
        events
            .iter()
            .map(|ev| {
                let nx = self.nx_event(ev, now);
                let mut b = Vec::with_capacity(4 + 28 + 28 + nx.len());
                let event_size = (28 + nx.len()) as u32;
                b.extend_from_slice(&(28 + event_size).to_le_bytes());
                b.extend_from_slice(&now.to_le_bytes());
                b.extend_from_slice(&sender.to_le_bytes());
                b.extend_from_slice(&0u32.to_le_bytes());
                b.extend_from_slice(&0u32.to_le_bytes());
                b.extend_from_slice(&1u32.to_le_bytes());
                b.extend_from_slice(&event_size.to_le_bytes());
                b.extend_from_slice(&VENDOR_DEFINED.to_le_bytes());
                b.extend_from_slice(&0u32.to_le_bytes());
                b.extend_from_slice(&[0, 0, 0, 0]);
                b.extend_from_slice(&APPLE_VENDOR.to_le_bytes());
                b.extend_from_slice(&NX_EVENT_USAGE.to_le_bytes());
                b.extend_from_slice(&0u32.to_le_bytes());
                b.extend_from_slice(&(nx.len() as u32).to_le_bytes());
                b.extend_from_slice(&nx);
                b
            })
            .collect()
    }

    /// The memory of the event system's queue as the kernel gives it: all
    /// zeros (its header is written by who maps it).
    pub fn queue_memory(&self) -> Option<Vec<u8>> {
        let (_, size) = self.queue?;
        Some(vec![0u8; (QUEUE_HEADER + size as usize).next_multiple_of(0x1000)])
    }

    /// Where each entry goes in a queue whose header says `size`, `head`
    /// and `tail`, as `IODataQueue::enqueue` puts it: at the tail, or, when
    /// it does not fit before the end, at the start if the head is past
    /// it; and the tail after them. Entries that do not fit are left out.
    pub fn enqueue_at(size: u32, head: u32, mut tail: u32, entries: &[Vec<u8>]) -> (Vec<(u32, usize)>, u32) {
        let mut at = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            let n = e.len() as u32;
            if tail >= head {
                if tail.checked_add(n).is_some_and(|end| end <= size) {
                    at.push((tail, i));
                    tail += n;
                } else if head > n {
                    at.push((0, i));
                    tail = n;
                } else {
                    break;
                }
            } else if tail + n < head {
                at.push((tail, i));
                tail += n;
            } else {
                break;
            }
        }
        (at, tail)
    }

    pub fn properties() -> Value {
        Value::Dict(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_memory_begins_as_the_hid_system_leaves_it() {
        let b = Hid::shmem();
        let rd = |at: usize| i32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        assert_eq!(rd(0), 8);
        assert_eq!((rd(8 + 44), rd(8 + 48)), (4, 23264));
        // An empty queue: head and tail together, each element linked to
        // the next, the last to the first.
        assert_eq!((rd(8 + 212), rd(8 + 216)), (1, 1));
        assert_eq!(rd(8 + 224), 1);
        assert_eq!(rd(8 + 224 + 239 * 96), 0);
    }

    #[test]
    fn setting_the_mouse_location_says_it_in_the_globals() {
        // IOFixedPoint32 (and a word more): kept as given.
        let mut inband = Vec::new();
        for v in [300 << 8 | 0x80, 200 << 8, 0] {
            inband.extend_from_slice(&(v as i32).to_le_bytes());
        }
        let mut h = Hid::new(0);
        assert_eq!(h.method(4, &[], &inband).map(|(s, _)| s), Ok(vec![]));
        assert_eq!(h.at, (300, 200));
        let g = Hid::cursor_globals(h.at, &inband);
        let mut shm = Hid::shmem();
        for (at, b) in &g {
            shm[*at..*at + b.len()].copy_from_slice(b);
        }
        let rd = |at: usize| i32::from_le_bytes(shm[at..at + 4].try_into().unwrap());
        assert_eq!((i16::from_le_bytes([shm[EVG + 20], shm[EVG + 21]]), i16::from_le_bytes([shm[EVG + 22], shm[EVG + 23]])), (300, 200));
        assert_eq!((rd(EVG + 64), rd(EVG + 68)), (300 << 8 | 0x80, 200 << 8));
        assert_eq!((rd(EVG + 56), rd(EVG + 60)), (300 << 8 | 0x80, 200 << 8));
        // An IOGPoint: whole points.
        let g = Hid::cursor_globals((5, 7), &[5, 0, 0, 0, 7, 0, 0, 0]);
        assert_eq!(g[2], (EVG + 64, [5i32 << 8, 7 << 8].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>()));
    }

    #[test]
    fn the_event_system_is_given_nx_events_as_vendor_defined_events() {
        let mut h = Hid::new(3);
        assert_eq!(h.method(0, &[0, 0x4000], &[]).map(|(s, _)| s), Ok(vec![QUEUE_TOKEN as u64]));
        assert_eq!(h.queue_memory().map(|m| m.len()), Some(0x5000));
        let e = h.queue_entries(&[Input::Move { x: 300, y: 200 }, Input::Button { button: 0, down: true }], 99, 7);
        let rd32 = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        let rd16 = |b: &[u8], at: usize| u16::from_le_bytes(b[at..at + 2].try_into().unwrap());
        // An entry's size counts the element (28 bytes) and the event.
        assert_eq!(e[0].len(), 4 + 144);
        assert_eq!(rd32(&e[0], 0), 144);
        let el = &e[1][4..];
        assert_eq!(u64::from_le_bytes(el[0..8].try_into().unwrap()), 99);
        assert_eq!(u64::from_le_bytes(el[8..16].try_into().unwrap()), 7);
        assert_eq!(rd32(el, 24), 1);
        let ev = &el[28..];
        assert_eq!((rd32(ev, 0), rd32(ev, 4)), (116, 1));
        assert_eq!((rd16(ev, 16), rd16(ev, 18), rd32(ev, 24)), (0xff00, 23, 88));
        // The NX event: a left button down where the pointer was moved to.
        let nx = &ev[28..];
        assert_eq!(rd32(nx, 0) as i32, nx::LMOUSEDOWN);
        assert_eq!((rd32(nx, 4), rd32(nx, 8)), (300, 200));
        assert_eq!((nx[48], nx[49]), (255, 0));
    }

    #[test]
    fn a_key_says_the_character_it_types() {
        assert_eq!([ascii(0), ascii(0x12), ascii(0x1d), ascii(0x24), ascii(0x31), ascii(0x45), ascii(0x5c), ascii(0x0a), ascii(0x7a)], [Some(b'a'), Some(b'1'), Some(b'0'), Some(b'\r'), Some(b' '), Some(b'+'), Some(b'9'), None, None]);
        assert_eq!([ascii(0x2a), ascii(0x27), ascii(0x32)], [Some(b'\\'), Some(b'\''), Some(b'`')]);
        let nx = Hid::new(3).nx_event(&Input::Key { code: 0x13, down: true }, 0);
        assert_eq!((nx[0], nx[46], nx[48], nx[50]), (nx::KEYDOWN as u8, b'2', 0x13, b'2'));
    }

    #[test]
    fn entries_go_at_the_tail_or_round_to_the_start() {
        let e = vec![vec![0u8; 100]; 3];
        assert_eq!(Hid::enqueue_at(1000, 0, 0, &e), (vec![(0, 0), (100, 1), (200, 2)], 300));
        // Past the end: to the start, the head being further on.
        assert_eq!(Hid::enqueue_at(1000, 500, 850, &e), (vec![(850, 0), (0, 1), (100, 2)], 200));
        // Full: what does not fit is left out.
        assert_eq!(Hid::enqueue_at(1000, 150, 0, &e), (vec![(0, 0)], 100));
    }

    #[test]
    fn a_line_of_input_says_its_events() {
        assert_eq!(Line::parse("move 10 20"), Ok(Line::Events(vec![Input::Move { x: 10, y: 20 }])));
        assert_eq!(
            Line::parse("click 5 6"),
            Ok(Line::Events(vec![Input::Move { x: 5, y: 6 }, Input::Button { button: 0, down: true }, Input::Button { button: 0, down: false }]))
        );
        assert_eq!(Line::parse("up 1"), Ok(Line::Events(vec![Input::Button { button: 1, down: false }])));
        assert_eq!(Line::parse("key 18 down"), Ok(Line::Events(vec![Input::Key { code: 18, down: true }])));
        assert_eq!(Line::parse("press 36").map(|l| matches!(l, Line::Events(e) if e.len() == 2)), Ok(true));
        assert_eq!(Line::parse("sleep 250"), Ok(Line::Sleep(250)));
        assert_eq!(Line::parse("  # a note"), Ok(Line::Events(Vec::new())));
        assert_eq!(Line::parse(""), Ok(Line::Events(Vec::new())));
        assert!(Line::parse("key 18").is_err());
        assert!(Line::parse("move 1").is_err());
        assert!(Line::parse("jump 1 2").is_err());
    }

    #[test]
    fn posted_events_go_in_at_the_tail() {
        let mut b = Hid::shmem();
        let mut h = Hid::new(0);
        assert!(h.drain(&mut b, &[Input::Move { x: 100, y: 50 }, Input::Button { button: 0, down: true }], 7));
        let rd = |b: &[u8], at: usize| i32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        // Two elements written (1 and 2), the tail at 3, the cursor moved,
        // the left button down.
        assert_eq!(rd(&b, 8 + 216), 3);
        assert_eq!(rd(&b, 8 + 220), 2);
        assert_eq!(rd(&b, 8 + 224 + 96 + 8), nx::MOUSEMOVED);
        assert_eq!(rd(&b, 8 + 224 + 2 * 96 + 8), nx::LMOUSEDOWN);
        assert_eq!(i16::from_le_bytes([b[8 + 20], b[8 + 21]]), 100);
        assert_eq!(rd(&b, 8 + 8), 1);
    }
}
