//! Events delivered to the work queue's threads: the work queue's own
//! kqueue (`KEVENT_FLAG_WORKQ`) and workloops (`kevent_id`,
//! `EVFILT_WORKLOOP`), which is how libdispatch is told of everything it
//! waits for once the kernel says it can (`WORKQ_FEATURE_KEVENT`,
//! `WORKQ_FEATURE_WORKLOOP`), and what Apple's libdispatch will not start
//! without.
//!
//! XNU's model (`bsd/kern/kern_event.c`, xnu-4903), as libdispatch sees
//! it:
//!
//! - A process has one work-queue kqueue. What is registered on it is
//!   delivered to a thread of the work queue, which the kernel asks for
//!   when a note has something to say, and which is entered at
//!   libdispatch's kevent routine with the events under its `pthread_t`
//!   ([`super::workq::event_areas`]). It comes back with the changes it
//!   made of them (`WQOPS_THREAD_KEVENT_RETURN`), and is told of more, if
//!   there are any, or let go. Here that thread is always the event
//!   manager (`WQ_FLAG_THREAD_EVENT_MANAGER`), one at a time: libdispatch
//!   merges events there and hands the work on to its queues, as XNU's
//!   manager bucket does.
//! - A workloop is a kqueue of its own, named by an id (libdispatch's is a
//!   serial queue's address), made at its first use. Its notes are those
//!   of any filter, and of `EVFILT_WORKLOOP`: a thread request (the note
//!   whose ident is the workloop's id, `NOTE_WL_THREAD_REQUEST`), which is
//!   said once each time it is put on; and synchronous waiters
//!   (`NOTE_WL_SYNC_WAIT`, whose ident is the waiting thread's), on which
//!   the thread that registers it waits in `kevent_id` until another wakes
//!   it (`NOTE_WL_SYNC_WAKE`) or deletes it. A workloop with something to
//!   say gets a thread of the work queue of its own -- one at a time, its
//!   servicer -- entered at libdispatch's workloop routine
//!   (`WQ_FLAG_THREAD_WORKLOOP`) with the id under the events; it comes
//!   back with `WQOPS_THREAD_WORKLOOP_RETURN`.
//! - A change may say what it expects a word of the caller's memory to
//!   hold (`ext[EV_EXTIDX_WL_ADDR]`, `_MASK`, `_VALUE`: a queue's state),
//!   and is refused with `ESTALE` when the word holds otherwise
//!   (unless `NOTE_WL_IGNORE_ESTALE`, when it is dropped without a word).
//!   It may name the thread that owns the workloop (`NOTE_WL_DISCOVER_OWNER`,
//!   the port name in the word's low bits) and end that ownership
//!   (`NOTE_WL_END_OWNERSHIP`). A workloop with an owner gets no
//!   servicer: its owner drains it, and when ownership ends one is asked
//!   for if there is anything to say.
//!
//! Priorities are not kept: a thread is entered with the class its
//! request names, and nothing is overridden.
//!
//! What those queues watch -- descriptors, timers, ports -- has to be
//! waited on by someone while no thread is told of it. That is a thread of
//! the work queue that is the kernel's own ([`super::workq::POLL_STUB`]):
//! it waits on every such queue without a servicer, and asks for one when
//! a queue has something to say.

use super::kqueue::{ev, poll, Event, Form, Kqueue};
use super::workq::{event_areas, flag, flags_of, pthread_of, Work, Worker, Workq, KEVENT_DATA, KEVENT_LIST, KEVENT_SIZE};
use super::nr as d;
use super::*;
use crate::syscalls::nr;
use crate::vfs::{FileHandle, FileType, Metadata};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Weak};

/// `kevent_id`'s and `kevent_qos`'s flags.
mod kf {
    pub const IMMEDIATE: u64 = 0x1;
    pub const ERROR_EVENTS: u64 = 0x2;
    pub const MUST_EXIST: u64 = 0x2_0000;
    pub const MUST_NOT_EXIST: u64 = 0x4_0000;
}

/// `EVFILT_WORKLOOP`.
const WORKLOOP: i16 = -17;

/// A workloop note's `fflags`.
mod wl {
    pub const THREAD_REQUEST: u32 = 0x1;
    pub const SYNC_WAIT: u32 = 0x4;
    pub const SYNC_WAKE: u32 = 0x8;
    pub const COMMANDS: u32 = 0xf;
    pub const UPDATE_QOS: u32 = 0x10;
    pub const END_OWNERSHIP: u32 = 0x20;
    pub const DISCOVER_OWNER: u32 = 0x80;
    pub const IGNORE_ESTALE: u32 = 0x100;
    pub const UPDATES: u32 = 0x1f0;
}

/// Where a workloop note keeps the word it is checked against.
const ADDR: usize = 1;
const MASK: usize = 2;
const VALUE: usize = 3;

/// Darwin's own error numbers, for those Linux spells otherwise.
fn estale() -> Errno {
    abi::errno_raw(70)
}
fn eownerdead() -> Errno {
    abi::errno_raw(105)
}

/// Whose events a thread is told of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// The work queue's own kqueue.
    Kqwq,
    /// A workloop, by its id.
    Loop(u64),
}

/// A workloop's note of `EVFILT_WORKLOOP`: what it was registered with,
/// and last changed by.
#[derive(Clone, Copy, Debug)]
struct Knote {
    udata: u64,
    qos: u32,
    flags: u16,
    sfflags: u32,
    sdata: i64,
    ext: [u64; 4],
}

/// A workloop.
pub struct Workloop {
    /// Its notes of the other filters.
    kq: Arc<Kqueue>,
    /// Its thread request, and whether it is to be said.
    request: Option<(Knote, bool)>,
    /// Its synchronous waiters' notes, by ident, and the thread that waits
    /// on each.
    syncs: HashMap<u64, (Knote, Option<i32>)>,
    /// The thread that owns it, if a change said.
    owner: Option<i32>,
    /// The thread told of its events, and whether one is asked for.
    servicer: Option<i32>,
    requested: bool,
}

impl Workloop {
    fn new(clock: crate::syscalls::ClockFn) -> Workloop {
        Workloop { kq: Arc::new(Kqueue::new(clock)), request: None, syncs: HashMap::new(), owner: None, servicer: None, requested: false }
    }

    /// Whether it has something to say to a servicer.
    fn ready(&self) -> bool {
        self.request.is_some_and(|(_, on)| on) || self.kq.any_ready()
    }

    /// Whether nothing is left of it.
    fn idle(&self) -> bool {
        self.request.is_none() && self.syncs.is_empty() && self.owner.is_none() && self.servicer.is_none() && !self.requested && self.kq.is_empty()
    }
}

/// A thread waiting in `kevent_id` on a note of its own: where its call
/// had got to.
#[derive(Clone, Debug)]
struct Waiter {
    id: u64,
    ident: u64,
    /// The next change to make.
    next: usize,
    /// What the changes before have said.
    out: Vec<Event>,
}

/// The queues the work queue's threads are told of.
#[derive(Default)]
pub struct Events {
    kqwq: Option<Arc<Kqueue>>,
    kqwq_servicer: Option<i32>,
    kqwq_requested: bool,
    loops: HashMap<u64, Workloop>,
    /// The kernel's own thread, once it is asked for or there.
    pub poller: Option<i32>,
    /// What it waits on besides the notes' descriptors.
    pump: Option<(i32, Arc<Pump>)>,
    waiters: HashMap<i32, Waiter>,
}

impl std::fmt::Debug for Events {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Events").field("loops", &self.loops.len()).field("poller", &self.poller).finish()
    }
}

impl Events {
    /// Whether target `t` wants a thread: it has none, none is asked for,
    /// nobody owns it, and it has something to say.
    fn wants(&self, t: Target) -> bool {
        match t {
            Target::Kqwq => self.kqwq_servicer.is_none() && !self.kqwq_requested && self.kqwq.as_ref().is_some_and(|kq| kq.any_ready()),
            Target::Loop(id) => self.loops.get(&id).is_some_and(|l| l.servicer.is_none() && !l.requested && l.owner.is_none() && l.ready()),
        }
    }

    /// Every target that may have something to say with nobody to say it
    /// to: what the kernel's thread watches.
    fn unserved(&self) -> Vec<Target> {
        let mut out = Vec::new();
        if self.kqwq.is_some() && self.kqwq_servicer.is_none() && !self.kqwq_requested {
            out.push(Target::Kqwq);
        }
        for (id, l) in &self.loops {
            if l.servicer.is_none() && !l.requested && l.owner.is_none() {
                out.push(Target::Loop(*id));
            }
        }
        out
    }

    fn kq(&self, t: Target) -> Option<Arc<Kqueue>> {
        match t {
            Target::Kqwq => self.kqwq.clone(),
            Target::Loop(id) => self.loops.get(&id).map(|l| Arc::clone(&l.kq)),
        }
    }
}

/// What the kernel's own thread waits on besides the notes' descriptors:
/// readable when it has been rung (a change was made to a queue, or a
/// servicer let one go), or when a queue nobody serves has something to
/// say.
pub struct Pump {
    rung: AtomicBool,
    wq: Weak<Mutex<Workq>>,
}

impl std::fmt::Debug for Pump {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pump").finish()
    }
}

impl FileHandle for Pump {
    fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> SysResult {
        Err(Errno::ENXIO)
    }
    fn write_at(&self, _offset: u64, _buf: &[u8]) -> SysResult {
        Err(Errno::ENXIO)
    }
    fn metadata(&self) -> Result<Metadata, Errno> {
        Ok(Metadata { file_type: FileType::Fifo, size: 0, mode: 0o600, ino: 0, mtime: 0, nlink: 1, rdev: 0, times: Default::default() })
    }
    fn poll_readable(&self) -> bool {
        if self.rung.load(Ordering::SeqCst) {
            return true;
        }
        let Some(wq) = self.wq.upgrade() else { return true };
        // (Whoever holds the queue is changing it: it rings when done.)
        let Ok(wq) = wq.try_lock() else { return false };
        wq.events.unserved().into_iter().any(|t| wq.events.wants(t))
    }
    fn poll_writable(&self) -> bool {
        false
    }
}

impl Kernel {
    /// The work queue's kqueue, made at its first need.
    fn darwin_kqwq(&self, ev: &mut Events) -> Arc<Kqueue> {
        Arc::clone(ev.kqwq.get_or_insert_with(|| Arc::new(Kqueue::new(self.clock))))
    }

    /// Ring the kernel's thread: a queue changed.
    fn darwin_events_ring(&mut self, ev: &Events) {
        if let Some((_, pump)) = &ev.pump {
            pump.rung.store(true, Ordering::SeqCst);
        }
        self.fd_activity = true;
    }

    /// Ask for a thread for each of `targets` that wants one, and for the
    /// kernel's own if there is none yet: the threads that wait take the
    /// work, and what is left is made a thread for by this call's end, if
    /// it can (`darwin_workq_does`).
    fn darwin_events_kick(&mut self, m: &mut GuestMem<'_>, wq: &mut Workq, targets: &[Target]) {
        if wq.events.poller.is_none() {
            wq.events.poller = Some(-1);
            wq.pending.push_front(Work::Poll);
        }
        for &t in targets {
            if wq.events.wants(t) {
                match t {
                    Target::Kqwq => wq.events.kqwq_requested = true,
                    Target::Loop(id) => wq.events.loops.get_mut(&id).unwrap().requested = true,
                }
                wq.pending.push_back(Work::Events(t));
            }
        }
        let made = self.darwin_workq_hand_out(m, wq);
        if let Some(work) = made {
            if self.darwin_workq_does.is_none() {
                self.darwin_workq_does = Some(super::workq::Does::Make(work));
            } else {
                wq.pending.push_front(work);
            }
        }
        self.darwin_events_ring(&wq.events);
    }

    /// The thread let go of `t`, back with nothing more: it may want
    /// another, and the kernel's thread watches it again.
    pub(super) fn darwin_events_unbind(&mut self, wq: &mut Workq, t: Target) {
        match t {
            Target::Kqwq => {
                if wq.events.kqwq_servicer == Some(self.tid) {
                    wq.events.kqwq_servicer = None;
                }
            }
            Target::Loop(id) => {
                if let Some(l) = wq.events.loops.get_mut(&id) {
                    if l.servicer == Some(self.tid) {
                        l.servicer = None;
                    }
                    if l.idle() {
                        wq.events.loops.remove(&id);
                    }
                }
            }
        }
        if wq.events.wants(t) {
            match t {
                Target::Kqwq => wq.events.kqwq_requested = true,
                Target::Loop(id) => wq.events.loops.get_mut(&id).unwrap().requested = true,
            }
            wq.pending.push_back(Work::Events(t));
        }
        self.darwin_events_ring(&wq.events);
    }

    /// What `t` has to say to its servicer (this thread), up to the room
    /// the thread has: written under its `pthread_t`, the messages they
    /// receive under them. How many.
    fn darwin_events_collect(&mut self, m: &mut GuestMem<'_>, wq: &mut Workq, w: Worker, t: Target) -> i64 {
        let me = pthread_of(w.stack);
        let (list, id_at, data, _) = event_areas(me);
        let mut out = Vec::new();
        let kq = match t {
            Target::Kqwq => wq.events.kqwq.clone(),
            Target::Loop(id) => {
                let Some(l) = wq.events.loops.get_mut(&id) else { return 0 };
                // The thread request, once each time it is put on, while
                // nobody owns the workloop.
                if l.owner.is_none() {
                    if let Some((kn, on)) = &mut l.request {
                        if *on {
                            *on = false;
                            out.push(Event { ident: id, filter: WORKLOOP, flags: kn.flags | ev::CLEAR, fflags: kn.sfflags, data: kn.sdata, udata: kn.udata, qos: kn.qos, ext: kn.ext });
                        }
                    }
                }
                let _ = m.write_u64(id_at, id);
                Some(Arc::clone(&l.kq))
            }
        };
        if let Some(kq) = kq {
            out.extend(kq.collect(KEVENT_LIST as usize - out.len(), &Default::default()));
            self.darwin_kevent_ports(m, &mut out, Some((data, KEVENT_DATA)));
            for fd in kq.take_closed() {
                let _ = self.sys_close(fd);
            }
        }
        for (i, e) in out.iter().enumerate() {
            if e.write(m, list + i as u64 * KEVENT_SIZE, Form::Qos).is_err() {
                return i as i64;
            }
        }
        out.len() as i64
    }

    /// The flags a servicer of `t` is entered with.
    fn darwin_events_flags(wq: &Workq, t: Target) -> u32 {
        match t {
            Target::Kqwq => flag::NEWSPI | flag::KEVENT | flag::EVENT_MANAGER,
            Target::Loop(id) => {
                let qos = wq.events.loops.get(&id).and_then(|l| l.request.map(|(kn, _)| kn.qos as u64)).unwrap_or(0);
                (flags_of(qos) & !(flag::EVENT_MANAGER | flag::COOPERATIVE)) | flag::KEVENT | flag::WORKLOOP
            }
        }
    }

    /// Thread `w` (this one) is to be told of `t`'s events: it is `t`'s
    /// servicer, and what it is entered with, if there is anything to say;
    /// `None` if not (or `t` is another's, or owned).
    pub(super) fn darwin_events_deliver(&mut self, m: &mut GuestMem<'_>, wq: &mut Workq, w: Worker, t: Target) -> Option<(u32, i64)> {
        let tid = self.tid;
        match t {
            Target::Kqwq => {
                wq.events.kqwq_requested = false;
                if wq.events.kqwq_servicer.is_some_and(|s| s != tid) {
                    return None;
                }
                wq.events.kqwq_servicer = Some(tid);
            }
            Target::Loop(id) => {
                let l = wq.events.loops.get_mut(&id)?;
                l.requested = false;
                if l.servicer.is_some_and(|s| s != tid) || l.owner.is_some() {
                    return None;
                }
                l.servicer = Some(tid);
            }
        }
        let n = self.darwin_events_collect(m, wq, w, t);
        if n == 0 {
            self.darwin_events_unbind(wq, t);
            return None;
        }
        if self.trace {
            eprintln!("[gex:{}] workq: thread {} told of {} event(s) of {:?}", self.pid, tid, n, t);
        }
        Some((Self::darwin_events_flags(wq, t), n))
    }

    /// Servicer `w` of `t` is back with `n` changes at `changes`: they are
    /// made, and it is told of what they refused, or of what there is now;
    /// `None` when there is nothing, and it is to let `t` go.
    pub(super) fn darwin_events_return(&mut self, m: &mut GuestMem<'_>, wq: &mut Workq, w: Worker, t: Target, changes: u64, n: u64) -> Option<(u32, i64)> {
        let mut errors = Vec::new();
        for i in 0..n.min(KEVENT_LIST * 4) {
            let Ok(mut e) = Event::read(m, changes + i * KEVENT_SIZE, Form::Qos) else { break };
            let r = match t {
                Target::Kqwq => {
                    let kq = self.darwin_kqwq(&mut wq.events);
                    self.darwin_kq_apply(m, &kq, &e)
                }
                Target::Loop(id) => self.darwin_loop_change(m, wq, id, &mut e, true).map(|_| ()),
            };
            if let Err(err) = r {
                errors.push(Event { flags: e.flags | ev::ERROR, data: abi::errno_to_darwin(err) as i64, ..e });
            }
        }
        let me = pthread_of(w.stack);
        let (list, ..) = event_areas(me);
        let flags = Self::darwin_events_flags(wq, t);
        if !errors.is_empty() {
            errors.truncate(KEVENT_LIST as usize);
            for (i, e) in errors.iter().enumerate() {
                let _ = e.write(m, list + i as u64 * KEVENT_SIZE, Form::Qos);
            }
            return Some((flags, errors.len() as i64));
        }
        // (A workloop someone has come to own since is theirs to drain.)
        if let Target::Loop(id) = t {
            if wq.events.loops.get(&id).is_none_or(|l| l.owner.is_some()) {
                return None;
            }
        }
        match self.darwin_events_collect(m, wq, w, t) {
            0 => None,
            got => Some((flags, got)),
        }
    }

    /// `kevent_qos` with `KEVENT_FLAG_WORKQ`: changes to the work queue's
    /// kqueue, and, for its servicer, what it has to say now. The answer is
    /// left in `darwin_answer`.
    pub(super) fn darwin_kevent_workq(&mut self, m: &mut GuestMem<'_>, x: [u64; 8]) -> Result<(), Errno> {
        let (changes, nchanges, events, nevents, flags) = (x[1], x[2] as i32, x[3], x[4] as i32, x[7]);
        if nchanges < 0 || nevents < 0 {
            return Err(Errno::EINVAL);
        }
        let shared = Arc::clone(&self.darwin_workq);
        let mut wq = shared.lock().unwrap_or_else(|e| e.into_inner());
        let kq = self.darwin_kqwq(&mut wq.events);
        let mut out = Vec::new();
        for i in 0..nchanges as u64 {
            let e = Event::read(m, changes + i * 72, Form::Qos)?;
            let r = self.darwin_kq_apply(m, &kq, &e);
            if r.is_err() || e.flags & ev::RECEIPT != 0 {
                let code = r.err().map_or(0, |e| abi::errno_to_darwin(e) as i64);
                if (out.len() as i32) < nevents {
                    out.push(Event { flags: e.flags | ev::ERROR, data: code, ..e });
                } else if let Err(e) = r {
                    return Err(e);
                }
            }
        }
        // What there is to say, for the thread that is told of it.
        if flags & kf::ERROR_EVENTS == 0 && out.is_empty() && nevents > 0 && wq.events.kqwq_servicer == Some(self.tid) {
            out = kq.collect(nevents as usize, &Default::default());
            let room = if x[5] != 0 && x[6] != 0 { Some((x[5], m.read_u64(x[6])?)) } else { None };
            if let Some(left) = self.darwin_kevent_ports(m, &mut out, room) {
                m.write_u64(x[6], left)?;
            }
        }
        for fd in kq.take_closed() {
            let _ = self.sys_close(fd);
        }
        for (i, e) in out.iter().enumerate() {
            e.write(m, events + i as u64 * 72, Form::Qos)?;
        }
        self.darwin_answer = Some(Ok(out.len() as u64));
        self.darwin_events_kick(m, &mut wq, &[Target::Kqwq]);
        Ok(())
    }

    /// `kevent_id(id, changes, nchanges, events, nevents, data_out,
    /// data_available, flags)`: changes to workloop `id`, and, for its
    /// servicer, what it has to say now. The answer is left in
    /// `darwin_answer`; or the call waits (a synchronous waiter's note),
    /// as the canonical call returned with its arguments in `a`.
    pub(super) fn darwin_kevent_id(&mut self, m: &mut GuestMem<'_>, x: [u64; 8], a: &mut [u64; 6]) -> u64 {
        match self.darwin_kevent_id_or_wait(m, x, a) {
            Ok(Some(call)) => call,
            Ok(None) => NATIVE + d::KEVENT,
            Err(e) => {
                self.darwin_answer = Some(Err(e));
                NATIVE + d::KEVENT
            }
        }
    }

    fn darwin_kevent_id_or_wait(&mut self, m: &mut GuestMem<'_>, x: [u64; 8], a: &mut [u64; 6]) -> Result<Option<u64>, Errno> {
        let (id, changes, nchanges, events, nevents, flags) = (x[0], x[1], x[2] as i32, x[3], x[4] as i32, x[7]);
        if nchanges < 0 || nevents < 0 {
            return Err(Errno::EINVAL);
        }
        let shared = Arc::clone(&self.darwin_workq);
        let mut wq = shared.lock().unwrap_or_else(|e| e.into_inner());
        // Made again after a wait: where the call had got to, if it is
        // this one.
        let (mut next, mut out) = (0usize, Vec::new());
        if let Some(w) = wq.events.waiters.remove(&self.tid) {
            if w.id == id {
                // Still to be woken: wait on.
                let still = wq.events.loops.get(&id).and_then(|l| l.syncs.get(&w.ident)).is_some_and(|(kn, _)| kn.sfflags & wl::SYNC_WAKE == 0);
                if still {
                    return self.darwin_loop_wait(m, &mut wq, w, a).map(Some);
                }
                if let Some((_, waiter)) = wq.events.loops.get_mut(&id).and_then(|l| l.syncs.get_mut(&w.ident)) {
                    *waiter = None;
                }
                (next, out) = (w.next, w.out);
            }
        }
        if next == 0 {
            let exists = wq.events.loops.contains_key(&id);
            if flags & kf::MUST_EXIST != 0 && !exists {
                return Err(Errno::ENOENT);
            }
            if flags & kf::MUST_NOT_EXIST != 0 && exists {
                return Err(Errno::EEXIST);
            }
        }
        if !wq.events.loops.contains_key(&id) {
            wq.events.loops.insert(id, Workloop::new(self.clock));
        }
        for i in next..nchanges as usize {
            let mut e = Event::read(m, changes + i as u64 * 72, Form::Qos)?;
            let r = if e.filter == WORKLOOP {
                self.darwin_loop_change(m, &mut wq, id, &mut e, false)
            } else {
                let kq = Arc::clone(&wq.events.loops[&id].kq);
                self.darwin_kq_apply(m, &kq, &e).map(|_| false)
            };
            match r {
                // The caller waits on its note, then goes on from the next.
                Ok(true) => {
                    let w = Waiter { id, ident: e.ident, next: i + 1, out };
                    return self.darwin_loop_wait(m, &mut wq, w, a).map(Some);
                }
                Ok(false) if e.flags & ev::RECEIPT == 0 => {}
                r => {
                    let code = r.err().map_or(0, |e| abi::errno_to_darwin(e) as i64);
                    if (out.len() as i32) < nevents {
                        out.push(Event { flags: e.flags | ev::ERROR, data: code, ..e });
                    } else if let Err(e) = r {
                        return Err(e);
                    }
                }
            }
        }
        // What there is to say, for the workloop's servicer.
        let servicer = wq.events.loops.get(&id).is_some_and(|l| l.servicer == Some(self.tid));
        if flags & kf::ERROR_EVENTS == 0 && out.is_empty() && nevents > 0 && servicer {
            let l = wq.events.loops.get_mut(&id).unwrap();
            if l.owner.is_none() {
                if let Some((kn, on)) = &mut l.request {
                    if *on {
                        *on = false;
                        out.push(Event { ident: id, filter: WORKLOOP, flags: kn.flags | ev::CLEAR, fflags: kn.sfflags, data: kn.sdata, udata: kn.udata, qos: kn.qos, ext: kn.ext });
                    }
                }
            }
            let kq = Arc::clone(&l.kq);
            out.extend(kq.collect((nevents as usize).saturating_sub(out.len()), &Default::default()));
            let room = if x[5] != 0 && x[6] != 0 { Some((x[5], m.read_u64(x[6])?)) } else { None };
            if let Some(left) = self.darwin_kevent_ports(m, &mut out, room) {
                m.write_u64(x[6], left)?;
            }
        }
        if let Some(l) = wq.events.loops.get(&id) {
            for fd in l.kq.take_closed() {
                let _ = self.sys_close(fd);
            }
        }
        for (i, e) in out.iter().enumerate() {
            e.write(m, events + i as u64 * 72, Form::Qos)?;
        }
        if wq.events.loops.get(&id).is_some_and(|l| l.idle()) {
            wq.events.loops.remove(&id);
        }
        self.darwin_answer = Some(Ok(out.len() as u64));
        self.darwin_events_kick(m, &mut wq, &[Target::Loop(id)]);
        Ok(None)
    }

    /// Wait on the note a synchronous waiter registered, until it is woken
    /// or gone: the canonical call that sleeps on this thread's word.
    fn darwin_loop_wait(&mut self, m: &mut GuestMem<'_>, wq: &mut Workq, w: Waiter, a: &mut [u64; 6]) -> Result<u64, Errno> {
        if let Some((_, waiter)) = wq.events.loops.get_mut(&w.id).and_then(|l| l.syncs.get_mut(&w.ident)) {
            *waiter = Some(self.tid);
        }
        let at = self.darwin_workq_word(m, wq, self.tid)?;
        let seen = m.read_u32(at)?;
        let id = w.id;
        wq.events.waiters.insert(self.tid, w);
        // (Whatever this call made of the workloop, the threads hear of it
        // now: it may be what wakes this one.)
        let does = self.darwin_workq_does.take();
        self.darwin_events_kick(m, wq, &[Target::Loop(id)]);
        // (A thread to make waits for the next call that can: this one
        // sleeps.)
        if let Some(super::workq::Does::Make(work)) = self.darwin_workq_does.take() {
            wq.pending.push_front(work);
        }
        self.darwin_workq_does = does;
        self.darwin_post = Post::LoopWait;
        *a = [at, crate::futex::op::WAIT as u64, seen as u64, 0, 0, 0];
        Ok(nr::FUTEX)
    }

    /// A synchronous wait ended without being made again (a signal): the
    /// thread's next `kevent_id` is a new one.
    pub(super) fn darwin_loop_wait_ended(&mut self) {
        let shared = Arc::clone(&self.darwin_workq);
        let mut wq = shared.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = wq.events.waiters.remove(&self.tid) {
            if let Some((_, waiter)) = wq.events.loops.get_mut(&w.id).and_then(|l| l.syncs.get_mut(&w.ident)) {
                *waiter = None;
            }
        }
    }

    /// One change `e` of `EVFILT_WORKLOOP` to workloop `id`, as XNU's
    /// `filt_wlattach`, `filt_wltouch` and `filt_wlallow_drop` make it.
    /// `Ok(true)`: the caller is to wait on the note it named. A servicer
    /// coming back (`back`) never waits.
    fn darwin_loop_change(&mut self, m: &mut GuestMem<'_>, wq: &mut Workq, id: u64, e: &mut Event, back: bool) -> Result<bool, Errno> {
        if e.filter != WORKLOOP {
            let kq = wq.events.loops.entry(id).or_insert_with(|| Workloop::new(self.clock)).kq.clone();
            return self.darwin_kq_apply(m, &kq, e).map(|_| false);
        }
        let l = wq.events.loops.entry(id).or_insert_with(|| Workloop::new(self.clock));
        let cmd = e.fflags & wl::COMMANDS;
        let request = e.ident == id;
        // (Delete and disable overrule.)
        let mut flags = e.flags;
        if flags & ev::DELETE != 0 {
            flags &= !ev::ADD;
        }
        if flags & ev::DISABLE != 0 {
            flags &= !ev::ENABLE;
        }
        let saved = if request { l.request.map(|(kn, _)| kn) } else { l.syncs.get(&e.ident).map(|(kn, _)| *kn) };
        let Some(kn) = saved else {
            if flags & ev::ADD == 0 {
                // (Added and deleted at once: a delete that does not mind
                // there being nothing to delete.)
                return if e.flags & (ev::ADD | ev::DELETE) == ev::ADD | ev::DELETE { Ok(false) } else { Err(Errno::ENOENT) };
            }
            // A new note.
            match cmd {
                wl::THREAD_REQUEST => {
                    if !request {
                        return Err(Errno::EINVAL);
                    }
                    if (e.qos >> 8) & 0x3f == 0 {
                        return Err(Errno::ERANGE);
                    }
                }
                wl::SYNC_WAIT | wl::SYNC_WAKE => {
                    if request || flags & ev::DISABLE == 0 || e.fflags & wl::END_OWNERSHIP != 0 {
                        return Err(Errno::EINVAL);
                    }
                }
                _ => return Err(Errno::EINVAL),
            }
            match self.darwin_loop_update(m, wq, id, e) {
                // (Stale, and told not to say so: not added.)
                Err(err) if err == estale() && e.fflags & wl::IGNORE_ESTALE != 0 => return Ok(false),
                Err(err) => return Err(err),
                Ok(()) => {}
            }
            let l = wq.events.loops.get_mut(&id).unwrap();
            let kn = Knote { udata: e.udata, qos: e.qos, flags, sfflags: e.fflags, sdata: e.data, ext: e.ext };
            if cmd == wl::THREAD_REQUEST {
                l.request = Some((kn, true));
                return Ok(false);
            }
            l.syncs.insert(e.ident, (kn, None));
            return Ok(cmd == wl::SYNC_WAIT && !back);
        };
        Self::darwin_loop_valid(kn.sfflags, e, flags)?;
        if flags & ev::DELETE != 0 {
            match self.darwin_loop_update(m, wq, id, e) {
                // (Stale, and told not to say so: not deleted.)
                Err(err) if err == estale() && e.fflags & wl::IGNORE_ESTALE != 0 => return Ok(false),
                Err(err) => return Err(err),
                Ok(()) => {}
            }
            let l = wq.events.loops.get_mut(&id).unwrap();
            if request {
                l.request = None;
            } else if let Some((_, Some(waiter))) = l.syncs.remove(&e.ident) {
                // A waiter whose note goes is woken.
                self.darwin_workq_wake(m, wq, waiter);
            }
            return Ok(false);
        }
        // A note there already, touched.
        match self.darwin_loop_update(m, wq, id, e) {
            Err(err) if err == estale() && e.fflags & wl::IGNORE_ESTALE != 0 => return Ok(false),
            Err(err) => return Err(err),
            Ok(()) => {}
        }
        let l = wq.events.loops.get_mut(&id).unwrap();
        let mut kn = kn;
        kn.sfflags = (kn.sfflags & !wl::UPDATES) | e.fflags;
        kn.sdata = e.data;
        kn.udata = e.udata;
        kn.ext = e.ext;
        if e.fflags & wl::UPDATE_QOS != 0 {
            kn.qos = e.qos;
        }
        if request {
            // Touched, it is to be said again.
            l.request = Some((kn, true));
            return Ok(false);
        }
        let waiter = l.syncs.get(&e.ident).and_then(|(_, w)| *w);
        l.syncs.insert(e.ident, (kn, waiter));
        if e.fflags & wl::SYNC_WAKE != 0 {
            if let Some(t) = waiter {
                self.darwin_workq_wake(m, wq, t);
            }
        }
        Ok(cmd == wl::SYNC_WAIT && kn.sfflags & wl::SYNC_WAKE == 0 && !back)
    }

    /// `filt_wlvalidate_kev_flags`: whether change `e` (with its flags as
    /// simplified) may be made to a note registered with `saved`.
    fn darwin_loop_valid(saved: u32, e: &Event, flags: u16) -> Result<(), Errno> {
        let (new, sav) = (e.fflags & wl::COMMANDS, saved & wl::COMMANDS);
        if e.fflags & wl::DISCOVER_OWNER != 0 && flags & ev::DELETE != 0 {
            return Err(Errno::EINVAL);
        }
        if e.fflags & wl::UPDATE_QOS != 0 {
            if flags & ev::DELETE != 0 || sav != wl::THREAD_REQUEST {
                return Err(Errno::EINVAL);
            }
            if (e.qos >> 8) & 0x3f == 0 {
                return Err(Errno::ERANGE);
            }
        }
        match new {
            wl::THREAD_REQUEST if sav == wl::THREAD_REQUEST => Ok(()),
            wl::SYNC_WAIT if e.fflags & wl::END_OWNERSHIP != 0 => Err(Errno::EINVAL),
            wl::SYNC_WAIT | wl::SYNC_WAKE => {
                if sav & (wl::SYNC_WAIT | wl::SYNC_WAKE) == 0 || flags & (ev::ENABLE | ev::DELETE) == ev::ENABLE {
                    Err(Errno::EINVAL)
                } else {
                    Ok(())
                }
            }
            _ => Err(Errno::EINVAL),
        }
    }

    /// `filt_wlupdate`: the word the change names checked against what it
    /// expects (`ESTALE` if it holds otherwise, its value given back in
    /// `ext[VALUE]`), and the workloop's owner found or let go as it says.
    fn darwin_loop_update(&mut self, m: &mut GuestMem<'_>, wq: &mut Workq, id: u64, e: &mut Event) -> Result<(), Errno> {
        let tid = self.tid;
        let l = wq.events.loops.get_mut(&id).unwrap();
        let tracks = e.fflags & (wl::END_OWNERSHIP | wl::DISCOVER_OWNER) != 0 || l.servicer == Some(tid);
        let cur = if tracks { l.owner } else { None };
        let mut new = cur;
        let mut error = Ok(());
        if e.ext[ADDR] != 0 {
            let word = m.read_u64(e.ext[ADDR]).map_err(|_| Errno::EFAULT)?;
            let expected = e.ext[VALUE];
            e.ext[VALUE] = word;
            if word & e.ext[MASK] != expected & e.ext[MASK] {
                error = Err(estale());
            } else if e.fflags & wl::DISCOVER_OWNER != 0 {
                // The owner's port name, in the low bits but its two lowest
                // (which every name has set).
                let name = word as u32 & !3;
                if name != 0 {
                    new = Some(super::mach::port_thread(self.pid, name | 3).ok_or_else(eownerdead)?);
                }
            }
        }
        if e.fflags & wl::END_OWNERSHIP != 0 && new == Some(tid) {
            new = None;
        }
        if tracks {
            // (A servicer is not its own workloop's owner.)
            if new.is_some() && new == l.servicer {
                new = None;
            }
            if new != cur {
                l.owner = new;
                if self.trace {
                    eprintln!("[gex:{}] workloop {:#x}: owner {:?} -> {:?}", self.pid, id, cur, new);
                }
            }
        }
        error
    }

    /// The kernel's own thread, made again: ask for a thread for each
    /// queue that has something to say and nobody to say it to; then wait
    /// on all of them, and on whatever rings it.
    pub(super) fn darwin_events_poll(&mut self, m: &mut GuestMem<'_>, wq: &mut Workq, a: &mut [u64; 6]) -> u64 {
        // What it waits on besides: put back if the program closed it.
        let pump = match wq.events.pump.clone() {
            Some((fd, pump)) if self.fds.get(fd).is_ok_and(|f| Arc::as_ptr(&f.handle) as *const () == Arc::as_ptr(&pump) as *const ()) => (fd, pump),
            _ => {
                let pump = Arc::new(Pump { rung: AtomicBool::new(false), wq: Arc::downgrade(&self.darwin_workq) });
                let handle: Arc<dyn FileHandle> = pump.clone();
                let file = crate::file::OpenFile::new(handle, "anon_inode:[workq]".into(), 0, true, false);
                let high = (self.fds.limit() as i32 - 12).clamp(0, 236);
                match self.fds.insert_at(Arc::clone(&file), high, true).or_else(|_| self.fds.insert(file, true)) {
                    Ok(fd) => {
                        wq.events.pump = Some((fd as i32, Arc::clone(&pump)));
                        (fd as i32, pump)
                    }
                    Err(e) => {
                        self.darwin_answer = Some(Err(e));
                        return NATIVE + d::WORKQ_KERNRETURN;
                    }
                }
            }
        };
        pump.1.rung.store(false, Ordering::SeqCst);
        let unserved = wq.events.unserved();
        self.darwin_events_kick(m, wq, &unserved);
        if self.darwin_workq_does.is_some() {
            // (A thread to make: made as this call ends, and the call is
            // made again.)
            self.darwin_answer = Some(Ok(0));
            return NATIVE + d::WORKQ_KERNRETURN;
        }
        // Wait: on the pump, on every descriptor a queue nobody serves
        // watches, until the nearest of their timers.
        let mut ask = vec![(pump.0, poll::IN)];
        let mut left: Option<u64> = None;
        for t in wq.events.unserved() {
            if let Some(kq) = wq.events.kq(t) {
                ask.extend(kq.watched());
                if let Some(n) = kq.next_timer() {
                    left = Some(left.map_or(n, |l| l.min(n)));
                }
            }
        }
        let slot = match self.darwin_kevent_slot(m) {
            Ok(slot) => slot,
            Err(e) => {
                self.darwin_answer = Some(Err(e));
                return NATIVE + d::WORKQ_KERNRETURN;
            }
        };
        let fds = super::kqueue::FDS;
        ask.truncate(((abi::PAGE_SIZE - fds) / 8) as usize);
        for (i, (fd, events)) in ask.iter().enumerate() {
            let _ = m.write_u32(slot + fds + 8 * i as u64, *fd as u32);
            let _ = m.write_u32(slot + fds + 8 * i as u64 + 4, *events as u32);
        }
        if let Some(left) = left {
            let left = left.max(1);
            let _ = m.write_u64(slot, left / 1_000_000_000);
            let _ = m.write_u64(slot + 8, left % 1_000_000_000);
        }
        self.darwin_post = Post::Workq;
        *a = [slot + fds, ask.len() as u64, if left.is_some() { slot } else { 0 }, 0, 8, 0];
        nr::PPOLL
    }
}
