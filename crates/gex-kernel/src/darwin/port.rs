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
//! task's and the host's, a clock, semaphores. A message's rights and
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
use crate::syscalls::nr;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

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

pub const MACH_MSG_SUCCESS: u64 = 0;
pub const MACH_SEND_INVALID_DATA: u64 = 0x1000_0002;
pub const MACH_SEND_MSG_TOO_SMALL: u64 = 0x1000_0008;
pub const MACH_SEND_INVALID_REPLY: u64 = 0x1000_0009;
pub const MACH_SEND_INVALID_RIGHT: u64 = 0x1000_000a;
pub const MACH_SEND_INVALID_TYPE: u64 = 0x1000_000f;
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
    /// A semaphore, which is waited on and signalled by traps of its own.
    Semaphore,
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
}

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
}

impl Entry {
    fn of(port: Arc<Port>) -> Entry {
        Entry { port, recv: false, send: 0, once: false, dead: 0, members: Vec::new() }
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
}

impl Default for Space {
    fn default() -> Space {
        let mut names = HashMap::new();
        for (name, kind) in [(names::TASK, Kind::Task), (names::HOST, Kind::Host)] {
            names.insert(name, Entry { send: 1, ..Entry::of(Port::new(kind)) });
        }
        Space { names, next: names::FIRST_ALLOCATED, words: 0 }
    }
}

impl Space {
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

    /// The entry under `name`. A thread's port is there for the asking.
    fn entry(&mut self, pid: i32, name: u32) -> Option<&mut Entry> {
        if !self.names.contains_key(&name) {
            let tid = port_thread(pid, name)?;
            self.names.insert(name, Entry { send: 1, ..Entry::of(Port::new(Kind::Thread(tid))) });
        }
        self.names.get_mut(&name)
    }

    fn drop_if_empty(&mut self, name: u32) {
        if self.names.get(&name).is_some_and(Entry::empty) {
            self.names.remove(&name);
        }
    }

    /// Take a right from under `name`, as a message being sent does:
    /// the port, and the type its receiver will be told.
    fn take(&mut self, pid: i32, name: u32, how: u32) -> Option<(Arc<Port>, u32)> {
        let e = self.entry(pid, name)?;
        let port = Arc::clone(&e.port);
        let got = match how {
            disp::COPY_SEND if e.send > 0 => disp::MOVE_SEND,
            disp::MOVE_SEND if e.send > 0 => {
                // The kernel's own ports are not used up.
                if port.kind == Kind::User || port.kind == Kind::Semaphore {
                    e.send -= 1;
                }
                disp::MOVE_SEND
            }
            disp::MAKE_SEND if e.recv => disp::MOVE_SEND,
            disp::MAKE_SEND_ONCE if e.recv => disp::MOVE_SEND_ONCE,
            disp::MOVE_SEND_ONCE if e.once => {
                e.once = false;
                disp::MOVE_SEND_ONCE
            }
            disp::MOVE_RECEIVE if e.recv => {
                e.recv = false;
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
                    port.state().wake = vec![name];
                } else {
                    e.send += 1;
                }
                name
            }
            (_, None) => {
                let name = self.fresh();
                let mut e = Entry::of(Arc::clone(&port));
                if got == disp::MOVE_RECEIVE {
                    e.recv = true;
                    port.state().wake = vec![name];
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
        port.state().wake = vec![name];
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
    /// name. The names to wake.
    fn destroy_receive(&mut self, name: u32) -> Vec<u32> {
        let Some(e) = self.names.get_mut(&name) else { return Vec::new() };
        e.recv = false;
        let port = Arc::clone(&e.port);
        let wake = {
            let mut st = port.state();
            st.dead = true;
            st.queue.clear();
            std::mem::take(&mut st.wake)
        };
        for e in self.names.values_mut().filter(|e| Arc::ptr_eq(&e.port, &port)) {
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

fn get32(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes(s.try_into().unwrap()))
}

impl Kernel {
    fn darwin_space(&self) -> Arc<Mutex<Space>> {
        Arc::clone(&self.darwin_ports)
    }

    /// The word a thread waiting on `name` sleeps on, in the page of words
    /// the task's memory holds for its space (made at the first need).
    fn darwin_word(&mut self, m: &mut GuestMem<'_>, sp: &mut Space, name: u32) -> Result<u64, Errno> {
        if sp.words == 0 || self.mem.lock().vma_at(sp.words).is_none() {
            sp.words = self.mem.mmap_anon(m, 0, abi::PAGE_SIZE, 3, 0, false, "[mach ports]")?;
        }
        Ok(sp.words + 4 * ((name as u64 >> 8) & 0xfff))
    }

    /// Say that `names` have something for whoever waits on them.
    fn darwin_wake(&mut self, m: &mut GuestMem<'_>, sp: &mut Space, names: &[u32], all: bool) {
        for &name in names {
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

    /// The traps that name and count rights. `None`: not one of them.
    pub(super) fn darwin_port_trap(&mut self, m: &mut GuestMem<'_>, n: u64, x: [u64; 8]) -> Option<u64> {
        use super::nr::trap as t;
        let space = self.darwin_space();
        let mut sp = space.lock().unwrap_or_else(|e| e.into_inner());
        let pid = self.pid;
        let name = x[1] as u32;
        // Every one of them is asked of a task, which must be this one.
        let own = x[0] as u32 == names::TASK;
        let bad_task = |r: u64| if own { r } else { MACH_SEND_INVALID_DEST };
        Some(match n {
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
                    sp.names.get_mut(&made).unwrap().send = 1;
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
                    if matches!(e.port.kind, Kind::User | Kind::Semaphore) {
                        e.send -= 1;
                    }
                    KERN_SUCCESS
                }
                Some(e) if e.once => {
                    e.once = false;
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
                                if matches!(e.port.kind, Kind::User | Kind::Semaphore) {
                                    e.send = v;
                                }
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
                    e.send = e.send.saturating_add_signed(x[2] as i32);
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
                                let e = sp.names.entry(name).or_insert_with(|| Entry::of(Arc::clone(&port)));
                                if got == disp::MOVE_RECEIVE {
                                    e.recv = true;
                                } else {
                                    e.send += 1;
                                }
                                KERN_SUCCESS
                            }
                        }
                    }
                }
            }),
            // (task, name, set): a receive right into a port set, or (set 0)
            // out of the one it is in.
            t::MACH_PORT_INSERT_MEMBER | t::MACH_PORT_MOVE_MEMBER | t::MACH_PORT_EXTRACT_MEMBER => bad_task({
                let set = if n == t::MACH_PORT_EXTRACT_MEMBER { 0 } else { x[2] as u32 };
                let member = sp.names.get(&name).filter(|e| e.recv).map(|e| Arc::clone(&e.port));
                let is_set = set == 0 || sp.names.get(&set).is_some_and(Entry::is_set);
                match member {
                    None if !sp.names.contains_key(&name) => KERN_INVALID_NAME,
                    None => KERN_INVALID_RIGHT,
                    Some(_) if !is_set => KERN_INVALID_RIGHT,
                    Some(port) => {
                        for e in sp.names.values_mut().filter(|e| e.is_set()) {
                            e.members.retain(|m| *m != name);
                        }
                        let ready = {
                            let mut st = port.state();
                            st.wake.truncate(1);
                            if set != 0 {
                                st.wake.push(set);
                            }
                            !st.queue.is_empty()
                        };
                        if set != 0 {
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
    pub(super) fn darwin_wait_trap(&mut self, m: &mut GuestMem<'_>, n: u64, x: [u64; 8], a: &mut [u64; 6]) -> Option<Result<u64, u64>> {
        use super::nr::trap as t;
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
        // What is asked of: a receive right, or a set of them.
        let from = match sp.names.get(&c.rcv_name) {
            Some(e) if e.is_set() => e.members.iter().filter_map(|n| sp.names.get(n).map(|e| (*n, Arc::clone(&e.port)))).collect::<Vec<_>>(),
            Some(e) if e.recv => vec![(c.rcv_name, Arc::clone(&e.port))],
            _ => return Err(MACH_RCV_INVALID_NAME),
        };
        for (name, port) in from {
            let mut st = port.state();
            let Some(msg) = st.queue.front() else { continue };
            let size = HEADER + msg.body.len() as u64;
            if size + TRAILER > c.rcv_size {
                // Too large: with RCV_LARGE it stays queued and its size
                // is said; without, it is gone.
                let mut head = [0u8; HEADER as usize];
                put32(&mut head, 4, size as u32);
                put32(&mut head, 12, name);
                put32(&mut head, 20, msg.id);
                if c.option & opt::RCV_LARGE == 0 {
                    st.queue.pop_front();
                }
                if c.rcv_size >= HEADER {
                    let _ = m.write(c.rcv_at, &head);
                }
                return Err(MACH_RCV_TOO_LARGE);
            }
            let msg = st.queue.pop_front().unwrap();
            drop(st);
            let mut out = vec![0u8; (size + TRAILER) as usize];
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
            let dest_type = if msg.once { disp::MOVE_SEND_ONCE } else { disp::MOVE_SEND };
            put32(&mut out, 0, reply_type | dest_type << 8 | if msg.complex { COMPLEX } else { 0 });
            put32(&mut out, 4, size as u32);
            put32(&mut out, 8, reply_name);
            put32(&mut out, 12, name);
            put32(&mut out, 20, msg.id);
            // The trailer: format 0, eight bytes.
            put32(&mut out, size as usize + 4, TRAILER as u32);
            return Err(match m.write(c.rcv_at, &out) {
                Ok(()) => MACH_MSG_SUCCESS,
                Err(_) => MACH_RCV_INVALID_DATA,
            });
        }
        // Nothing there: say so, or wait to be told to look again.
        let timeout = (c.option & opt::RCV_TIMEOUT != 0).then_some(c.timeout * 1_000_000);
        if timeout == Some(0) {
            return Err(MACH_RCV_TIMED_OUT);
        }
        match self.darwin_wait(m, &mut sp, c.rcv_name, timeout, a) {
            Ok(call) => {
                self.darwin_post = Post::Wake { woken: MACH_RCV_INTERRUPTED as u32, timed_out: MACH_RCV_TIMED_OUT as u32 };
                Ok(call)
            }
            Err(_) => Err(MACH_RCV_INVALID_DATA),
        }
    }

    /// The sending half: the message leaves the caller's memory and its
    /// rights the caller's space, for the destination's queue or, when the
    /// destination is the kernel's, for the routine it asks for.
    fn darwin_send(&mut self, m: &mut GuestMem<'_>, sp: &mut Space, c: &MsgCall) -> u64 {
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
        if complex {
            let count = get32(&body, 0) as usize;
            let mut at = 4;
            for _ in 0..count {
                if at + 12 > body.len() {
                    return MACH_SEND_MSG_TOO_SMALL;
                }
                // A port descriptor: the name, then its disposition and
                // type in the last two bytes. Memory out of line, and
                // arrays of ports, are not carried yet.
                if body[at + 11] != 0 {
                    return MACH_SEND_INVALID_TYPE;
                }
                descs.push((at, get32(&body, at), body[at + 10] as u32));
                at += 12;
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
        let msg = Msg { id: c.id, body, complex, once: dest_got == disp::MOVE_SEND_ONCE, reply, ports };
        match dest.kind {
            Kind::User => {
                let wake = {
                    let mut st = dest.state();
                    if st.dead {
                        return MACH_SEND_INVALID_DEST;
                    }
                    st.queue.push_back(msg);
                    st.wake.clone()
                };
                self.darwin_wake(m, sp, &wake, true);
            }
            Kind::Set => return MACH_SEND_INVALID_DEST,
            kind => {
                // The kernel's: the routine is run, and its answer queued
                // on the reply port for the receive that follows.
                let Some((reply, once)) = msg.reply.clone() else { return MACH_MSG_SUCCESS };
                let answer = self.darwin_mig(m, sp, kind, &msg);
                let wake = {
                    let mut st = reply.state();
                    st.queue.push_back(Msg { once, reply: None, ..answer });
                    st.wake.clone()
                };
                self.darwin_wake(m, sp, &wake, true);
            }
        }
        MACH_MSG_SUCCESS
    }

    /// A request to one of the kernel's own ports, answered as its MIG
    /// server answers: the routine by the message's number, the reply
    /// numbered a hundred more. A routine there is none of is `MIG_BAD_ID`.
    fn darwin_mig(&mut self, m: &mut GuestMem<'_>, sp: &mut Space, kind: Kind, req: &Msg) -> Msg {
        let reply = |body: Vec<u8>, ports: Vec<(usize, Arc<Port>, u32)>| Msg { id: req.id + 100, complex: !ports.is_empty(), body, once: false, reply: None, ports };
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
            // _host_page_size(host, out size)
            (Kind::Host, 202) => plain(0, &abi::PAGE_SIZE.to_le_bytes()),
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
