//! Mach ports: a task's names for them, the rights behind each name, and
//! the messages queued on them.
//!
//! A port is a queue of messages with one receiver. A task knows a port by
//! a name in its own space, and holds rights under that name: the receive
//! right, send rights (counted), a send-once right. A message carries
//! rights with it -- to the port a reply should go to, and in its body --
//! which leave the sender's space as it is sent and are given names in the
//! receiver's as it is received.
//!
//! What is here is one task's ports: threads of a process send to each
//! other, and to the few ports the kernel itself answers ([`Kind`]): the
//! task's and the host's, a clock, semaphores. And the kernel sends to
//! one kind of its own making, a timer's (`mk_timer_create`): a message at
//! the moment the timer is armed for, which is what a run loop waits on. A message's rights and
//! bytes stay in the kernel between the two calls, so nothing here yet
//! crosses to another process.
//!
//! A thread that waits -- for a message, or on a semaphore -- waits on a
//! word of the task's memory with the kernel's futex ([`Space::words`]),
//! and is told to ask again when the word changes. Mach's own callers ask
//! again: `mach_msg` loops while it is told `MACH_RCV_INTERRUPTED`, and a
//! semaphore's while `KERN_ABORTED`.

use super::mach::port as names;
use super::mach::*;
use super::*;
use crate::syscalls::{clock_ns, nr};
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, Weak};

/// `mach_port_right_t`.
pub mod right {
    pub const SEND: u64 = 0;
    pub const RECEIVE: u64 = 1;
    pub const SEND_ONCE: u64 = 2;
    pub const PORT_SET: u64 = 3;
    pub const DEAD_NAME: u64 = 4;
}

/// `mach_msg_type_name_t`: how a right is taken from the sender, and (the
/// three `MOVE`s) what the receiver is told it got.
pub mod disp {
    pub const MOVE_RECEIVE: u32 = 16;
    pub const MOVE_SEND: u32 = 17;
    pub const MOVE_SEND_ONCE: u32 = 18;
    pub const COPY_SEND: u32 = 19;
    pub const MAKE_SEND: u32 = 20;
    pub const MAKE_SEND_ONCE: u32 = 21;
}

/// `mach_port_type_t`'s bits.
mod ty {
    pub const SEND: u32 = 0x1_0000;
    pub const RECEIVE: u32 = 0x2_0000;
    pub const SEND_ONCE: u32 = 0x4_0000;
    pub const PORT_SET: u32 = 0x8_0000;
    pub const DEAD_NAME: u32 = 0x10_0000;
}

/// The soonest after it is armed that a timer fires, in nanoseconds.
const SOONEST: u64 = 100_000;

pub const MACH_MSG_SUCCESS: u64 = 0;
pub const MACH_SEND_INVALID_DATA: u64 = 0x1000_0002;
pub const MACH_SEND_MSG_TOO_SMALL: u64 = 0x1000_0008;
pub const MACH_SEND_INVALID_REPLY: u64 = 0x1000_0009;
pub const MACH_SEND_INVALID_RIGHT: u64 = 0x1000_000a;
pub const MACH_SEND_INVALID_TYPE: u64 = 0x1000_000f;
pub const MACH_SEND_INVALID_MEMORY: u64 = 0x1000_000c;
pub const MACH_RCV_INVALID_NAME: u64 = 0x1000_4002;
pub const MACH_RCV_TIMED_OUT: u64 = 0x1000_4003;
pub const MACH_RCV_TOO_LARGE: u64 = 0x1000_4004;
pub const MACH_RCV_INTERRUPTED: u64 = 0x1000_4005;
pub const MACH_RCV_INVALID_DATA: u64 = 0x1000_4008;

/// `mach_msg`'s options.
pub mod opt {
    pub const SEND_MSG: u64 = 0x1;
    pub const RCV_MSG: u64 = 0x2;
    pub const RCV_LARGE: u64 = 0x4;
    /// With `RCV_LARGE`: whose message was too large is said too.
    pub const RCV_LARGE_IDENTITY: u64 = 0x8;
    pub const RCV_TIMEOUT: u64 = 0x100;
    /// `mach_msg2`: the message is a pair of vectors, not a buffer.
    pub const MSG_VECTOR: u64 = 0x1_0000_0000;
}

/// `MACH_MSGH_BITS_COMPLEX`: the body begins with descriptors.
const COMPLEX: u32 = 0x8000_0000;
/// `mach_msg_header_t`, and the trailer a received message ends with
/// (`MACH_MSG_TRAILER_FORMAT_0`, its own length).
const HEADER: u64 = 24;
const TRAILER: u64 = 8;

/// The trailer a receive asks for (`MACH_RCV_TRAILER_ELEMENTS` in its
/// options): how long, of `mach_msg_max_trailer_t`'s parts in order --
/// the type and length, a sequence number, the sender's security token,
/// its audit token, the port's context, and two words of what a MAC
/// policy would say.
fn trailer_size(options: u64) -> u64 {
    match (options >> 24) & 0xf {
        0 => TRAILER,
        1 => 12,
        2 => 20,
        3 => 52,
        4 => 60,
        _ => 68,
    }
}

/// The trailer itself, for a message from `sender` received from a port
/// whose context is `context`.
fn trailer(options: u64, sender: &[u32; 8], context: u64) -> Vec<u8> {
    let size = trailer_size(options);
    let mut t = vec![0u8; 68];
    put32(&mut t, 4, size as u32);
    put32(&mut t, 12, sender[1]);
    put32(&mut t, 16, sender[2]);
    for (i, w) in sender.iter().enumerate() {
        put32(&mut t, 20 + 4 * i, *w);
    }
    t[52..60].copy_from_slice(&context.to_le_bytes());
    t.truncate(size as usize);
    t
}
/// The name a right to a dead port is received under.
const MACH_PORT_DEAD: u32 = u32::MAX;
/// `MIG_BAD_ID`: a server's answer to a request it has no routine for.
const MIG_BAD_ID: i32 = -303;
/// `NDR_record`, as every MIG message of this byte order carries it.
const NDR: [u8; 8] = [0, 0, 0, 0, 1, 0, 0, 0];

/// What a port is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A queue a task receives from.
    User,
    /// A port set: a name to receive from any of its members by.
    Set,
    /// The task's own port, the host's, and a thread's: the kernel answers.
    Task,
    Host,
    Thread(i32),
    /// The system clock, likewise.
    Clock,
    /// IOKit's master device port (`host_get_io_master`): there is no
    /// registry here, and every service asked for is not found
    /// ([`Kernel::darwin_iokit`]).
    IoMaster,
    /// A semaphore, which is waited on and signalled by traps of its own.
    Semaphore,
    /// A voucher: what a message carries of who it is sent for. It is a
    /// name and nothing more here.
    Voucher,
}

/// A message on a port's queue: what its receiver will be given, with the
/// rights it carries still rights.
struct Msg {
    id: u32,
    /// Everything after the header, descriptors first when there are any.
    body: Vec<u8>,
    complex: bool,
    /// Whether it was sent by a send-once right.
    once: bool,
    /// The right to reply by, and whether it is a send-once right.
    reply: Option<(Arc<Port>, bool)>,
    /// Each port descriptor in the body: its offset there, the right, and
    /// the type the receiver is told (`disp::MOVE_*`).
    ports: Vec<(usize, Arc<Port>, u32)>,
    /// Each descriptor of memory out of line: its offset in the body, and
    /// the bytes, which are the receiver's at an address of its own once
    /// it has the message.
    memory: Vec<(usize, Vec<u8>)>,
    /// Each array of ports out of line (`MACH_MSG_OOL_PORTS_DESCRIPTOR`):
    /// its offset in the body, and each right in it (none for a null name)
    /// with the type its receiver is told.
    port_arrays: Vec<(usize, Vec<Option<(Arc<Port>, u32)>>)>,
    /// Who sent it, as its audit token says (the kernel's own: zeroes):
    /// the user and group the trailer's security token gives are its
    /// effective ones, the second and third.
    sender: [u32; 8],
}

#[derive(Default)]
struct State {
    queue: VecDeque<Msg>,
    /// The receive right is gone.
    dead: bool,
    /// A semaphore's count.
    count: i64,
    /// The names whose words say this port has something: its receiver's
    /// name for it (a semaphore's own), and each port set it is in.
    wake: Vec<u32>,
    /// The task that has the receive right, to be woken by whoever sends
    /// ([`Rx`]); none while the right is on its way in a message.
    rx: Option<Rx>,
    /// A timer's port (`mk_timer_create`), and the moment it is armed for
    /// on `mach_absolute_time`'s clock, if it is: a message is queued here
    /// then, by whoever next looks ([`Kernel::darwin_msg`]).
    is_timer: bool,
    timer: Option<u64>,
    /// How many send rights have been made from the receive right
    /// (`MACH_MSG_TYPE_MAKE_SEND`): what a no-senders notification says.
    made: u32,
    /// How many send rights there are, in every task and in every message
    /// on its way (XNU's `ip_srights`): a task's references under one name
    /// are one right.
    srights: u32,
    /// The right to tell by when no send right to the port is left
    /// ([`notice::NO_SENDERS`]), if that was asked.
    no_senders: Option<Arc<Port>>,
    /// The names, in whichever tasks, whose dead-name requests are for
    /// this port ([`notice::DEAD_NAME`]).
    dead_requests: Vec<(Weak<Mutex<Space>>, u32)>,
    /// What its receiver keeps with it (`mach_port_set_context`, or a
    /// guard it was made with), which a trailer that asks says.
    context: u64,
}

/// A port: one queue of messages, whose rights any task may hold. It is
/// the system's, not a task's: a right in a message is the port itself,
/// and lands in the receiving task's space as it is received.
pub struct Port {
    pub kind: Kind,
    state: Mutex<State>,
}

impl Port {
    fn new(kind: Kind) -> Arc<Port> {
        Arc::new(Port { kind, state: Mutex::default() })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether the port's send rights are counted, and their last going
    /// told of: a port of a task's, not one the kernel answers.
    fn counted(&self) -> bool {
        self.kind == Kind::User
    }
}

/// How a task that receives from a port is woken by a sender, whichever
/// task that is: its futex queues, and the page its waiting threads'
/// words are in ([`Space::words`]). A waiting receiver took its place on
/// its word's queue before it looked for a message
/// ([`Kernel::darwin_parked`]), so a wake that does not change the word
/// is not lost.
#[derive(Clone)]
pub struct Rx {
    futex: Arc<crate::futex::FutexTable>,
    words: Arc<std::sync::atomic::AtomicU64>,
}

impl Rx {
    /// Wake whoever in the task waits on any of `names`.
    fn wake(&self, names: &[u32]) {
        let words = self.words.load(std::sync::atomic::Ordering::SeqCst);
        if words == 0 {
            return;
        }
        for &name in names {
            self.futex.wake(gex_abi::canon(words + 4 * ((name as u64 >> 8) & 0xfff)), i32::MAX as u32);
        }
    }

    fn is(&self, futex: &Arc<crate::futex::FutexTable>) -> bool {
        Arc::ptr_eq(&self.futex, futex)
    }
}

/// Queue notification `id` on `to`, with its word if it has one, and wake
/// its receiver. The send-once right it is sent by is used up; a port that
/// has died is told nothing.
fn notify(to: &Arc<Port>, id: u32, word: Option<u32>) {
    let (rx, wake) = {
        let mut st = to.state();
        if st.dead {
            return;
        }
        let mut body = Vec::new();
        if let Some(word) = word {
            body.extend_from_slice(&NDR);
            body.extend_from_slice(&word.to_le_bytes());
        }
        st.queue.push_back(Msg { id, body, complex: false, once: true, reply: None, ports: Vec::new(), memory: Vec::new(), port_arrays: Vec::new(), sender: [0; 8] });
        (st.rx.clone(), st.wake.clone())
    };
    if let Some(rx) = rx {
        rx.wake(&wake);
    }
}

/// One more send right to `port` (made, copied, or a reference given a
/// right of its own as it leaves in a message).
fn send_made(port: &Arc<Port>) {
    if port.counted() {
        port.state().srights += 1;
    }
}

/// A send right to `port` is gone: if it was the last, whoever asked is
/// told the port has no senders.
fn send_gone(port: &Arc<Port>) {
    if !port.counted() {
        return;
    }
    let told = {
        let mut st = port.state();
        st.srights = st.srights.saturating_sub(1);
        if st.srights == 0 && !st.dead {
            st.no_senders.take().map(|to| (to, st.made))
        } else {
            None
        }
    };
    if let Some((to, made)) = told {
        notify(&to, notice::NO_SENDERS, Some(made));
    }
}

/// The right a message was sent by (`got`) is used up by the sending: a
/// send right taken to send by is no longer counted.
fn send_gone_if(port: &Arc<Port>, got: u32) {
    if got == disp::MOVE_SEND {
        send_gone(port);
    }
}

/// A right (of type `got`, as a message carries it) destroyed unused.
fn right_gone(port: &Arc<Port>, got: u32) {
    match got {
        disp::MOVE_SEND => send_gone(port),
        disp::MOVE_SEND_ONCE => notify(port, notice::SEND_ONCE, None),
        disp::MOVE_RECEIVE => kill(port),
        _ => {}
    }
}

/// A message destroyed unreceived: the rights it carries go with it.
fn destroy(msg: Msg) {
    if let Some((port, once)) = msg.reply {
        right_gone(&port, if once { disp::MOVE_SEND_ONCE } else { disp::MOVE_SEND });
    }
    for (_, port, got) in msg.ports {
        right_gone(&port, got);
    }
    for (_, rights) in msg.port_arrays {
        for (port, got) in rights.into_iter().flatten() {
            right_gone(&port, got);
        }
    }
}

/// The port's receive right is gone: it is dead, what was queued on it is
/// destroyed, its receiver's waiting threads woken, and each name with a
/// dead-name request for it is told (by [`flush`]).
fn kill(port: &Arc<Port>) {
    let (queue, unused, requests, rx, wake) = {
        let mut st = port.state();
        if st.dead {
            return;
        }
        st.dead = true;
        (std::mem::take(&mut st.queue), st.no_senders.take(), std::mem::take(&mut st.dead_requests), st.rx.take(), std::mem::take(&mut st.wake))
    };
    if let Some(rx) = rx {
        rx.wake(&wake);
    }
    for msg in queue {
        destroy(msg);
    }
    // What its receiver asked to be told of its senders never will be:
    // a send-once right not used.
    if let Some(to) = unused {
        notify(&to, notice::SEND_ONCE, None);
    }
    let mut later = DEAD.lock().unwrap_or_else(|e| e.into_inner());
    for (space, name) in requests {
        later.push((space, name, Arc::downgrade(port)));
    }
}

/// The dead-name requests of ports that have died, to be told by [`flush`]
/// with no space locked: the space, the name, the port.
static DEAD: Mutex<Vec<(Weak<Mutex<Space>>, u32, Weak<Port>)>> = Mutex::new(Vec::new());

/// Tell each name whose port has died and that asked to be told: it is a
/// dead name, with a reference more for whoever is told. Called with no
/// space locked (the end of each trap).
pub fn flush() {
    loop {
        let todo = std::mem::take(&mut *DEAD.lock().unwrap_or_else(|e| e.into_inner()));
        if todo.is_empty() {
            return;
        }
        for (space, name, port) in todo {
            let (Some(space), Some(port)) = (space.upgrade(), port.upgrade()) else { continue };
            let mut sp = space.lock().unwrap_or_else(|e| e.into_inner());
            let Some(e) = sp.names.get_mut(&name).filter(|e| Arc::ptr_eq(&e.port, &port)) else { continue };
            let Some(to) = e.dead_request.take() else { continue };
            e.dead += e.send + e.once as u32 + 1;
            (e.send, e.once) = (0, false);
            drop(sp);
            notify(&to, notice::DEAD_NAME, Some(name));
        }
    }
}

/// What a task holds under one name.
struct Entry {
    port: Arc<Port>,
    recv: bool,
    /// User references to a send right.
    send: u32,
    once: bool,
    /// User references to a dead name: rights to a port that has gone.
    dead: u32,
    /// A port set's members, by name.
    members: Vec<u32>,
    /// The right to tell by when the port this name is a right to dies
    /// ([`notice::DEAD_NAME`]), or the name itself goes
    /// ([`notice::PORT_DELETED`]), if that was asked.
    dead_request: Option<Arc<Port>>,
}

impl Entry {
    fn of(port: Arc<Port>) -> Entry {
        Entry { port, recv: false, send: 0, once: false, dead: 0, members: Vec::new(), dead_request: None }
    }

    fn is_set(&self) -> bool {
        self.port.kind == Kind::Set
    }

    fn empty(&self) -> bool {
        !self.recv && self.send == 0 && !self.once && self.dead == 0 && !self.is_set()
    }
}

/// A task's port name space.
pub struct Space {
    names: HashMap<u32, Entry>,
    next: u32,
    /// A page of the task's memory the kernel keeps, a word for each name
    /// (by the name's index): what a thread waiting on that name sleeps on.
    pub words: u64,
    /// The wait queues told when a name has something, by name: those of
    /// the kqueues with a note on it ([`super::kqueue`]).
    watchers: HashMap<u32, Vec<u32>>,
    /// The ports a notification was queued on since they were last woken
    /// ([`Kernel::darwin_told`]).
    told: Vec<Arc<Port>>,
    /// How a sender wakes this task's threads ([`Rx`]), and the space
    /// itself as a dead-name request names it: given by the kernel at the
    /// space's first use ([`Kernel::darwin_space`]).
    rx: Option<Rx>,
    words_at: Arc<std::sync::atomic::AtomicU64>,
    me: Weak<Mutex<Space>>,
    /// The task's special ports (`task_set_special_port`): a send right to
    /// each, by which; the bootstrap port among them. Passed on to a fork's
    /// child and to the program a task runs next.
    special: HashMap<u32, Arc<Port>>,
}

/// `task_get_special_port`'s and `task_set_special_port`'s numbers.
pub mod special {
    pub const KERNEL: u32 = 1;
    pub const HOST: u32 = 2;
    pub const BOOTSTRAP: u32 = 4;
}

/// The kernel's notifications (`mach/notify.h`): a message to a port its
/// receiver named, by a send-once right made for it
/// (`mach_port_request_notification`), each an NDR record and one word.
mod notice {
    /// A name with a dead-name request has gone: the name.
    pub const PORT_DELETED: u32 = 65;
    /// A port can be sent to again. Asked and never told: no queue here is
    /// ever full. (The asking arms a dead-name notification too.)
    pub const SEND_POSSIBLE: u32 = 66;
    /// No send right to a port is left: how many were ever made.
    pub const NO_SENDERS: u32 = 70;
    /// A send-once right was destroyed and not used. No record, no word.
    pub const SEND_ONCE: u32 = 71;
    /// The port a name was a right to has died: the name, now a dead
    /// name, with a reference more for whoever is told.
    pub const DEAD_NAME: u32 = 72;
}

/// What became of an attempt to receive ([`Kernel::darwin_port_receive`]).
pub(super) enum Received {
    /// A message, written: its size with its trailer.
    Got(u64),
    /// One with no room for it: its size without, and whose it is.
    TooLarge { size: u64, name: u32 },
    /// None; and when the nearest timer among the ports fires, if one is
    /// armed.
    Nothing(Option<u64>),
    /// The name is of nothing to receive from.
    NoName,
    /// The message could not be written where it was to go.
    BadBuffer,
}

impl Default for Space {
    fn default() -> Space {
        let mut names = HashMap::new();
        for (name, kind) in [(names::TASK, Kind::Task), (names::HOST, Kind::Host)] {
            names.insert(name, Entry { send: 1, ..Entry::of(Port::new(kind)) });
        }
        Space {
            names,
            next: names::FIRST_ALLOCATED,
            words: 0,
            watchers: HashMap::new(),
            told: Vec::new(),
            rx: None,
            words_at: Default::default(),
            me: Weak::new(),
            special: HashMap::new(),
        }
    }
}

/// A task that ends gives up every right it holds: its ports die, its
/// send rights are no longer counted, its send-once rights are told of as
/// never used.
impl Drop for Space {
    fn drop(&mut self) {
        for (_, e) in self.names.drain() {
            if e.recv {
                kill(&e.port);
            }
            if e.send > 0 {
                send_gone(&e.port);
            }
            if e.once {
                notify(&e.port, notice::SEND_ONCE, None);
            }
            if let Some(to) = e.dead_request {
                notify(&to, notice::SEND_ONCE, None);
            }
        }
        for (_, port) in self.special.drain() {
            send_gone(&port);
        }
    }
}

impl Space {
    /// The space a fork's child, or the next program, begins with: the
    /// task's own ports, and a send right to each of this one's special
    /// ports.
    pub fn inherit(&self) -> Space {
        let mut s = Space::default();
        for (which, port) in &self.special {
            send_made(port);
            s.special.insert(*which, Arc::clone(port));
        }
        s
    }

    /// The space is this task's, woken through `rx` and named by `me`.
    fn bind(&mut self, futex: &Arc<crate::futex::FutexTable>, me: Weak<Mutex<Space>>) {
        self.rx = Some(Rx { futex: Arc::clone(futex), words: Arc::clone(&self.words_at) });
        self.me = me;
    }

    /// A name nothing has: XNU's are an index and a generation, and the
    /// index is what tells two apart.
    fn fresh(&mut self) -> u32 {
        loop {
            let name = self.next;
            self.next = self.next.wrapping_add(0x100);
            if !self.names.contains_key(&name) && name != 0 && name != MACH_PORT_DEAD {
                return name;
            }
        }
    }

    /// The entry under `name`. A thread's port is there for the asking. A
    /// right to a port that has died since is a dead name from now on.
    fn entry(&mut self, pid: i32, name: u32) -> Option<&mut Entry> {
        if !self.names.contains_key(&name) {
            let tid = port_thread(pid, name)?;
            self.names.insert(name, Entry { send: 1, ..Entry::of(Port::new(Kind::Thread(tid))) });
        }
        let e = self.names.get_mut(&name)?;
        if (e.send > 0 || e.once) && !e.recv && e.port.counted() && e.port.state().dead {
            e.dead += e.send + e.once as u32;
            (e.send, e.once) = (0, false);
        }
        Some(e)
    }

    /// The ports `name` receives from: its own, or a set's members.
    /// `None`: it is a name for neither.
    fn receives(&self, name: u32) -> Option<Vec<(u32, Arc<Port>)>> {
        match self.names.get(&name) {
            Some(e) if e.is_set() => Some(e.members.iter().filter_map(|n| self.names.get(n).map(|e| (*n, Arc::clone(&e.port)))).collect()),
            Some(e) if e.recv => Some(vec![(name, Arc::clone(&e.port))]),
            _ => None,
        }
    }

    /// Whether `name` is one a kqueue may watch (a receive right, or a
    /// set), and if not whether it is a name at all.
    pub(super) fn watchable(&mut self, pid: i32, name: u32) -> Result<(), Errno> {
        match self.entry(pid, name) {
            Some(e) if e.recv || e.is_set() => Ok(()),
            Some(_) => Err(Errno::EOPNOTSUPP),
            None => Err(Errno::ENOENT),
        }
    }

    /// `queue` is told when `name` has something, from now on; or no more.
    pub(super) fn watch(&mut self, name: u32, queue: u32) {
        self.watchers.entry(name).or_default().push(queue);
    }

    pub(super) fn unwatch(&mut self, name: u32, queue: u32) {
        if let Some(v) = self.watchers.get_mut(&name) {
            if let Some(at) = v.iter().position(|q| *q == queue) {
                v.remove(at);
            }
            if v.is_empty() {
                self.watchers.remove(&name);
            }
        }
    }

    /// Whose message a receive from `name` would be given at `now`, if
    /// there is one (a timer whose moment has come has one); and when the
    /// nearest timer still to come fires.
    pub(super) fn pending(&self, name: u32, now: u64) -> (Option<u32>, Option<u64>) {
        let mut nearest: Option<u64> = None;
        for (member, port) in self.receives(name).unwrap_or_default() {
            let st = port.state();
            if !st.queue.is_empty() || st.timer.is_some_and(|when| when <= now) {
                return (Some(member), None);
            }
            if let Some(when) = st.timer {
                nearest = Some(nearest.map_or(when, |n| n.min(when)));
            }
        }
        (None, nearest)
    }

    fn drop_if_empty(&mut self, name: u32) {
        if self.names.get(&name).is_some_and(Entry::empty) {
            if let Some(to) = self.names.remove(&name).and_then(|e| e.dead_request) {
                self.tell(&to, notice::PORT_DELETED, Some(name));
            }
        }
    }

    /// Queue notification `id` on `to`, with its word if it has one, its
    /// receiver to be woken when the call ends ([`Kernel::darwin_told`]).
    fn tell(&mut self, to: &Arc<Port>, id: u32, word: Option<u32>) {
        let mut st = to.state();
        if st.dead {
            return;
        }
        let mut body = Vec::new();
        if let Some(word) = word {
            body.extend_from_slice(&NDR);
            body.extend_from_slice(&word.to_le_bytes());
        }
        st.queue.push_back(Msg { id, body, complex: false, once: true, reply: None, ports: Vec::new(), memory: Vec::new(), port_arrays: Vec::new(), sender: [0; 8] });
        drop(st);
        self.told.push(Arc::clone(to));
    }

    /// A send-once right to `port` has gone without being used, which its
    /// receiver is told.
    fn once_gone(&mut self, port: &Arc<Port>) {
        self.tell(port, notice::SEND_ONCE, None);
    }

    /// The send right under `name`'s references drop to `now`: the right
    /// itself goes with the last.
    fn send_refs(&mut self, name: u32, now: u32) {
        let Some(e) = self.names.get_mut(&name) else { return };
        let was = e.send;
        if !e.port.counted() && e.port.kind != Kind::Semaphore {
            return;
        }
        e.send = now;
        let port = Arc::clone(&e.port);
        if was > 0 && now == 0 {
            send_gone(&port);
        } else if was == 0 && now > 0 {
            send_made(&port);
        }
    }

    /// `mach_port_request_notification`: `to` (a send-once right, or none
    /// to ask no longer) is what notification `which` about `name` is sent
    /// by from now on. What was asked before, or XNU's refusal: no such
    /// name, a name without the right the notification is about, a
    /// notification there is none of.
    fn request(&mut self, pid: i32, name: u32, which: u32, sync: u32, to: Option<Arc<Port>>) -> Result<Option<Arc<Port>>, u64> {
        let me = self.me.clone();
        let Some(e) = self.entry(pid, name) else { return Err(KERN_INVALID_NAME) };
        match which {
            notice::NO_SENDERS => {
                if !e.recv {
                    return Err(KERN_INVALID_RIGHT);
                }
                let port = Arc::clone(&e.port);
                let before = port.state().no_senders.take();
                if let Some(to) = to {
                    // None now, and no fewer made than the caller knows
                    // of: told at once.
                    let (made, none) = {
                        let st = port.state();
                        (st.made, st.srights == 0)
                    };
                    if sync <= made && none {
                        self.tell(&to, notice::NO_SENDERS, Some(made));
                    } else {
                        port.state().no_senders = Some(to);
                    }
                }
                Ok(before)
            }
            notice::DEAD_NAME | notice::SEND_POSSIBLE => {
                if e.is_set() {
                    return Err(KERN_INVALID_RIGHT);
                }
                // (A name dead already has nothing left to die.)
                if !e.recv && e.send == 0 && !e.once {
                    return Err(KERN_INVALID_ARGUMENT);
                }
                let port = Arc::clone(&e.port);
                let before = std::mem::replace(&mut e.dead_request, to);
                if e.dead_request.is_some() {
                    let mut st = port.state();
                    if !st.dead_requests.iter().any(|(s, n)| *n == name && Weak::ptr_eq(s, &me)) {
                        st.dead_requests.push((me, name));
                    }
                }
                Ok(before)
            }
            _ => Err(KERN_INVALID_VALUE),
        }
    }

    /// Take a right from under `name`, as a message being sent does:
    /// the port, and the type its receiver will be told. The right is
    /// counted as one on its way.
    fn take(&mut self, pid: i32, name: u32, how: u32) -> Option<(Arc<Port>, u32)> {
        let e = self.entry(pid, name)?;
        let port = Arc::clone(&e.port);
        let got = match how {
            disp::COPY_SEND if e.send > 0 => {
                send_made(&port);
                disp::MOVE_SEND
            }
            disp::MOVE_SEND if e.send > 0 => {
                // The kernel's own ports are not used up. One reference of
                // several leaves a right of its own; the last takes the
                // name's.
                if port.kind == Kind::User || port.kind == Kind::Semaphore {
                    e.send -= 1;
                    if e.send > 0 {
                        send_made(&port);
                    }
                }
                disp::MOVE_SEND
            }
            disp::MAKE_SEND if e.recv => {
                port.state().made += 1;
                send_made(&port);
                disp::MOVE_SEND
            }
            disp::MAKE_SEND_ONCE if e.recv => disp::MOVE_SEND_ONCE,
            disp::MOVE_SEND_ONCE if e.once => {
                e.once = false;
                disp::MOVE_SEND_ONCE
            }
            disp::MOVE_RECEIVE if e.recv => {
                e.recv = false;
                let mut st = port.state();
                st.wake.clear();
                st.rx = None;
                drop(st);
                for set in self.names.values_mut().filter(|e| e.is_set()) {
                    set.members.retain(|m| *m != name);
                }
                disp::MOVE_RECEIVE
            }
            _ => return None,
        };
        self.drop_if_empty(name);
        Some((port, got))
    }

    /// Give this task a right, as a message being received does: the name
    /// it has there. A send right joins the others to its port; a send-once
    /// right gets a name of its own.
    fn give(&mut self, port: Arc<Port>, got: u32) -> u32 {
        if port.state().dead && got != disp::MOVE_RECEIVE {
            return MACH_PORT_DEAD;
        }
        let held = self.names.iter().find(|(_, e)| Arc::ptr_eq(&e.port, &port) && (e.recv || e.send > 0)).map(|(n, _)| *n);
        match (got, held) {
            (disp::MOVE_SEND_ONCE, _) => {
                let name = self.fresh();
                self.names.insert(name, Entry { once: true, ..Entry::of(port) });
                name
            }
            (_, Some(name)) => {
                let e = self.names.get_mut(&name).unwrap();
                if got == disp::MOVE_RECEIVE {
                    e.recv = true;
                    let mut st = port.state();
                    st.wake = vec![name];
                    st.rx = self.rx.clone();
                } else {
                    // (One right under one name: the one that came is
                    // another reference to the one there.)
                    if e.send > 0 && port.counted() {
                        let mut st = port.state();
                        st.srights = st.srights.saturating_sub(1);
                    }
                    e.send += 1;
                }
                name
            }
            (_, None) => {
                let name = self.fresh();
                let mut e = Entry::of(Arc::clone(&port));
                if got == disp::MOVE_RECEIVE {
                    e.recv = true;
                    let mut st = port.state();
                    st.wake = vec![name];
                    st.rx = self.rx.clone();
                } else {
                    e.send = 1;
                }
                self.names.insert(name, e);
                name
            }
        }
    }

    /// A new port this task receives from, under a new name.
    fn allocate(&mut self, kind: Kind) -> u32 {
        let name = self.fresh();
        let port = Port::new(kind);
        {
            let mut st = port.state();
            st.wake = vec![name];
            st.rx = self.rx.clone();
        }
        let mut e = Entry::of(port);
        match kind {
            Kind::Set => {}
            Kind::User => e.recv = true,
            _ => e.send = 1,
        }
        self.names.insert(name, e);
        name
    }

    /// End the receive right under `name`: the port is dead, what was
    /// queued on it is gone, and every other right to it here is a dead
    /// name (and elsewhere, as each task next looks). The names to wake.
    fn destroy_receive(&mut self, name: u32) -> Vec<u32> {
        let Some(e) = self.names.get_mut(&name) else { return Vec::new() };
        e.recv = false;
        let port = Arc::clone(&e.port);
        let wake = port.state().wake.clone();
        kill(&port);
        for (_, e) in self.names.iter_mut().filter(|(_, e)| Arc::ptr_eq(&e.port, &port)) {
            e.dead += e.send + e.once as u32;
            e.send = 0;
            e.once = false;
        }
        for set in self.names.values_mut().filter(|e| e.is_set()) {
            set.members.retain(|m| *m != name);
        }
        self.drop_if_empty(name);
        wake
    }

    /// The semaphore under `name`, or why there is none to use: no such
    /// name, a name for something else, or a semaphore since destroyed.
    fn semaphore(&mut self, pid: i32, name: u32) -> Result<Arc<Port>, u64> {
        let e = self.entry(pid, name).ok_or(KERN_INVALID_NAME)?;
        if e.port.kind != Kind::Semaphore || e.send == 0 {
            return Err(KERN_INVALID_CAPABILITY);
        }
        if e.port.state().dead {
            return Err(KERN_TERMINATED);
        }
        Ok(Arc::clone(&e.port))
    }

    /// `mach_port_type`'s answer for `name`.
    fn type_of(&mut self, pid: i32, name: u32) -> Option<u32> {
        let e = self.entry(pid, name)?;
        let mut t = 0;
        if e.is_set() {
            t |= ty::PORT_SET;
        }
        if e.recv {
            t |= ty::RECEIVE;
        }
        if e.send > 0 {
            t |= ty::SEND;
        }
        if e.once {
            t |= ty::SEND_ONCE;
        }
        if e.dead > 0 {
            t |= ty::DEAD_NAME;
        }
        Some(t)
    }
}

/// A `mach_msg` call, whichever trap made it.
pub struct MsgCall {
    pub option: u64,
    /// Where the message to send is, and its header as the caller gave it
    /// (`mach_msg2` passes the header in registers).
    pub send_at: u64,
    pub send_size: u64,
    pub bits: u32,
    pub remote: u32,
    pub local: u32,
    pub id: u32,
    /// Where a received message goes, and the room there.
    pub rcv_at: u64,
    pub rcv_size: u64,
    pub rcv_name: u32,
    /// Milliseconds, with `RCV_TIMEOUT`.
    pub timeout: u64,
}

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// `task_restartable_ranges_synchronize`'s number.
const RESTART_SYNCHRONIZE: u32 = 8001;

fn get32(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes(s.try_into().unwrap()))
}

impl Kernel {
    /// The task's space, told at its first use how this task is woken
    /// and what it is called ([`Rx`]).
    pub(super) fn darwin_space(&self) -> Arc<Mutex<Space>> {
        let space = Arc::clone(&self.darwin_ports);
        {
            let mut sp = space.lock().unwrap_or_else(|e| e.into_inner());
            if !sp.rx.as_ref().is_some_and(|rx| rx.is(&self.futexes)) {
                sp.bind(&self.futexes, Arc::downgrade(&space));
            }
        }
        space
    }

    /// The word a thread waiting on `name` sleeps on, in the page of words
    /// the task's memory holds for its space (made at the first need).
    fn darwin_word(&mut self, m: &mut GuestMem<'_>, sp: &mut Space, name: u32) -> Result<u64, Errno> {
        if sp.words == 0 || self.mem.lock().vma_at(sp.words).is_none() {
            sp.words = self.mem.mmap_anon(m, 0, abi::PAGE_SIZE, 3, 0, false, "[mach ports]")?;
            sp.words_at.store(sp.words, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(sp.words + 4 * ((name as u64 >> 8) & 0xfff))
    }

    /// `port` has something: whoever waits on it is told, in this task
    /// (its word changed, as [`Kernel::darwin_wake`]) or another (on its
    /// futex queue, [`Rx`]; and the threads that wait on descriptors look
    /// again, a kqueue's among them).
    fn darwin_wake_port(&mut self, m: &mut GuestMem<'_>, sp: &mut Space, port: &Arc<Port>) {
        let (rx, wake) = {
            let st = port.state();
            (st.rx.clone(), st.wake.clone())
        };
        match rx {
            Some(rx) if rx.is(&self.futexes) => self.darwin_wake(m, sp, &wake, true),
            Some(rx) => {
                rx.wake(&wake);
                self.fd_activity = true;
            }
            None => {}
        }
    }

    /// Take this thread's place on the queue of the word it would wait on
    /// for `name` before looking for a message, so that a wake from
    /// another task, which does not change the word, is not missed
    /// between the look and the wait ([`Kernel::darwin_parked`]).
    fn darwin_park(&mut self, m: &mut GuestMem<'_>, sp: &mut Space, name: u32) {
        self.darwin_parked = self.darwin_word(m, sp, name).ok().map(|at| (at, self.futexes.park(gex_abi::canon(at))));
    }

    /// Say that `names` have something for whoever waits on them.
    fn darwin_wake(&mut self, m: &mut GuestMem<'_>, sp: &mut Space, names: &[u32], all: bool) {
        for &name in names {
            for queue in sp.watchers.get(&name).into_iter().flatten() {
                crate::wait::notify(*queue);
            }
            if let Ok(at) = self.darwin_word(m, sp, name) {
                let v = m.read_u32(at).unwrap_or(0);
                let _ = m.write_u32(at, v.wrapping_add(1));
                let _ = self.sys_futex(m, [at, crate::futex::op::WAKE as u64, if all { i32::MAX as u64 } else { 1 }, 0, 0, 0]);
            }
        }
    }

    /// The canonical call that waits on `name`'s word for `timeout`
    /// nanoseconds (none: for ever), as it is now.
    fn darwin_wait(&mut self, m: &mut GuestMem<'_>, sp: &mut Space, name: u32, timeout: Option<u64>, a: &mut [u64; 6]) -> Result<u64, Errno> {
        let at = self.darwin_word(m, sp, name)?;
        let seen = m.read_u32(at)?;
        let mut ts = 0;
        if let Some(ns) = timeout {
            ts = self.ctx_addr + SCRATCH_SLOT as u64;
            m.write_u64(ts, ns / 1_000_000_000)?;
            m.write_u64(ts + 8, ns % 1_000_000_000)?;
        }
        *a = [at, crate::futex::op::WAIT as u64, seen as u64, ts, 0, 0];
        Ok(nr::FUTEX)
    }

    /// Wake whoever waits on a port a notification was queued on: after a
    /// call that may have dropped a right.
    fn darwin_told(&mut self, m: &mut GuestMem<'_>, sp: &mut Space) {
        for port in std::mem::take(&mut sp.told) {
            self.darwin_wake_port(m, sp, &port);
        }
    }

    /// The traps that name and count rights. `None`: not one of them.
    pub(super) fn darwin_port_trap(&mut self, m: &mut GuestMem<'_>, n: u64, x: [u64; 8]) -> Option<u64> {
        let r = self.darwin_port_trap_of(m, n, x);
        if r.is_some() {
            let space = self.darwin_space();
            let mut sp = space.lock().unwrap_or_else(|e| e.into_inner());
            self.darwin_told(m, &mut sp);
            drop(sp);
            flush();
        }
        r
    }

    fn darwin_port_trap_of(&mut self, m: &mut GuestMem<'_>, n: u64, x: [u64; 8]) -> Option<u64> {
        use super::nr::trap as t;
        let space = self.darwin_space();
        let mut sp = space.lock().unwrap_or_else(|e| e.into_inner());
        let pid = self.pid;
        let name = x[1] as u32;
        // Every one of them is asked of a task, which must be this one.
        let own = x[0] as u32 == names::TASK;
        let bad_task = |r: u64| if own { r } else { MACH_SEND_INVALID_DEST };
        // A timer: a port of the task's that the kernel sends to. These
        // four are asked of the timer itself, by its name.
        // (No such name; a name for something this task does not receive
        // from; a port that is no timer.)
        let timer = |sp: &Space, name: u32| match sp.names.get(&name) {
            None => Err(KERN_INVALID_NAME),
            Some(e) if !e.recv => Err(KERN_INVALID_RIGHT),
            Some(e) if !e.port.state().is_timer => Err(KERN_INVALID_ARGUMENT),
            Some(e) => Ok(Arc::clone(&e.port)),
        };
        Some(match n {
            // (host, recipes, their size, out voucher): a voucher made to
            // the recipes, which libdispatch asks for before it first sends
            // for anyone. What it is made of is not kept.
            t::HOST_CREATE_MACH_VOUCHER => {
                if x[0] as u32 != names::HOST {
                    MACH_SEND_INVALID_DEST
                } else {
                    let name = sp.give(Port::new(Kind::Voucher), disp::MOVE_SEND);
                    kern(m.write_u32(x[3], name).map(|_| 0))
                }
            }
            t::MK_TIMER_CREATE => {
                let name = sp.allocate(Kind::User);
                sp.names[&name].port.state().is_timer = true;
                name as u64
            }
            // (name, when), or (name, flags, when, leeway): armed, and
            // whoever waits on it told to look at how long for.
            // (A moment already gone is as soon as a timer can fire, which
            // is not at once: its message follows the call, a little
            // after.)
            t::MK_TIMER_ARM | t::MK_TIMER_ARM_LEEWAY => match timer(&sp, x[0] as u32) {
                Ok(port) => {
                    let when = if n == t::MK_TIMER_ARM { x[1] } else { x[2] };
                    let wake = {
                        let mut st = port.state();
                        st.timer = Some(when.max(clock_ns(self.clock, 1) + SOONEST));
                        st.wake.clone()
                    };
                    self.darwin_wake(m, &mut sp, &wake, true);
                    KERN_SUCCESS
                }
                Err(e) => e,
            },
            // (name, when*): disarmed; when it was armed for, or 0.
            t::MK_TIMER_CANCEL => match timer(&sp, x[0] as u32) {
                Ok(port) => {
                    let was = port.state().timer.take().unwrap_or(0);
                    match x[1] {
                        0 => KERN_SUCCESS,
                        at => kern(m.write_u64(at, was).map(|_| 0)),
                    }
                }
                Err(e) => e,
            },
            t::MK_TIMER_DESTROY => match timer(&sp, x[0] as u32) {
                Ok(port) => {
                    port.state().timer = None;
                    let wake = sp.destroy_receive(x[0] as u32);
                    self.darwin_wake(m, &mut sp, &wake, true);
                    KERN_SUCCESS
                }
                Err(e) => e,
            },
            t::MACH_REPLY_PORT | t::THREAD_GET_SPECIAL_REPLY_PORT => sp.allocate(Kind::User) as u64,
            // (task, right, name*)
            t::MACH_PORT_ALLOCATE => bad_task({
                let kind = match x[1] {
                    right::RECEIVE => Some(Kind::User),
                    right::PORT_SET => Some(Kind::Set),
                    right::DEAD_NAME => None,
                    _ => return Some(bad_task(KERN_INVALID_VALUE)),
                };
                let made = match kind {
                    Some(k) => sp.allocate(k),
                    None => {
                        let name = sp.fresh();
                        sp.names.insert(name, Entry { dead: 1, ..Entry::of(Port::new(Kind::User)) });
                        name
                    }
                };
                match m.write_u32(x[2], made) {
                    Ok(()) => KERN_SUCCESS,
                    Err(_) => KERN_INVALID_ADDRESS,
                }
            }),
            // (task, options*, context, name*): a receive right, with a
            // send right too when the options ask (MPO_INSERT_SEND_RIGHT).
            t::MACH_PORT_CONSTRUCT => bad_task({
                let flags = m.read_u32(x[1]).unwrap_or(0);
                let made = sp.allocate(Kind::User);
                if flags & 0x10 != 0 {
                    sp.names[&made].port.state().made += 1;
                    sp.send_refs(made, 1);
                }
                // (MPO_CONTEXT_AS_GUARD: the guard is its context.)
                if flags & 0x1 != 0 {
                    sp.names[&made].port.state().context = x[2];
                }
                match m.write_u32(x[3], made) {
                    Ok(()) => KERN_SUCCESS,
                    Err(_) => KERN_INVALID_ADDRESS,
                }
            }),
            // (task, name): one reference to a send, send-once or dead name.
            t::MACH_PORT_DEALLOCATE => bad_task(match sp.entry(pid, name) {
                None => KERN_INVALID_NAME,
                Some(e) if e.send > 0 => {
                    let now = e.send - 1;
                    sp.send_refs(name, now);
                    sp.drop_if_empty(name);
                    KERN_SUCCESS
                }
                Some(e) if e.once => {
                    e.once = false;
                    let port = Arc::clone(&e.port);
                    sp.drop_if_empty(name);
                    sp.once_gone(&port);
                    KERN_SUCCESS
                }
                Some(e) if e.dead > 0 => {
                    e.dead -= 1;
                    sp.drop_if_empty(name);
                    KERN_SUCCESS
                }
                Some(_) => KERN_INVALID_RIGHT,
            }),
            // (task, name, right, delta)
            t::MACH_PORT_MOD_REFS => bad_task({
                let delta = x[3] as i32;
                let r = match sp.entry(pid, name) {
                    None => KERN_INVALID_NAME,
                    Some(e) => match x[2] {
                        right::SEND if e.send > 0 || (delta >= 0 && e.recv) => match e.send.checked_add_signed(delta) {
                            Some(v) => {
                                sp.send_refs(name, v);
                                KERN_SUCCESS
                            }
                            None => KERN_INVALID_VALUE,
                        },
                        right::DEAD_NAME if e.dead > 0 => match e.dead.checked_add_signed(delta) {
                            Some(v) => {
                                e.dead = v;
                                KERN_SUCCESS
                            }
                            None => KERN_INVALID_VALUE,
                        },
                        right::SEND_ONCE if e.once => match delta {
                            0 => KERN_SUCCESS,
                            -1 => {
                                e.once = false;
                                let port = Arc::clone(&e.port);
                                sp.once_gone(&port);
                                KERN_SUCCESS
                            }
                            _ => KERN_INVALID_VALUE,
                        },
                        right::RECEIVE if e.recv => match delta {
                            0 => KERN_SUCCESS,
                            -1 => {
                                let wake = sp.destroy_receive(name);
                                self.darwin_wake(m, &mut sp, &wake, true);
                                KERN_SUCCESS
                            }
                            _ => KERN_INVALID_VALUE,
                        },
                        right::PORT_SET if e.is_set() => match delta {
                            0 => KERN_SUCCESS,
                            -1 => {
                                let members = sp.names.remove(&name).map(|e| e.members).unwrap_or_default();
                                for member in members {
                                    if let Some(e) = sp.names.get(&member) {
                                        e.port.state().wake.retain(|w| *w != name);
                                    }
                                }
                                self.darwin_wake(m, &mut sp, &[name], true);
                                KERN_SUCCESS
                            }
                            _ => KERN_INVALID_VALUE,
                        },
                        right::SEND | right::RECEIVE | right::SEND_ONCE | right::PORT_SET | right::DEAD_NAME => KERN_INVALID_RIGHT,
                        _ => KERN_INVALID_VALUE,
                    },
                };
                sp.drop_if_empty(name);
                r
            }),
            // (task, name, send-right delta, guard): the receive right goes.
            t::MACH_PORT_DESTRUCT => bad_task(match sp.entry(pid, name) {
                None => KERN_INVALID_NAME,
                Some(e) if !e.recv => KERN_INVALID_RIGHT,
                Some(e) => {
                    let now = e.send.saturating_add_signed(x[2] as i32);
                    sp.send_refs(name, now);
                    let wake = sp.destroy_receive(name);
                    self.darwin_wake(m, &mut sp, &wake, true);
                    KERN_SUCCESS
                }
            }),
            // (task, name, right's name, how): a right, put under `name`.
            // A task makes a send right from its own receive right, or
            // counts another reference to one it holds.
            t::MACH_PORT_INSERT_RIGHT => bad_task({
                let (from, how) = (x[2] as u32, x[3] as u32);
                match sp.take(pid, from, how) {
                    None => {
                        if sp.entry(pid, from).is_none() {
                            KERN_INVALID_NAME
                        } else {
                            KERN_INVALID_RIGHT
                        }
                    }
                    Some((port, got)) => {
                        let held = sp.names.iter().find(|(_, e)| Arc::ptr_eq(&e.port, &port)).map(|(n, _)| *n);
                        match held {
                            // A port has one name in a space.
                            Some(h) if h != name && got != disp::MOVE_SEND_ONCE => KERN_RIGHT_EXISTS,
                            None if sp.names.contains_key(&name) => KERN_NAME_EXISTS,
                            _ if got == disp::MOVE_SEND_ONCE => KERN_INVALID_CAPABILITY,
                            _ => {
                                let rx = sp.rx.clone();
                                let e = sp.names.entry(name).or_insert_with(|| Entry::of(Arc::clone(&port)));
                                if got == disp::MOVE_RECEIVE {
                                    e.recv = true;
                                    let mut st = port.state();
                                    st.wake = vec![name];
                                    st.rx = rx;
                                } else {
                                    // (Another reference to a right there
                                    // already is no right of its own.)
                                    if e.send > 0 && port.counted() {
                                        let mut st = port.state();
                                        st.srights = st.srights.saturating_sub(1);
                                    }
                                    e.send += 1;
                                }
                                KERN_SUCCESS
                            }
                        }
                    }
                }
            }),
            // (task, name, set): a receive right into a port set beside the
            // sets it is in already (insert), into that one and out of every
            // other (move; set 0: out of them all), or out of that one
            // (extract). A port may be in several: a run loop's port to wake
            // it by is in the set of each of its modes.
            t::MACH_PORT_INSERT_MEMBER | t::MACH_PORT_MOVE_MEMBER | t::MACH_PORT_EXTRACT_MEMBER => bad_task({
                const KERN_ALREADY_IN_SET: u64 = 11;
                const KERN_NOT_IN_SET: u64 = 12;
                let set = x[2] as u32;
                let member = sp.names.get(&name).filter(|e| e.recv).map(|e| Arc::clone(&e.port));
                let is_set = (set == 0 && n == t::MACH_PORT_MOVE_MEMBER) || sp.names.get(&set).is_some_and(Entry::is_set);
                let inside = sp.names.get(&set).is_some_and(|e| e.is_set() && e.members.contains(&name));
                match member {
                    None if !sp.names.contains_key(&name) => KERN_INVALID_NAME,
                    None => KERN_INVALID_RIGHT,
                    Some(_) if !is_set => KERN_INVALID_RIGHT,
                    Some(_) if n == t::MACH_PORT_INSERT_MEMBER && inside => KERN_ALREADY_IN_SET,
                    Some(_) if n == t::MACH_PORT_EXTRACT_MEMBER && !inside => KERN_NOT_IN_SET,
                    Some(port) => {
                        // Out of what it leaves, and into what it joins.
                        let (leaves_all, joins) = (n == t::MACH_PORT_MOVE_MEMBER, n != t::MACH_PORT_EXTRACT_MEMBER && set != 0);
                        for (at, e) in sp.names.iter_mut().filter(|(_, e)| e.is_set()) {
                            if leaves_all || (n == t::MACH_PORT_EXTRACT_MEMBER && *at == set) {
                                e.members.retain(|m| *m != name);
                            }
                        }
                        let ready = {
                            let mut st = port.state();
                            // (Its first to wake is itself; the rest are its sets.)
                            if leaves_all {
                                st.wake.truncate(1);
                            } else if n == t::MACH_PORT_EXTRACT_MEMBER {
                                let own = st.wake.first().copied();
                                st.wake.retain(|w| Some(*w) == own || *w != set);
                            }
                            if joins {
                                st.wake.push(set);
                            }
                            !st.queue.is_empty()
                        };
                        if joins {
                            sp.names.get_mut(&set).unwrap().members.push(name);
                            if ready {
                                self.darwin_wake(m, &mut sp, &[set], true);
                            }
                        }
                        KERN_SUCCESS
                    }
                }
            }),
            // (task, name, type*)
            t::MACH_PORT_TYPE => bad_task(match sp.type_of(pid, name) {
                None => KERN_INVALID_NAME,
                Some(t) => match m.write_u32(x[2], t) {
                    Ok(()) => KERN_SUCCESS,
                    Err(_) => KERN_INVALID_ADDRESS,
                },
            }),
            // Guards are not kept; a notification asked for is not sent
            // (the previous registration, of which there was none, at x[6]).
            t::MACH_PORT_GUARD | t::MACH_PORT_UNGUARD => bad_task(if sp.entry(pid, name).is_some() { KERN_SUCCESS } else { KERN_INVALID_NAME }),
            t::MACH_PORT_REQUEST_NOTIFICATION => bad_task(match sp.entry(pid, name) {
                None => KERN_INVALID_NAME,
                Some(_) => {
                    let _ = m.write_u32(x[6], 0);
                    KERN_SUCCESS
                }
            }),
            t::SEMAPHORE_SIGNAL | t::SEMAPHORE_SIGNAL_ALL => {
                match sp.semaphore(pid, x[0] as u32) {
                    Err(why) => why,
                    Ok(sem) => {
                        let all = n == t::SEMAPHORE_SIGNAL_ALL;
                        // A signal is kept for a waiter to come; one to all
                        // waiters is for those there now.
                        if !all {
                            sem.state().count += 1;
                        }
                        self.darwin_wake(m, &mut sp, &[x[0] as u32], all);
                        KERN_SUCCESS
                    }
                }
            }
            _ => return None,
        })
    }

    /// A trap that may wait: `mach_msg` receiving, and a semaphore's waits.
    /// Its answer is ready (`Err`), or it is a canonical call that waits,
    /// its arguments in `a` and [`Post::Wake`] left to turn the wait's end
    /// into the trap's answer.
    /// Whether the calling thread's request that its task's threads be
    /// restarted ([`super::restart`]) is still to be answered by one of
    /// them, and how long to wait before asking again: made, the first
    /// time this is asked; looked at again very soon at first (a thread at
    /// work answers within a thousand turns of whatever loop it is in),
    /// then less often; and given up on after about a second, when a
    /// thread that has not answered is taken to be one this kernel has
    /// lost sight of and not one in a range.
    fn darwin_restart_waits(&mut self) -> Option<u64> {
        const LOOKS: u64 = 5000;
        let ranges = Arc::clone(&self.darwin_restart);
        if !ranges.on() {
            return None;
        }
        let me = Arc::clone(self.signals.thread());
        let (asked, looks) = me.restart_waits(|| ranges.ask());
        // (Of the threads there are: a thread that has ended may have
        // left its signal state behind.)
        let (procs, pid) = (Arc::clone(&self.procs), self.pid);
        if looks < LOOKS && ranges.unanswered(&self.thread_signals, &me, asked, |tid| procs.task_owner(tid) == Some(pid)) {
            return Some(match looks {
                0..16 => 1_000,
                16..64 => 20_000,
                _ => 200_000,
            });
        }
        me.restart_waited();
        None
    }

    pub(super) fn darwin_wait_trap(&mut self, m: &mut GuestMem<'_>, n: u64, x: [u64; 8], a: &mut [u64; 6]) -> Option<Result<u64, u64>> {
        use super::nr::trap as t;
        let restart = if n == t::MACH_MSG2 && (x[4] >> 32) as u32 == RESTART_SYNCHRONIZE && x[1] & opt::SEND_MSG != 0 { self.darwin_restart_waits() } else { None };
        Some(match n {
            t::MACH_MSG | t::MACH_MSG_OVERWRITE => {
                let Ok(h) = m.read(x[0], HEADER as usize).map(|b| b.to_vec()) else { return Some(Err(MACH_SEND_INVALID_DATA)) };
                let rcv_at = if n == t::MACH_MSG_OVERWRITE && x[7] != 0 { x[7] } else { x[0] };
                self.darwin_msg(m, MsgCall {
                    option: x[1] & 0xffff_ffff,
                    send_at: x[0],
                    send_size: x[2] & 0xffff_ffff,
                    bits: get32(&h, 0),
                    remote: get32(&h, 8),
                    local: get32(&h, 12),
                    id: get32(&h, 20),
                    rcv_at,
                    rcv_size: x[3] & 0xffff_ffff,
                    rcv_name: x[4] as u32,
                    timeout: x[5] & 0xffff_ffff,
                }, a)
            }
            // (data, options, bits and send size, remote and local,
            // voucher and id, descriptor count and receive name, receive
            // size and priority, timeout). With MSG_VECTOR the data is two
            // vectors, the message's and its auxiliary data's, and the two
            // sizes count them.
            // The task asks that its threads be restarted
            // (`task_restartable_ranges_synchronize`, [`super::restart`]):
            // while one of them has yet to answer, a little later and
            // again. Only once they all have is it answered.
            t::MACH_MSG2 if restart.is_some() => {
                let ts = self.ctx_addr + SCRATCH_SLOT as u64;
                let _ = m.write_u64(ts, 0);
                let _ = m.write_u64(ts + 8, restart.unwrap_or(0));
                self.darwin_post = Post::Again;
                *a = [ts, 0, 0, 0, 0, 0];
                Ok(nr::NANOSLEEP)
            }
            t::MACH_MSG2 => {
                let (mut send_at, mut rcv_at, mut send_size, mut rcv_size) = (x[0], x[0], x[2] >> 32, x[6] & 0xffff_ffff);
                if x[1] & opt::MSG_VECTOR != 0 {
                    let Ok(v) = m.read(x[0], 24).map(|b| b.to_vec()) else { return Some(Err(MACH_SEND_INVALID_DATA)) };
                    send_at = u64::from_le_bytes(v[0..8].try_into().unwrap());
                    rcv_at = match u64::from_le_bytes(v[8..16].try_into().unwrap()) {
                        0 => send_at,
                        at => at,
                    };
                    send_size = get32(&v, 16) as u64;
                    rcv_size = get32(&v, 20) as u64;
                }
                self.darwin_msg(m, MsgCall {
                    option: x[1],
                    send_at,
                    send_size,
                    bits: x[2] as u32,
                    remote: x[3] as u32,
                    local: (x[3] >> 32) as u32,
                    id: (x[4] >> 32) as u32,
                    rcv_at,
                    rcv_size,
                    rcv_name: (x[5] >> 32) as u32,
                    timeout: x[7],
                }, a)
            }
            // (semaphore), and with a time (semaphore, seconds, nanoseconds).
            t::SEMAPHORE_WAIT | t::SEMAPHORE_TIMEDWAIT => {
                let timeout = (n == t::SEMAPHORE_TIMEDWAIT).then(|| (x[1] & 0xffff_ffff) * 1_000_000_000 + (x[2] & 0xffff_ffff));
                match self.darwin_sem_wait(m, x[0] as u32, timeout, a) {
                    Ok(Some(call)) => {
                        self.darwin_post = Post::Wake { woken: KERN_ABORTED as u32, timed_out: KERN_OPERATION_TIMED_OUT as u32 };
                        Ok(call)
                    }
                    Ok(None) => Err(KERN_SUCCESS),
                    Err(why) => Err(why),
                }
            }
            _ => return None,
        })
    }

    /// Take one from the semaphore `name`, or the canonical call that
    /// waits for one (`timeout` nanoseconds, when there is one; none at all
    /// is no wait). An error is Mach's word for why not.
    pub(super) fn darwin_sem_wait(&mut self, m: &mut GuestMem<'_>, name: u32, timeout: Option<u64>, a: &mut [u64; 6]) -> Result<Option<u64>, u64> {
        let space = self.darwin_space();
        let mut sp = space.lock().unwrap_or_else(|e| e.into_inner());
        let sem = sp.semaphore(self.pid, name)?;
        {
            let mut st = sem.state();
            if st.count > 0 {
                st.count -= 1;
                return Ok(None);
            }
        }
        self.darwin_wait(m, &mut sp, name, timeout, a).map(Some).map_err(|_| KERN_INVALID_ADDRESS)
    }

    /// `mach_msg`: send, receive, or both in turn.
    fn darwin_msg(&mut self, m: &mut GuestMem<'_>, c: MsgCall, a: &mut [u64; 6]) -> Result<u64, u64> {
        let r = self.darwin_msg_of(m, c, a);
        flush();
        r
    }

    fn darwin_msg_of(&mut self, m: &mut GuestMem<'_>, c: MsgCall, a: &mut [u64; 6]) -> Result<u64, u64> {
        let space = self.darwin_space();
        let mut sp = space.lock().unwrap_or_else(|e| e.into_inner());
        if c.option & opt::SEND_MSG != 0 {
            let r = self.darwin_send(m, &mut sp, &c);
            if r != MACH_MSG_SUCCESS {
                return Err(r);
            }
        }
        if c.option & opt::RCV_MSG == 0 {
            return Err(MACH_MSG_SUCCESS);
        }
        // (Its place on the queue, before it looks.)
        if sp.receives(c.rcv_name).is_some() {
            self.darwin_park(m, &mut sp, c.rcv_name);
        }
        let nearest = match self.darwin_port_receive(m, &mut sp, c.rcv_name, c.rcv_at, c.rcv_size, c.option & opt::RCV_LARGE != 0, c.option) {
            Received::Got(_) => return Err(MACH_MSG_SUCCESS),
            Received::TooLarge { .. } => return Err(MACH_RCV_TOO_LARGE),
            Received::NoName => return Err(MACH_RCV_INVALID_NAME),
            Received::BadBuffer => return Err(MACH_RCV_INVALID_DATA),
            Received::Nothing(nearest) => nearest,
        };
        // Nothing there: say so, or wait to be told to look again. A wait
        // that ends at a timer's moment was not timed out: it looks again,
        // and finds the timer's message.
        let now = clock_ns(self.clock, 1);
        let timeout = (c.option & opt::RCV_TIMEOUT != 0).then_some(c.timeout * 1_000_000);
        if timeout == Some(0) {
            return Err(MACH_RCV_TIMED_OUT);
        }
        let until_timer = nearest.map(|when| when.saturating_sub(now).max(1));
        let timer_first = until_timer.is_some_and(|t| timeout.is_none_or(|u| t < u));
        let timeout = if timer_first { until_timer } else { timeout };
        match self.darwin_wait(m, &mut sp, c.rcv_name, timeout, a) {
            Ok(call) => {
                let timed_out = if timer_first { MACH_RCV_INTERRUPTED } else { MACH_RCV_TIMED_OUT };
                self.darwin_post = Post::Wake { woken: MACH_RCV_INTERRUPTED as u32, timed_out: timed_out as u32 };
                Ok(call)
            }
            Err(_) => Err(MACH_RCV_INVALID_DATA),
        }
    }

    /// The receiving half, without the wait: the first message of what
    /// `rcv_name` receives from, into the `rcv_size` bytes at `rcv_at`,
    /// its rights into the caller's space. One too large for the room
    /// stays queued if `large`, and is gone otherwise.
    pub(super) fn darwin_port_receive(&mut self, m: &mut GuestMem<'_>, sp: &mut Space, rcv_name: u32, rcv_at: u64, rcv_size: u64, large: bool, options: u64) -> Received {
        let tsize = trailer_size(options);
        // What is asked of: a receive right, or a set of them.
        let Some(from) = sp.receives(rcv_name) else { return Received::NoName };
        // A timer whose moment has come has its message now (24 bytes of
        // nothing, as `mk_timer_expire` sends); the nearest still to come
        // is how long a wait may be.
        let now = clock_ns(self.clock, 1);
        let mut nearest: Option<u64> = None;
        for (_, port) in &from {
            let mut st = port.state();
            match st.timer {
                Some(when) if when <= now => {
                    st.timer = None;
                    st.queue.push_back(Msg { id: 0, body: vec![0; 24], complex: false, once: false, reply: None, ports: Vec::new(), memory: Vec::new(), port_arrays: Vec::new(), sender: [0; 8] });
                }
                Some(when) => nearest = Some(nearest.map_or(when, |n| n.min(when))),
                None => {}
            }
        }
        for (name, port) in from {
            let mut st = port.state();
            let Some(msg) = st.queue.front() else { continue };
            let size = HEADER + msg.body.len() as u64;
            if size + tsize > rcv_size {
                // Too large: with RCV_LARGE it stays queued and its size
                // is said; without, it is gone.
                let mut head = [0u8; HEADER as usize];
                put32(&mut head, 4, size as u32);
                put32(&mut head, 12, name);
                put32(&mut head, 20, msg.id);
                if !large {
                    let gone = st.queue.pop_front();
                    drop(st);
                    if let Some(msg) = gone {
                        destroy(msg);
                    }
                } else {
                    drop(st);
                }
                if rcv_size >= HEADER {
                    let _ = m.write(rcv_at, &head);
                }
                return Received::TooLarge { size, name };
            }
            let msg = st.queue.pop_front().unwrap();
            let context = st.context;
            drop(st);
            let mut out = vec![0u8; (size + tsize) as usize];
            out[HEADER as usize..size as usize].copy_from_slice(&msg.body);
            let (reply_name, reply_type) = match msg.reply {
                Some((port, once)) => {
                    let got = if once { disp::MOVE_SEND_ONCE } else { disp::MOVE_SEND };
                    (sp.give(port, got), got)
                }
                None => (0, 0),
            };
            for (at, port, got) in msg.ports {
                let given = sp.give(port, got);
                put32(&mut out, HEADER as usize + at, given);
                out[HEADER as usize + at + 10] = got as u8;
            }
            // An array of ports out of line arrives as names in memory
            // newly the receiver's.
            for (at, rights) in msg.port_arrays {
                let mut names = Vec::with_capacity(rights.len() * 4);
                let mut kind = disp::MOVE_SEND;
                for right in rights {
                    let given = match right {
                        Some((port, got)) => {
                            kind = got;
                            sp.give(port, got)
                        }
                        None => 0,
                    };
                    names.extend_from_slice(&given.to_le_bytes());
                }
                let address = if names.is_empty() {
                    0
                } else {
                    let Ok(address) = self.darwin_vm_map_at(m, 0, names.len() as u64, 0, 1, 3) else { return Received::BadBuffer };
                    if m.write(address, &names).is_err() {
                        return Received::BadBuffer;
                    }
                    address
                };
                out[HEADER as usize + at..HEADER as usize + at + 8].copy_from_slice(&address.to_le_bytes());
                out[HEADER as usize + at + 8] = 1;
                out[HEADER as usize + at + 10] = kind as u8;
            }
            // Memory out of line arrives in memory newly the receiver's.
            for (at, bytes) in msg.memory {
                if bytes.is_empty() {
                    continue;
                }
                let Ok(address) = self.darwin_vm_map_at(m, 0, bytes.len() as u64, 0, 1, 3) else { return Received::BadBuffer };
                if m.write(address, &bytes).is_err() {
                    return Received::BadBuffer;
                }
                out[HEADER as usize + at..HEADER as usize + at + 8].copy_from_slice(&address.to_le_bytes());
            }
            let dest_type = if msg.once { disp::MOVE_SEND_ONCE } else { disp::MOVE_SEND };
            put32(&mut out, 0, reply_type | dest_type << 8 | if msg.complex { COMPLEX } else { 0 });
            put32(&mut out, 4, size as u32);
            put32(&mut out, 8, reply_name);
            put32(&mut out, 12, name);
            put32(&mut out, 20, msg.id);
            // The trailer: format 0, as long as was asked.
            out[size as usize..].copy_from_slice(&trailer(options, &msg.sender, context));
            return match m.write(rcv_at, &out) {
                Ok(()) => Received::Got(size + tsize),
                Err(_) => Received::BadBuffer,
            };
        }
        Received::Nothing(nearest)
    }

    /// The sending half: the message leaves the caller's memory and its
    /// rights the caller's space, for the destination's queue or, when the
    /// destination is the kernel's, for the routine it asks for.
    fn darwin_send(&mut self, m: &mut GuestMem<'_>, sp: &mut Space, c: &MsgCall) -> u64 {
        let r = self.darwin_send_of(m, sp, c);
        self.darwin_told(m, sp);
        r
    }

    fn darwin_send_of(&mut self, m: &mut GuestMem<'_>, sp: &mut Space, c: &MsgCall) -> u64 {
        let pid = self.pid;
        if c.send_size < HEADER || c.send_size % 4 != 0 {
            return MACH_SEND_MSG_TOO_SMALL;
        }
        let Ok(mut body) = m.read(c.send_at + HEADER, (c.send_size - HEADER) as usize).map(|b| b.to_vec()) else {
            return MACH_SEND_INVALID_DATA;
        };
        let complex = c.bits & COMPLEX != 0;
        // The body's descriptors are checked before any right moves: a
        // message refused is one that changed nothing.
        let mut descs = Vec::new();
        let mut memory = Vec::new();
        let mut arrays = Vec::new();
        let mut gone = Vec::new();
        if complex {
            let count = get32(&body, 0) as usize;
            let mut at = 4;
            for _ in 0..count {
                if at + 12 > body.len() {
                    return MACH_SEND_MSG_TOO_SMALL;
                }
                match body[at + 11] {
                    // A port descriptor: the name, then its disposition
                    // and type in the last two bytes.
                    0 => {
                        descs.push((at, get32(&body, at), body[at + 10] as u32));
                        at += 12;
                    }
                    // Memory out of line (and what may be changed under
                    // the message, which here is the same): an address,
                    // whether the sender's own memory goes with it, how
                    // it is carried, the type, and its size. The bytes are
                    // the message's from now on.
                    1 | 3 => {
                        if at + 16 > body.len() {
                            return MACH_SEND_MSG_TOO_SMALL;
                        }
                        let address = u64::from_le_bytes(body[at..at + 8].try_into().unwrap());
                        let size = get32(&body, at + 12) as u64;
                        // (Memory the task has mapped to read, not only
                        // memory that can be read from here.)
                        if !self.mem.lock().check_access(address, size, crate::mem::prot::READ) {
                            return MACH_SEND_INVALID_MEMORY;
                        }
                        let Ok(bytes) = m.read(address, size as usize).map(|b| b.to_vec()) else { return MACH_SEND_INVALID_MEMORY };
                        if body[at + 8] != 0 && size != 0 {
                            gone.push((address, size));
                        }
                        memory.push((at, bytes));
                        at += 16;
                    }
                    // An array of ports out of line: an address, whether
                    // the sender's memory goes with it, how it is carried,
                    // the rights' disposition, the type, and how many.
                    2 => {
                        if at + 16 > body.len() {
                            return MACH_SEND_MSG_TOO_SMALL;
                        }
                        let address = u64::from_le_bytes(body[at..at + 8].try_into().unwrap());
                        let count = get32(&body, at + 12) as u64;
                        let Ok(raw) = m.read(address, (count * 4) as usize).map(|b| b.to_vec()) else { return MACH_SEND_INVALID_MEMORY };
                        if body[at + 8] != 0 && count != 0 {
                            gone.push((address, count * 4));
                        }
                        arrays.push((at, (0..count as usize).map(|i| get32(&raw, 4 * i)).collect::<Vec<u32>>(), body[at + 10] as u32));
                        at += 16;
                    }
                    _ => return MACH_SEND_INVALID_TYPE,
                }
            }
        }
        let Some((dest, dest_got)) = sp.take(pid, c.remote, c.bits & 0x1f) else {
            return MACH_SEND_INVALID_DEST;
        };
        let reply = if c.local == 0 {
            None
        } else {
            match sp.take(pid, c.local, (c.bits >> 8) & 0x1f) {
                Some((port, got)) if got != disp::MOVE_RECEIVE => Some((port, got == disp::MOVE_SEND_ONCE)),
                _ => return MACH_SEND_INVALID_REPLY,
            }
        };
        let mut ports = Vec::new();
        for (at, name, how) in descs {
            if name == 0 {
                continue;
            }
            match sp.take(pid, name, how) {
                Some((port, got)) => {
                    if got == disp::MOVE_RECEIVE {
                        port.state().wake.clear();
                    }
                    ports.push((at, port, got));
                    put32(&mut body, at, 0);
                }
                None => return MACH_SEND_INVALID_RIGHT,
            }
        }
        let mut port_arrays = Vec::new();
        for (at, names, how) in arrays {
            let mut rights = Vec::new();
            for name in names {
                if name == 0 || name == MACH_PORT_DEAD {
                    rights.push(None);
                    continue;
                }
                match sp.take(pid, name, how) {
                    Some((port, got)) => rights.push(Some((port, got))),
                    None => return MACH_SEND_INVALID_RIGHT,
                }
            }
            body[at..at + 8].fill(0);
            port_arrays.push((at, rights));
        }
        // The sender's memory that was to go with the message goes.
        for (address, size) in gone {
            let (lo, hi) = (address & !(abi::PAGE_SIZE - 1), (address + size).next_multiple_of(abi::PAGE_SIZE));
            let _ = self.map_io().call(m, nr::MUNMAP, [lo, hi - lo, 0, 0, 0, 0]);
        }
        // (Wherever it arrives, it is its receiver's to give back.)
        for (at, _) in &memory {
            body[*at..*at + 8].fill(0);
            body[*at + 8] = 1;
        }
        let sender = [self.creds.ruid(), self.creds.uid(), self.creds.gid(), self.creds.ruid(), self.creds.rgid(), self.pid as u32, self.procs.sid(self.pid, self.pid).unwrap_or(0) as u32, 1];
        let msg = Msg { id: c.id, body, complex, once: dest_got == disp::MOVE_SEND_ONCE, reply, ports, memory, port_arrays, sender };
        match dest.kind {
            Kind::User => {
                {
                    let mut st = dest.state();
                    if st.dead {
                        // (Its rights go with it.)
                        drop(st);
                        destroy(msg);
                        send_gone_if(&dest, dest_got);
                        return MACH_SEND_INVALID_DEST;
                    }
                    st.queue.push_back(msg);
                }
                // The right the message was sent by is used up with it.
                send_gone_if(&dest, dest_got);
                self.darwin_wake_port(m, sp, &dest);
            }
            Kind::Set => return MACH_SEND_INVALID_DEST,
            kind => {
                // The kernel's: the routine is run, and its answer queued
                // on the reply port for the receive that follows.
                let Some((reply, once)) = msg.reply.clone() else { return MACH_MSG_SUCCESS };
                let answer = self.darwin_mig(m, sp, kind, &msg);
                reply.state().queue.push_back(Msg { once, reply: None, ..answer });
                self.darwin_wake_port(m, sp, &reply);
            }
        }
        MACH_MSG_SUCCESS
    }

    /// A request to one of the kernel's own ports, answered as its MIG
    /// server answers: the routine by the message's number, the reply
    /// numbered a hundred more. A routine there is none of is `MIG_BAD_ID`.
    /// A request to IOKit's master port: what it asks for is said (with
    /// GEX_IOKIT_TRACE, or the trace), and answered as a system with no
    /// such service would: waiting for the registry to be quiet is done at
    /// once, and anything looked up is not found (kIOReturnNotFound).
    fn darwin_iokit(&mut self, id: u32, body: &[u8]) -> i32 {
        const NOT_FOUND: i32 = 0xe000_02f0_u32 as i32;
        let name = match id {
            2804 | 2871 | 2888 | 2889 | 2895 | 2896 => "matching services",
            2859 | 2873 | 2883 | 2885 | 2898 | 2900 => "a notification of matching services",
            2809 | 2903 => "a registry entry by path",
            2833 => "the registry's root",
            2838 => "the registry quiet",
            _ => "",
        };
        if self.trace || std::env::var_os("GEX_IOKIT_TRACE").is_some() {
            // What a matching dictionary names: its strings (the inline
            // XML of the older calls, or the binary serialization's).
            let mut words = Vec::new();
            let mut run = Vec::new();
            for &b in body.iter().chain(std::iter::once(&0)) {
                if (0x20..0x7f).contains(&b) {
                    run.push(b);
                } else {
                    if run.len() >= 3 {
                        words.push(String::from_utf8_lossy(&run).into_owned());
                    }
                    run.clear();
                }
            }
            eprintln!("[gex:{}] IOKit {} ({}): {}", self.pid, id, if name.is_empty() { "?" } else { name }, words.join(" "));
        }
        match id {
            2838 => 0,
            _ => NOT_FOUND,
        }
    }

    fn darwin_mig(&mut self, m: &mut GuestMem<'_>, sp: &mut Space, kind: Kind, req: &Msg) -> Msg {
        let reply = |body: Vec<u8>, ports: Vec<(usize, Arc<Port>, u32)>| Msg { id: req.id + 100, complex: !ports.is_empty(), body, once: false, reply: None, ports, memory: Vec::new(), port_arrays: Vec::new(), sender: [0; 8] };
        // { NDR, RetCode }, then what the routine returns.
        let plain = |code: i32, rest: &[u8]| {
            let mut b = NDR.to_vec();
            b.extend_from_slice(&code.to_le_bytes());
            b.extend_from_slice(rest);
            reply(b, Vec::new())
        };
        // { descriptor count, one port descriptor }: a right, sent.
        let with_port = |port: Arc<Port>| {
            let mut b = vec![0u8; 16];
            put32(&mut b, 0, 1);
            b[4 + 10] = disp::MOVE_SEND as u8;
            reply(b, vec![(4, port, disp::MOVE_SEND)])
        };
        match (kind, req.id) {
            // mach_port_extract_right(task, name, how): a right taken out
            // of the task's space as a message's would be, and handed
            // back in the reply, to arrive under a name of its own.
            // (No such way of taking one; no such name; a name that has
            // no right to be taken that way.)
            (Kind::Task, 3215) => {
                let (name, how) = (get32(&req.body, 8), get32(&req.body, 12));
                if !(disp::MOVE_RECEIVE..=disp::MAKE_SEND_ONCE).contains(&how) {
                    plain(18, &[])
                } else if !sp.names.contains_key(&name) {
                    plain(KERN_INVALID_NAME as i32, &[])
                } else {
                    match sp.take(self.pid, name, how) {
                        Some((port, got)) => {
                            let mut b = vec![0u8; 16];
                            put32(&mut b, 0, 1);
                            b[4 + 10] = got as u8;
                            reply(b, vec![(4, port, got)])
                        }
                        None => plain(KERN_INVALID_RIGHT as i32, &[]),
                    }
                }
            }
            // mach_port_request_notification(task, name, which, sync,
            // notify): the name and the two words follow the right the
            // notification is to be sent by and the NDR record. The reply
            // has the right that was asked by before, or none. A request
            // refused never uses the right it came with, which that
            // right's port is told.
            (Kind::Task, 3213) => {
                let (name, which, sync) = (get32(&req.body, 24), get32(&req.body, 28), get32(&req.body, 32));
                let to = req.ports.first().map(|(_, p, got)| (Arc::clone(p), *got));
                let asked = match &to {
                    Some((_, got)) if *got != disp::MOVE_SEND_ONCE => Err(KERN_INVALID_CAPABILITY),
                    _ => sp.request(self.pid, name, which, sync, to.as_ref().map(|(p, _)| Arc::clone(p))),
                };
                match asked {
                    Ok(before) => {
                        let mut b = vec![0u8; 16];
                        put32(&mut b, 0, 1);
                        b[4 + 10] = disp::MOVE_SEND_ONCE as u8;
                        Msg { complex: true, ..reply(b, before.into_iter().map(|p| (4, p, disp::MOVE_SEND_ONCE)).collect()) }
                    }
                    Err(code) => {
                        if let Some((p, disp::MOVE_SEND_ONCE)) = &to {
                            sp.once_gone(p);
                        }
                        plain(code as i32, &[])
                    }
                }
            }
            // task_get_special_port(task, which): the task's own port
            // (TASK_KERNEL_PORT) and the host's (TASK_HOST_PORT) as
            // task_self and host_self give them; any other is the send
            // right the task was given (`task_set_special_port`, or from
            // the task it was forked from), the bootstrap port among them;
            // or none, as for a process started outside a launchd.
            (Kind::Task, 3409) => {
                let which = get32(&req.body, 8);
                let own = match which {
                    special::KERNEL => sp.names.get(&names::TASK).map(|e| Arc::clone(&e.port)),
                    special::HOST => sp.names.get(&names::HOST).map(|e| Arc::clone(&e.port)),
                    _ => sp.special.get(&which).filter(|p| !p.state().dead).map(|p| {
                        send_made(p);
                        Arc::clone(p)
                    }),
                };
                match own {
                    Some(port) => with_port(port),
                    None => {
                        let mut b = vec![0u8; 16];
                        put32(&mut b, 0, 1);
                        b[4 + 10] = disp::MOVE_SEND as u8;
                        Msg { complex: true, ..reply(b, Vec::new()) }
                    }
                }
            }
            // task_set_special_port(task, which, port): the send right the
            // request carries (the port descriptor, then the NDR record and
            // the number) is the task's from now on, in place of any it had.
            (Kind::Task, 3410) => {
                let which = get32(&req.body, 24);
                let port = req.ports.first().filter(|(_, _, got)| *got == disp::MOVE_SEND).map(|(_, p, _)| Arc::clone(p));
                if which == special::KERNEL || which == special::HOST || which == 0 || which > 16 {
                    plain(KERN_INVALID_ARGUMENT as i32, &[])
                } else {
                    // (The right the request carries is the one kept: it was
                    // counted as it was sent.)
                    let old = match port {
                        Some(p) => sp.special.insert(which, p),
                        None => sp.special.remove(&which),
                    };
                    if let Some(old) = old {
                        send_gone(&old);
                    }
                    plain(0, &[])
                }
            }
            // mach_port_get_context(task, name), mach_port_set_context(task,
            // name, context): what the receiver keeps with its port.
            (Kind::Task, 3228 | 3229) => {
                let name = get32(&req.body, 8);
                let context = req.body.get(12..20).map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap()));
                match sp.names.get(&name) {
                    None => plain(KERN_INVALID_NAME as i32, &[]),
                    Some(e) if !e.recv => plain(KERN_INVALID_RIGHT as i32, &[]),
                    Some(e) if req.id == 3228 => plain(0, &e.port.state().context.to_le_bytes()),
                    Some(e) => {
                        e.port.state().context = context;
                        plain(0, &[])
                    }
                }
            }
            // semaphore_create(task, out semaphore, policy, value): the
            // value follows the NDR record and the policy.
            (Kind::Task, 3418) => {
                let sem = Port::new(Kind::Semaphore);
                sem.state().count = get32(&req.body, 12) as i32 as i64;
                with_port(sem)
            }
            // semaphore_destroy(task, semaphore): its waiters are let go,
            // and its name is a name for what was one.
            (Kind::Task, 3419) => match req.ports.first() {
                Some((_, sem, _)) if sem.kind == Kind::Semaphore => {
                    let name = sp.names.iter().find(|(_, e)| Arc::ptr_eq(&e.port, sem)).map(|(n, _)| *n);
                    sem.state().dead = true;
                    if let Some(name) = name {
                        self.darwin_wake(m, sp, &[name], true);
                    }
                    plain(0, &[])
                }
                _ => plain(KERN_INVALID_ARGUMENT as i32, &[]),
            },
            // mach_vm_map(task, in-out address, size, mask, flags, object,
            // offset, copy, protection, most protection, inheritance), as
            // a message: what libsyscall sends when the call says more
            // than its trap carries (protections other than everything,
            // which is how an allocator reserves a range it will not yet
            // touch). One descriptor, the memory object's port, comes
            // before the NDR record; only no object is anonymous memory.
            // The task's ring of memory to be taken back
            // ([`super::reclaim`]): made (len, max_len after the NDR
            // record), some of it taken back at once (how many entries),
            // made a different length (which empties it), and where it is.
            // Each answers how much was taken and when the task should
            // next say what it holds.
            // task_info(task, flavor, out info, in-out count): what the
            // task is and has used, in 32-bit words; too little room for a
            // flavor is a bad argument. The sizes are the memory map's
            // (everything mapped, and what of it can be touched: nothing
            // is paged out here), the times the process's, and what is
            // not counted (faults, messages, switches) is none.
            (Kind::Task, 3405) => {
                let (flavor, room) = (get32(&req.body, 8), get32(&req.body, 12) as usize);
                let (size, resident, anon) = self.mem.lock().vm_counters();
                let regions = self.mem.lock().vmas().len() as u32;
                let cpu = self.cpu_nanos();
                // (A time is seconds and microseconds.)
                let time = [(cpu / 1_000_000_000) as u32, (cpu % 1_000_000_000 / 1000) as u32];
                let wide = |v: u64| [v as u32, (v >> 32) as u32];
                let narrow = |v: u64| v.min(u32::MAX as u64) as u32;
                const TIMESHARE: u32 = 1;
                let basic32 = || [&[0, narrow(size), narrow(resident)][..], &time, &[0, 0, TIMESHARE]].concat();
                let basic64 = || [&[0][..], &wide(size), &wide(resident), &time, &[0, 0, TIMESHARE]].concat();
                let words: Option<Vec<u32>> = match flavor {
                    // TASK_EVENTS_INFO
                    2 => Some(vec![0; 8]),
                    // TASK_THREAD_TIMES_INFO: its live threads' times.
                    3 => Some([&time[..], &[0, 0]].concat()),
                    // TASK_BASIC_INFO_32 and TASK_BASIC2_INFO_32: how many
                    // times suspended, the sizes, the times, the policy.
                    4 | 6 => Some(basic32()),
                    // TASK_BASIC_INFO_64: number 18 on arm64, where 5 has
                    // the 32-bit one's shape, and 5 on x86-64.
                    5 if gex_abi::X86 => Some(basic64()),
                    5 => Some(basic32()),
                    18 => Some(basic64()),
                    // TASK_SECURITY_TOKEN: who it runs as.
                    13 => Some(vec![self.creds.uid(), self.creds.gid()]),
                    // TASK_AUDIT_TOKEN: the audit user, the effective and
                    // real ids, the process, its audit session and which
                    // use of the process's number this is.
                    15 => Some(vec![self.creds.ruid(), self.creds.uid(), self.creds.gid(), self.creds.ruid(), self.creds.rgid(), self.pid as u32, self.procs.sid(self.pid, self.pid).unwrap_or(0) as u32, 1]),
                    // TASK_DYLD_INFO: where the dynamic linker's list of
                    // images is, its size and its format. The kernel is
                    // not told here: nowhere, in the 64-bit format.
                    17 => Some(vec![0, 0, 0, 0, 1]),
                    // MACH_TASK_BASIC_INFO: the sizes and the most that
                    // was resident, the times, the policy, how many times
                    // suspended.
                    20 => Some([&wide(size)[..], &wide(resident), &wide(resident), &time, &[0, 0, TIMESHARE, 0]].concat()),
                    // TASK_VM_INFO (and with what is purgeable): the map's
                    // size and regions, the page, what is resident and
                    // what of it is the task's own, its footprint, the
                    // addresses a task may have, and how much more it may
                    // use. It has grown by revisions, and a caller is
                    // given the last one it has room for.
                    22 | 23 => {
                        const REVISIONS: [usize; 8] = [36, 38, 42, 84, 86, 87, 89, 93];
                        REVISIONS.iter().rev().find(|n| **n <= room).map(|n| {
                            let mut v = vec![0u32; 93];
                            let mut put = |at: usize, x: u64| v[at..at + 2].copy_from_slice(&wide(x));
                            put(0, size);
                            put(4, resident);
                            put(6, resident);
                            put(12, anon);
                            put(14, anon);
                            put(36, resident);
                            // (a process with the window has none below 4 GiB)
                            put(38, abi::PAGE_SIZE.max(gex_abi::guest_base()));
                            put(40, if gex_abi::mem64() { gex_abi::LIMIT64 } else { (1 << 32) + gex_abi::guest_base() });
                            put(84, self.darwin_machine().mem as u64);
                            v[2] = regions;
                            v[3] = abi::PAGE_SIZE as u32;
                            v.truncate(*n);
                            v
                        })
                    }
                    _ => None,
                };
                match words {
                    Some(w) if room >= w.len() => {
                        let mut b = (w.len() as u32).to_le_bytes().to_vec();
                        w.iter().for_each(|x| b.extend_from_slice(&x.to_le_bytes()));
                        plain(0, &b)
                    }
                    _ => plain(KERN_INVALID_ARGUMENT as i32, &[]),
                }
            }
            // mach_vm_remap(target, in-out address, size, mask, flags,
            // source task, source address, copy, out protections,
            // inheritance): the source's memory at a second address in the
            // target, both of them this task. What is there arrives as a
            // copy, with the source's protection: which is the source
            // itself for memory that is not written (a page of code mapped
            // again, as the Objective-C runtime maps its trampolines). To
            // have one writable memory at two addresses is refused.
            (Kind::Task, 4813) => {
                let at = |o: usize| u64::from_le_bytes(req.body.get(o..o + 8).and_then(|b| b.try_into().ok()).unwrap_or([0; 8]));
                let (want, size, mask, flags, from, copy) = (at(24), at(32), at(40), get32(&req.body, 48) as u64, at(52), get32(&req.body, 60) != 0);
                // (the source as the map has it; `want` and the answer are the task's)
                let from = gex_abi::canon(from);
                let source_is_this = req.ports.first().is_some_and(|(_, p, _)| p.kind == Kind::Task);
                let len = size.next_multiple_of(abi::PAGE_SIZE);
                let prot = self.mem.lock().vma_at(from).map(|v| v.prot & 7);
                match prot {
                    _ if !source_is_this || size == 0 => plain(KERN_INVALID_ARGUMENT as i32, &[]),
                    Some(prot) if self.mem.lock().check_access(from, len, prot) && (copy || prot & crate::mem::prot::WRITE == 0) => {
                        let bytes = m.read(from, len as usize).map(|b| b.to_vec());
                        let made = bytes.map_err(|_| Errno::EFAULT).and_then(|bytes| {
                            let to = self.darwin_vm_map_at(m, want, len, mask, flags, crate::mem::prot::READ | crate::mem::prot::WRITE)?;
                            m.write(to, &bytes)?;
                            self.map_io().call(m, nr::MPROTECT, [to, len, prot as u64, 0, 0, 0]).unwrap_or(Err(Errno::EINVAL))?;
                            Ok(to)
                        });
                        match made {
                            Ok(to) => {
                                let mut out = to.to_le_bytes().to_vec();
                                out.extend_from_slice(&prot.to_le_bytes());
                                out.extend_from_slice(&prot.to_le_bytes());
                                plain(0, &out)
                            }
                            Err(e) => plain(super::mach::kern(Err(e)) as i32, &[]),
                        }
                    }
                    Some(_) if !copy => plain(46, &[]),
                    _ => plain(KERN_INVALID_ADDRESS as i32, &[]),
                }
            }
            // mach_vm_region(task, in-out address, out size, flavor, out
            // info, in-out count, out object name) and vm_region_64: the
            // region of the task's memory the address is in, or the next
            // above it, and what may be done there. The reply begins with
            // the name of the region's object, of which there is none.
            (Kind::Task, 4816 | 3824) => {
                let address = u64::from_le_bytes(req.body.get(8..16).and_then(|b| b.try_into().ok()).unwrap_or([0; 8]));
                let (flavor, room) = (get32(&req.body, 16), get32(&req.body, 20));
                // (the address as the map has it, the region's as the task does)
                let address = gex_abi::canon(address);
                let found = self.mem.lock().vmas().iter().find(|v| v.end > address).map(|v| (gex_abi::guest_addr(v.start), gex_abi::guest_addr(v.end), v.prot));
                match found {
                    // VM_REGION_BASIC_INFO_64: its protection and the most
                    // it may be given, how a child inherits it (a copy),
                    // whether it is shared, its offset in its object, how
                    // it is used, how often wired.
                    Some((start, end, prot)) if flavor == 9 && room >= 9 => {
                        let mut b = vec![0u8; 16];
                        put32(&mut b, 0, 1);
                        b[4 + 10] = disp::MOVE_SEND as u8;
                        b.extend_from_slice(&NDR);
                        b.extend_from_slice(&start.to_le_bytes());
                        b.extend_from_slice(&(end - start).to_le_bytes());
                        for word in [9, prot & 7, 7, 1, 0, 0, 0, 0, 0, 0] {
                            b.extend_from_slice(&word.to_le_bytes());
                        }
                        Msg { complex: true, ..reply(b, Vec::new()) }
                    }
                    Some(_) => plain(KERN_INVALID_ARGUMENT as i32, &[]),
                    None => plain(KERN_INVALID_ADDRESS as i32, &[]),
                }
            }
            // mach_vm_region_recurse(task, in-out address, out size,
            // in-out depth, out info, in-out count) and
            // vm_region_recurse_64: the same region, with what is known of
            // its pages (all resident and its own) and no submaps to go
            // down into.
            (Kind::Task, 4815 | 3822) => {
                let address = u64::from_le_bytes(req.body.get(8..16).and_then(|b| b.try_into().ok()).unwrap_or([0; 8]));
                let room = get32(&req.body, 20);
                let address = gex_abi::canon(address);
                let found = self.mem.lock().vmas().iter().find(|v| v.end > address).map(|v| (gex_abi::guest_addr(v.start), gex_abi::guest_addr(v.end), v.prot));
                match found {
                    Some((start, end, prot)) if room >= 17 => {
                        const SM_PRIVATE: u32 = 2;
                        let pages = if prot == 0 { 0 } else { ((end - start) / abi::PAGE_SIZE) as u32 };
                        let mut info = vec![prot & 7, 7, 1, 0, 0, 0, pages, 0, 0, pages, 1, SM_PRIVATE << 24, 0, 0, 0, 0, 0, 0, 0];
                        info.truncate(if room >= 19 { 19 } else { 17 });
                        let mut b = start.to_le_bytes().to_vec();
                        b.extend_from_slice(&(end - start).to_le_bytes());
                        b.extend_from_slice(&0u32.to_le_bytes());
                        b.extend_from_slice(&(info.len() as u32).to_le_bytes());
                        info.iter().for_each(|w| b.extend_from_slice(&w.to_le_bytes()));
                        plain(0, &b)
                    }
                    Some(_) => plain(KERN_INVALID_ARGUMENT as i32, &[]),
                    None => plain(KERN_INVALID_ADDRESS as i32, &[]),
                }
            }
            // thread_info(thread, flavor, out info, in-out count), likewise:
            // a thread's times and state, and which thread it is.
            (Kind::Thread(tid), 3612) => {
                let (flavor, room) = (get32(&req.body, 8), get32(&req.body, 12) as usize);
                // (Its own time is known; another's is not kept.)
                let cpu = if tid == self.tid { crate::proc::cpu_clock(true).unwrap_or(0) } else { 0 };
                let time = [(cpu / 1_000_000_000) as u32, (cpu % 1_000_000_000 / 1000) as u32];
                const TIMESHARE: u32 = 1;
                const RUNNING: u32 = 1;
                let words: Option<Vec<u32>> = match flavor {
                    // THREAD_BASIC_INFO: the times, how much of a processor
                    // it has lately used, its policy, state and flags, how
                    // many times suspended, how long asleep.
                    3 => Some([&time[..], &[0, 0, 0, TIMESHARE, RUNNING, 0, 0, 0]].concat()),
                    // THREAD_IDENTIFIER_INFO: its id (`thread_selfid`), then
                    // its handle and its queue's address, which libpthread
                    // keeps and the kernel is not told here. As much as
                    // there is room for, past the id's part.
                    4 if room >= 6 => {
                        let mut v = vec![0u32; room.min(32)];
                        v[0] = tid as u32;
                        Some(v)
                    }
                    // THREAD_EXTENDED_INFO: the times in nanoseconds, the
                    // same state, its priorities and its name (none).
                    5 => {
                        let mut v = vec![0u32; 28];
                        v[0] = (cpu % (1 << 32)) as u32;
                        v[1] = (cpu >> 32) as u32;
                        v[5] = TIMESHARE;
                        v[6] = RUNNING;
                        v[9] = 31;
                        v[10] = 31;
                        v[11] = 63;
                        Some(v)
                    }
                    _ => None,
                };
                match words {
                    Some(w) if room >= w.len() => {
                        let mut b = (w.len() as u32).to_le_bytes().to_vec();
                        w.iter().for_each(|x| b.extend_from_slice(&x.to_le_bytes()));
                        plain(0, &b)
                    }
                    _ => plain(KERN_INVALID_ARGUMENT as i32, &[]),
                }
            }
            // task_restartable_ranges_register(task, ranges, count)
            // ([`super::restart`]): how many, then each's location, length,
            // recovery offset and flags.
            (Kind::Task, 8000) => {
                let count = get32(&req.body, 8) as usize;
                let given: Option<Vec<(u64, u16, u16, u32)>> = (0..count.min(65))
                    .map(|i| {
                        let b = req.body.get(12 + 16 * i..28 + 16 * i)?;
                        Some((u64::from_le_bytes(b[..8].try_into().ok()?), u16::from_le_bytes([b[8], b[9]]), u16::from_le_bytes([b[10], b[11]]), get32(b, 12)))
                    })
                    .collect();
                match given {
                    Some(given) if count <= 64 => plain(self.darwin_restart.register(&given).err().unwrap_or(0), &[]),
                    _ => plain(KERN_INVALID_ARGUMENT as i32, &[]),
                }
            }
            // task_restartable_ranges_synchronize(task): every thread has
            // answered by now, since the call waited to be made until
            // they had ([`Self::darwin_restart_waits`]).
            (Kind::Task, RESTART_SYNCHRONIZE) => plain(0, &[]),
            (Kind::Task, 4822) => match self.darwin_reclaim_allocate(m, get32(&req.body, 8) as u64, get32(&req.body, 12) as u64) {
                Ok(at) => {
                    let mut out = at.to_le_bytes().to_vec();
                    out.extend_from_slice(&self.darwin_reclaim_deadline().to_le_bytes());
                    plain(0, &out)
                }
                Err(why) => plain(why as i32, &[]),
            },
            (Kind::Task, 4823 | 4826) => {
                let most = if req.id == 4823 { get32(&req.body, 8) as u64 } else { u64::MAX };
                match self.darwin_reclaim_take(m, most) {
                    Some(bytes) => {
                        let mut out = bytes.to_le_bytes().to_vec();
                        out.extend_from_slice(&self.darwin_reclaim_deadline().to_le_bytes());
                        plain(0, &out)
                    }
                    None => plain(KERN_INVALID_ARGUMENT as i32, &[]),
                }
            }
            (Kind::Task, 4828) => match self.darwin_reclaim_query(m) {
                Some((at, size)) => {
                    let mut out = at.to_le_bytes().to_vec();
                    out.extend_from_slice(&size.to_le_bytes());
                    plain(0, &out)
                }
                None => plain(KERN_INVALID_ARGUMENT as i32, &[]),
            },
            (Kind::Task, 4811) => {
                let at = |o: usize| u64::from_le_bytes(req.body.get(o..o + 8).and_then(|b| b.try_into().ok()).unwrap_or([0; 8]));
                let (want, size, mask, flags, prot) = (at(24), at(32), at(40), get32(&req.body, 48) as u64, get32(&req.body, 64) & 7);
                if get32(&req.body, 4) != 0 {
                    plain(KERN_INVALID_ARGUMENT as i32, &[])
                } else {
                    match self.darwin_vm_map_at(m, want, size, mask, flags, prot) {
                        Ok(addr) => plain(0, &addr.to_le_bytes()),
                        Err(e) => plain(super::mach::kern(Err(e)) as i32, &[]),
                    }
                }
            }
            // host_info(host, flavor, out info, in-out count): the flavor
            // and the room follow the NDR record. What is told is counted
            // in 32-bit words; too little room for it is a failure.
            (Kind::Host, 200) => {
                let (flavor, room) = (get32(&req.body, 8), get32(&req.body, 12) as usize);
                let mc = self.darwin_machine();
                let (cpus, mem) = (mc.cpus as u32, mc.mem as u64);
                let words: Option<Vec<u32>> = match flavor {
                    // HOST_BASIC_INFO: the processors and the memory. The
                    // first five words are what it was before there were
                    // more, and a caller with room for only those has them.
                    1 => Some(vec![
                        cpus,
                        cpus,
                        mem.min(i32::MAX as u64) as u32,
                        mc.cputype as u32,
                        mc.cpusubtype as u32,
                        0,
                        cpus,
                        cpus,
                        cpus,
                        cpus,
                        mem as u32,
                        (mem >> 32) as u32,
                    ])
                    .map(|mut v| {
                        if room < v.len() {
                            v.truncate(5);
                        }
                        v
                    }),
                    // HOST_SCHED_INFO: the least timeout and quantum, in
                    // milliseconds.
                    3 => Some(vec![10, 10]),
                    // HOST_PRIORITY_INFO: the scheduler's bands, of which
                    // libpthread reads the user's, the least and the most.
                    5 => Some(vec![80, 80, 64, 31, 0, 0, 0, 79]),
                    // HOST_MACH_MSG_TRAP: that there is one.
                    8 => Some(Vec::new()),
                    // HOST_PREFERRED_USER_ARCH
                    12 => Some(vec![mc.cputype as u32, mc.cpusubtype as u32]),
                    _ => None,
                };
                match words {
                    None => plain(KERN_INVALID_ARGUMENT as i32, &[]),
                    Some(w) if room < w.len() => plain(KERN_FAILURE as i32, &[]),
                    Some(w) => {
                        let mut b = (w.len() as u32).to_le_bytes().to_vec();
                        w.iter().for_each(|x| b.extend_from_slice(&x.to_le_bytes()));
                        plain(0, &b)
                    }
                }
            }
            // host_statistics and host_statistics64(host, flavor, out
            // info, in-out count): how loaded the machine is and what its
            // memory is doing, which `vm_stat`, `uptime` and `top` print.
            // There is no load kept here and no paging: the averages are
            // nothing, the processors idle, and memory is half free, the
            // rest of it in use the ordinary ways. A caller with room for
            // less (an older structure) is given that much.
            (Kind::Host, 216 | 219) => {
                let (flavor, room) = (get32(&req.body, 8), get32(&req.body, 12) as usize);
                let mc = self.darwin_machine();
                let pages = (mc.mem as u64 / abi::PAGE_SIZE) as u32;
                let words = match flavor {
                    // HOST_LOAD_INFO: three averages, and three Mach
                    // factors (the processors free), in thousandths.
                    1 => Some(vec![0, 0, 0, mc.cpus as u32 * 1000, mc.cpus as u32 * 1000, mc.cpus as u32 * 1000]),
                    // HOST_VM_INFO: free, active, inactive and wired
                    // pages, then counts of what has happened (nothing).
                    2 => {
                        let mut w = vec![0u32; 15];
                        w[..4].copy_from_slice(&[pages / 2, pages / 4, pages / 8, pages / 8]);
                        Some(w)
                    }
                    // HOST_CPU_LOAD_INFO: ticks spent in user code, the
                    // system's, idle and at low priority.
                    3 => {
                        let ticks = (clock_ns(self.clock, 1) / 10_000_000) as u32;
                        Some(vec![ticks / 16, ticks / 32, ticks, 0])
                    }
                    // HOST_VM_INFO64: the same four, the counts 64 bits
                    // wide, and what is file-backed or not.
                    4 if req.id == 219 => {
                        let mut w = vec![0u32; 38];
                        w[..4].copy_from_slice(&[pages / 2, pages / 4, pages / 8, pages / 8]);
                        (w[34], w[35]) = (pages / 8, pages / 4);
                        Some(w)
                    }
                    _ => None,
                };
                match words {
                    None => plain(KERN_INVALID_ARGUMENT as i32, &[]),
                    Some(_) if room == 0 => plain(KERN_FAILURE as i32, &[]),
                    Some(mut w) => {
                        w.truncate(room);
                        let mut b = (w.len() as u32).to_le_bytes().to_vec();
                        w.iter().for_each(|x| b.extend_from_slice(&x.to_le_bytes()));
                        plain(0, &b)
                    }
                }
            }
            // _host_page_size(host, out size)
            (Kind::Host, 202) => plain(0, &abi::PAGE_SIZE.to_le_bytes()),
            // host_request_notification(host, what, port): to be told when
            // the calendar is set, which it never is here. (The request
            // has the port before the NDR record.)
            (Kind::Host, 217) => plain(if get32(&req.body, 24) <= 1 { 0 } else { KERN_INVALID_ARGUMENT as i32 }, &[]),
            // host_get_special_port(host, node, which, out port): none
            // that a program without privilege may have.
            (Kind::Host, 412) => plain(KERN_INVALID_ARGUMENT as i32, &[]),
            // host_get_io_master(host, out master): IOKit's way in.
            (Kind::Host, 205) => with_port(Port::new(Kind::IoMaster)),
            (Kind::IoMaster, id) => {
                let code = self.darwin_iokit(id, &req.body);
                plain(code, &[])
            }
            // host_get_clock_service(host, clock id, out clock)
            (Kind::Host, 206) => with_port(Port::new(Kind::Clock)),
            // clock_get_time(clock, out mach_timespec): the system clock,
            // which counts from the boot.
            (Kind::Clock, 1000) => {
                let ns = clock_ns(self.clock, 1);
                let mut t = ((ns / 1_000_000_000) as u32).to_le_bytes().to_vec();
                t.extend_from_slice(&((ns % 1_000_000_000) as u32).to_le_bytes());
                plain(0, &t)
            }
            _ => {
                let n = MACH + super::nr::trap::MACH_MSG;
                if self.trace || std::env::var_os("GEX_DARWIN_UNIMPLEMENTED").is_some() {
                    eprintln!("[gex:{}] MIG routine {} of the {kind:?} port: not one gex answers", self.pid, req.id);
                }
                if !self.unimplemented.contains(&n) {
                    self.unimplemented.push(n);
                }
                plain(MIG_BAD_ID, &[])
            }
        }
    }
}
