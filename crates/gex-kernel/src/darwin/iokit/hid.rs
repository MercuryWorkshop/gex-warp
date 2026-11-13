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
//! port WindowServer gave it (`IOConnectSetNotificationPort`). The queued
//! events go into the shared memory in WindowServer's own task, the next
//! time it calls the HID system or receives the kick ([`Hid::drain`]):
//! a task's memory is written by the kernel only while it runs for that
//! task, here and on the page alike. Nothing posts yet.

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

/// What the hosts have posted and not yet been delivered, and the port to
/// kick when there is some.
static INPUT: Mutex<VecDeque<Input>> = Mutex::new(VecDeque::new());

/// Give the HID system an event. (What takes it into WindowServer is
/// [`Hid::drain`].)
pub fn post(event: Input) {
    INPUT.lock().unwrap_or_else(|e| e.into_inner()).push_back(event);
    super::kick_hid();
}

/// The events posted and not yet taken.
pub fn take() -> Vec<Input> {
    INPUT.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect()
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
}

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
        Hid { kind, events: false, cursor: false, at: (0, 0), bounds: [0; 4] }
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
            _ => Err(super::UNSUPPORTED),
        }
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
            // What `NXEventData` holds for it, as bytes at their offsets.
            let (what, data): (i32, Vec<(usize, Vec<u8>)>) = match *ev {
                Input::Move { x, y } => {
                    self.at = (x, y);
                    shm[EVG + 20..EVG + 22].copy_from_slice(&(x as i16).to_le_bytes());
                    shm[EVG + 22..EVG + 24].copy_from_slice(&(y as i16).to_le_bytes());
                    wr(shm, EVG + 64, x << 8);
                    wr(shm, EVG + 68, y << 8);
                    (nx::MOUSEMOVED, Vec::new())
                }
                Input::Button { button, down } => {
                    let buttons = rd(shm, EVG + 8);
                    wr(shm, EVG + 8, if down { buttons | 1 << button } else { buttons & !(1 << button) });
                    let what = match (button, down) {
                        (0, true) => nx::LMOUSEDOWN,
                        (0, false) => nx::LMOUSEUP,
                        (1, true) => nx::RMOUSEDOWN,
                        (1, false) => nx::RMOUSEUP,
                        (_, true) => nx::OMOUSEDOWN,
                        (_, false) => nx::OMOUSEUP,
                    };
                    // mouse: click state (one click), button number
                    (what, vec![(4, 1i32.to_le_bytes().to_vec()), (9, vec![button])])
                }
                // key: the key code
                Input::Key { code, down } => (if down { nx::KEYDOWN } else { nx::KEYUP }, vec![(8, code.to_le_bytes().to_vec())]),
                // scrollWheel: the axes, vertical first
                Input::Scroll { dx, dy } => (nx::SCROLLWHEELMOVED, vec![(0, (dy as i16).to_le_bytes().to_vec()), (2, (dx as i16).to_le_bytes().to_vec())]),
            };
            // The tail element is filled, and the tail moves on unless the
            // ring is full (then the oldest is written over in place).
            let tail = rd(shm, EVG + 216) as usize % LLEQ_SIZE;
            let el = EVG + LLEQ + tail * ELEMENT;
            let num = rd(shm, EVG + 4);
            wr(shm, EVG + 4, num.wrapping_add(1));
            // NXEvent, packed to four: type, location, time, flags, window,
            // service id, pid, then the data.
            let e = el + 8;
            shm[e..e + 88].fill(0);
            wr(shm, e, what);
            wr(shm, e + 4, self.at.0);
            wr(shm, e + 8, self.at.1);
            shm[e + 12..e + 20].copy_from_slice(&now_ns.to_le_bytes());
            wr(shm, e + 20, rd(shm, EVG + 12));
            for (at, bytes) in data {
                shm[e + 40 + at..e + 40 + at + bytes.len()].copy_from_slice(&bytes);
            }
            wr(shm, EVG + 220, tail as i32);
            let next = rd(shm, el) as usize % LLEQ_SIZE;
            if next != rd(shm, EVG + 212) as usize % LLEQ_SIZE {
                wr(shm, EVG + 216, next as i32);
            }
        }
        true
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
