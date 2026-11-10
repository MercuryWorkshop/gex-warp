//! `kqueue` and `kevent`: BSD's way of waiting for many kinds of thing at
//! once, which Darwin's libraries are built on (libdispatch's sources, the
//! resolver, `tail -f`).
//!
//! A queue is a file: an object of its own in the descriptor table, with
//! the notes registered on it, each a thing watched (`ident`) through a
//! filter. It has a wait queue chained onto those of the files it watches
//! ([`crate::wait`]), as an `epoll` set has, so whatever can wait on a
//! descriptor can wait on a queue; and it is readable when a note has
//! something to say.
//!
//! `kevent` makes the changes it is given and collects what is ready. With
//! nothing ready and time to wait, the call becomes the kernel's `ppoll`:
//! of the queue's own descriptor, and of every descriptor a note watches,
//! since a host whose processes are apart learns of another's doing by
//! being asked about the descriptor itself (as `epoll_wait` asks). When
//! that ends the call is made again from its beginning
//! ([`Post::Kevent`]): the thread is known to have made its changes
//! (`Kernel::darwin_kevents`), so the second time it only collects, with
//! what the `ppoll` found, or waits out what is left of its time.
//!
//! A note on a process holds a descriptor of the kernel's for it
//! (`pidfd_open`), which the program can see among its own and should
//! leave alone.
//!
//! The filters here are the ones a program meets first: a descriptor
//! readable or writable, a timer, an event a program posts to itself
//! (`EVFILT_USER`), and a process ending. The others (a signal, a Mach
//! port, a file changing, the kernel's work queues delivering events to
//! threads it makes) are refused by name, `ENOTSUP`.
use super::nr as d;
use super::*;
use crate::syscalls::{clock_ns, nr, ClockFn};
use crate::vfs::{FileHandle, FileType, Metadata};
use std::sync::{Mutex, Weak};

/// The filters.
mod filt {
    pub const READ: i16 = -1;
    pub const WRITE: i16 = -2;
    pub const PROC: i16 = -5;
    pub const TIMER: i16 = -7;
    pub const USER: i16 = -10;
}

/// An event's flags: what to do with a note, and what one says.
mod ev {
    pub const ADD: u16 = 0x0001;
    pub const DELETE: u16 = 0x0002;
    pub const ENABLE: u16 = 0x0004;
    pub const DISABLE: u16 = 0x0008;
    pub const ONESHOT: u16 = 0x0010;
    pub const CLEAR: u16 = 0x0020;
    pub const RECEIPT: u16 = 0x0040;
    pub const DISPATCH: u16 = 0x0080;
    pub const ERROR: u16 = 0x4000;
    pub const EOF: u16 = 0x8000;
    /// What a note keeps of the flags it was added with.
    pub const KEPT: u16 = ONESHOT | CLEAR | DISPATCH;
}

/// `fflags`, by filter.
mod note {
    // A timer's unit (milliseconds without one), and a time of day.
    pub const SECONDS: u32 = 0x1;
    pub const USECONDS: u32 = 0x2;
    pub const NSECONDS: u32 = 0x4;
    pub const ABSOLUTE: u32 = 0x8;
    // A user event: post it, and how the flags given combine with the
    // note's.
    pub const TRIGGER: u32 = 0x0100_0000;
    pub const FFLAGS: u32 = 0x00ff_ffff;
    pub const FFCTRL: u32 = 0xc000_0000;
    pub const FFAND: u32 = 0x4000_0000;
    pub const FFOR: u32 = 0x8000_0000;
    pub const FFCOPY: u32 = 0xc000_0000;
    // A process: it ended, and how.
    pub const EXIT: u32 = 0x8000_0000;
    pub const EXITSTATUS: u32 = 0x0400_0000;
}

/// `kevent64`'s and `kevent_qos`'s flags: do not wait; and the two that
/// hand events to the kernel's own threads.
const KEVENT_FLAG_IMMEDIATE: u64 = 0x1;
const KEVENT_FLAG_WORKQ: u64 = 0x20;
const KEVENT_FLAG_WORKLOOP: u64 = 0x100;

/// The three layouts an event is passed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Form {
    /// `struct kevent`, 32 bytes.
    Plain,
    /// `struct kevent64_s`, with two words of the caller's.
    Wide,
    /// `struct kevent_qos_s`, with a quality of service and four.
    Qos,
}

impl Form {
    fn size(self) -> u64 {
        match self {
            Form::Plain => 32,
            Form::Wide => 48,
            Form::Qos => 64,
        }
    }
}

/// One event, in or out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Event {
    pub ident: u64,
    pub filter: i16,
    pub flags: u16,
    pub fflags: u32,
    pub data: i64,
    pub udata: u64,
    pub qos: u32,
    pub ext: [u64; 4],
}

impl Event {
    fn read(m: &GuestMem<'_>, at: u64, form: Form) -> Result<Event, Errno> {
        let b = m.read(at, form.size() as usize)?;
        let w = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        let h = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let mut e = Event { ident: w(0), filter: h(8) as i16, flags: (h(8) >> 16) as u16, ..Event::default() };
        match form {
            Form::Plain | Form::Wide => {
                (e.fflags, e.data, e.udata) = (h(12), w(16) as i64, w(24));
                if form == Form::Wide {
                    e.ext[..2].copy_from_slice(&[w(32), w(40)]);
                }
            }
            Form::Qos => {
                (e.qos, e.udata, e.fflags, e.data) = (h(12), w(16), h(24), w(32) as i64);
                e.ext = [w(40), w(48), w(56), 0];
                e.ext[3] = 0;
            }
        }
        Ok(e)
    }

    fn write(&self, m: &mut GuestMem<'_>, at: u64, form: Form) -> Result<(), Errno> {
        let mut b = [0u8; 64];
        b[0..8].copy_from_slice(&self.ident.to_le_bytes());
        b[8..10].copy_from_slice(&self.filter.to_le_bytes());
        b[10..12].copy_from_slice(&self.flags.to_le_bytes());
        match form {
            Form::Plain | Form::Wide => {
                b[12..16].copy_from_slice(&self.fflags.to_le_bytes());
                b[16..24].copy_from_slice(&self.data.to_le_bytes());
                b[24..32].copy_from_slice(&self.udata.to_le_bytes());
                b[32..40].copy_from_slice(&self.ext[0].to_le_bytes());
                b[40..48].copy_from_slice(&self.ext[1].to_le_bytes());
            }
            Form::Qos => {
                b[12..16].copy_from_slice(&self.qos.to_le_bytes());
                b[16..24].copy_from_slice(&self.udata.to_le_bytes());
                b[24..28].copy_from_slice(&self.fflags.to_le_bytes());
                b[32..40].copy_from_slice(&self.data.to_le_bytes());
                for (i, x) in self.ext.iter().take(3).enumerate() {
                    b[40 + 8 * i..48 + 8 * i].copy_from_slice(&x.to_le_bytes());
                }
            }
        }
        m.write(at, &b[..form.size() as usize])
    }
}

/// What a note watches.
enum What {
    /// An open file, to be read or written: the file (while anything has it
    /// open), the wait queue of its this queue is chained onto, and for a
    /// note that reports each change once (`EV_CLEAR`) how often the file
    /// had woken its waiters when it last reported.
    File { file: Weak<dyn FileHandle>, source: u32, seen: Option<u64> },
    /// A timer: when it next fires, on the monotonic clock in nanoseconds
    /// (or, `absolute`, the wall clock), and every how often after.
    Timer { next: u64, period: u64, absolute: bool },
    /// An event the program posts.
    User { triggered: bool },
    /// A process, to end: the kernel's descriptor for it, which the note
    /// opened and closes, and the file behind that.
    Proc { fd: i32, file: Arc<dyn FileHandle>, source: u32 },
}

/// A registration: a thing, a filter, and what the caller wants back.
struct Note {
    ident: u64,
    filter: i16,
    /// `EV_ONESHOT`, `EV_CLEAR`, `EV_DISPATCH`.
    flags: u16,
    disabled: bool,
    fflags: u32,
    udata: u64,
    qos: u32,
    ext: [u64; 4],
    what: What,
}

/// A file a note is to watch, as the caller's descriptor table has it.
pub struct Watched {
    pub file: Arc<dyn FileHandle>,
}

/// A kqueue.
pub struct Kqueue {
    queue: crate::wait::Queue,
    clock: ClockFn,
    notes: Mutex<Vec<Note>>,
    /// Which of the three calls it is used with: the first to use it says,
    /// and the others are refused after.
    form: Mutex<Option<Form>>,
    /// The descriptors of notes that are gone, for whoever has the table
    /// to close ([`Kqueue::take_closed`]).
    closed: Mutex<Vec<i32>>,
}

/// What a `ppoll` found of the descriptors it was given, by descriptor.
pub type Found = std::collections::HashMap<i32, u16>;

/// `poll`'s events: to read, to write, an error, the other end gone.
mod poll {
    pub const IN: u16 = 0x1;
    pub const OUT: u16 = 0x4;
    pub const ERR: u16 = 0x8;
    pub const HUP: u16 = 0x10;
}

impl std::fmt::Debug for Kqueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kqueue").finish()
    }
}

impl Kqueue {
    pub fn new(clock: ClockFn) -> Kqueue {
        Kqueue { queue: crate::wait::Queue::new(), clock, notes: Mutex::new(Vec::new()), form: Mutex::new(None), closed: Mutex::new(Vec::new()) }
    }

    /// The descriptors notes held that are gone, to be closed.
    pub fn take_closed(&self) -> Vec<i32> {
        std::mem::take(&mut *self.closed.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// A note taken out of the queue: it hears no more, and what it held is
    /// to be closed.
    fn retire(&self, left: &[Note], gone: Note) {
        self.unchain(left, &gone);
        if let What::Proc { fd, .. } = gone.what {
            self.closed.lock().unwrap_or_else(|e| e.into_inner()).push(fd);
        }
    }

    /// The descriptors a wait on this queue is to ask after, with what of
    /// each: the files of the notes that are on, and the processes'.
    pub fn watched(&self) -> Vec<(i32, u16)> {
        let mut out: Vec<(i32, u16)> = Vec::new();
        for n in self.notes().iter().filter(|n| !n.disabled) {
            let (fd, events) = match n.what {
                What::File { .. } => (n.ident as i32, if n.filter == filt::READ { poll::IN } else { poll::OUT }),
                What::Proc { fd, .. } => (fd, poll::IN),
                _ => continue,
            };
            match out.iter_mut().find(|w| w.0 == fd) {
                Some(w) => w.1 |= events,
                None => out.push((fd, events)),
            }
        }
        out
    }

    /// Whether the queue is one for events of this layout: it is for the
    /// first it is used with.
    fn takes(&self, form: Form) -> bool {
        *self.form.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert(form) == form
    }

    fn notes(&self) -> std::sync::MutexGuard<'_, Vec<Note>> {
        self.notes.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The monotonic clock and the wall clock, in nanoseconds.
    fn now(&self) -> (u64, u64) {
        (clock_ns(self.clock, 1), clock_ns(self.clock, 0))
    }

    /// One change: a note added, changed, switched on or off, or removed.
    /// `file` finds an open file by its descriptor, `process` opens a
    /// descriptor for a process to watch, by its id.
    pub fn change(
        &self,
        e: &Event,
        file: &dyn Fn(i32) -> Result<Arc<dyn FileHandle>, Errno>,
        process: &mut dyn FnMut(i32) -> Result<(i32, Arc<dyn FileHandle>), Errno>,
    ) -> Result<(), Errno> {
        let mut notes = self.notes();
        let at = notes.iter().position(|n| n.ident == e.ident && n.filter == e.filter);
        if e.flags & ev::DELETE != 0 {
            let n = notes.remove(at.ok_or(Errno::ENOENT)?);
            self.retire(&notes, n);
            return Ok(());
        }
        let (mono, wall) = self.now();
        let at = match at {
            Some(at) => {
                // A note there already keeps the flags it was added with
                // (told once, or each time), and takes whatever the change
                // has for the caller to be handed back; added again, a
                // timer is set again, and the others take the new `fflags`.
                let n = &mut notes[at];
                n.udata = e.udata;
                if e.flags & ev::ADD != 0 {
                    match &mut n.what {
                        What::Timer { .. } => {
                            n.what = timer(e, mono, wall)?;
                            n.fflags = e.fflags;
                        }
                        What::User { .. } => {}
                        _ => n.fflags = e.fflags,
                    }
                }
                at
            }
            None => {
                if e.flags & ev::ADD == 0 {
                    return Err(Errno::ENOENT);
                }
                let (what, fflags) = match e.filter {
                    filt::READ | filt::WRITE => {
                        let f = file(e.ident as i32)?;
                        let source = f.wait_queue();
                        crate::wait::chain(source, self.queue.id());
                        (What::File { file: Arc::downgrade(&f), source, seen: None }, e.fflags)
                    }
                    filt::TIMER => (timer(e, mono, wall)?, e.fflags),
                    filt::USER => (What::User { triggered: false }, e.fflags & note::FFLAGS),
                    filt::PROC => {
                        let (fd, file) = process(e.ident as i32)?;
                        let source = file.wait_queue();
                        crate::wait::chain(source, self.queue.id());
                        (What::Proc { fd, file, source }, e.fflags)
                    }
                    // A filter Darwin has and this does not yet; or none.
                    -15..=-3 => return Err(Errno::EOPNOTSUPP),
                    _ => return Err(Errno::EINVAL),
                };
                // (A timer tells of each firing once, asked to or not.)
                let flags = e.flags & ev::KEPT | if e.filter == filt::TIMER { ev::CLEAR } else { 0 };
                notes.push(Note { ident: e.ident, filter: e.filter, flags, disabled: false, fflags, udata: e.udata, qos: e.qos, ext: e.ext, what });
                notes.len() - 1
            }
        };
        let n = &mut notes[at];
        if e.flags & ev::DISABLE != 0 {
            n.disabled = true;
        } else if e.flags & ev::ENABLE != 0 {
            n.disabled = false;
        }
        // A user event's flags are combined with the note's as the change
        // says, and the event posted if it says so.
        if let What::User { triggered } = &mut n.what {
            let given = e.fflags & note::FFLAGS;
            match e.fflags & note::FFCTRL {
                note::FFAND => n.fflags &= given,
                note::FFOR => n.fflags |= given,
                note::FFCOPY => n.fflags = given,
                _ => {}
            }
            if e.fflags & note::TRIGGER != 0 {
                *triggered = true;
            }
        }
        drop(notes);
        // Whoever waits looks again: there may be something to say now, or
        // a nearer time to wake at.
        self.queue.notify();
        Ok(())
    }

    /// Stop hearing from `gone`'s source, unless another note still does.
    fn unchain(&self, left: &[Note], gone: &Note) {
        let source = |n: &Note| match n.what {
            What::File { source, .. } | What::Proc { source, .. } => source,
            _ => 0,
        };
        let s = source(gone);
        if s != 0 && !left.iter().any(|n| source(n) == s) {
            crate::wait::unchain(s, self.queue.id());
        }
    }

    /// What `n` has to say now, if anything: the flags, `fflags` and `data`
    /// of its event. `None` too for a note whose file is gone (`dead`).
    /// What a `ppoll` just `found` of a descriptor is believed before the
    /// file is asked.
    fn ready(n: &Note, mono: u64, wall: u64, found: &Found, dead: &mut bool) -> Option<(u16, u32, i64)> {
        match &n.what {
            What::File { file, seen, .. } => {
                let Some(f) = file.upgrade() else {
                    *dead = true;
                    return None;
                };
                let (eof, ready) = match found.get(&(n.ident as i32)) {
                    Some(&got) => (got & (poll::HUP | poll::ERR) != 0, got & if n.filter == filt::READ { poll::IN } else { poll::OUT } != 0),
                    None => (
                        f.poll_hup() || (n.filter == filt::READ && f.poll_rdhup()),
                        if n.filter == filt::READ { f.poll_readable() } else { f.poll_writable() },
                    ),
                };
                if !ready && !eof {
                    return None;
                }
                // Told of each change once: nothing new since the last.
                if n.flags & ev::CLEAR != 0 && seen.is_some() && f.wakes().is_some() && f.wakes() == *seen {
                    return None;
                }
                // How much there is to read; or room to write, which no
                // file here measures: a pipe's worth.
                let data = if n.filter == filt::READ { f.available().unwrap_or(ready as u64) as i64 } else { 16384 };
                Some((if eof { ev::EOF } else { 0 }, 0, data))
            }
            What::Timer { next, period, absolute } => {
                let now = if *absolute { wall } else { mono };
                (now >= *next).then(|| (0, 0, if *period == 0 { 1 } else { 1 + ((now - *next) / *period) as i64 }))
            }
            What::User { triggered } => triggered.then_some((0, n.fflags, 0)),
            What::Proc { fd, file, .. } => {
                let gone = found.get(fd).map_or_else(|| file.poll_readable(), |got| got & (poll::IN | poll::HUP) != 0);
                if n.fflags & note::EXIT == 0 || !gone {
                    return None;
                }
                // Gone, and the note with it. (How it ended is the
                // parent's to collect with `wait4`; no status is kept here
                // to give a second time.)
                Some((ev::EOF | ev::ONESHOT, note::EXIT | (n.fflags & note::EXITSTATUS), 0))
            }
        }
    }

    /// Whether a note has something to say.
    pub fn any_ready(&self) -> bool {
        let (mono, wall) = self.now();
        let nothing = Found::new();
        self.notes().iter().any(|n| !n.disabled && Kqueue::ready(n, mono, wall, &nothing, &mut false).is_some())
    }

    /// Up to `max` events, from the notes that have something to say. A
    /// note that said its one thing is removed (`EV_ONESHOT`), or switched
    /// off (`EV_DISPATCH`), or starts over (`EV_CLEAR`); a timer is set for
    /// its next time.
    pub fn collect(&self, max: usize, found: &Found) -> Vec<Event> {
        let (mono, wall) = self.now();
        let mut notes = self.notes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < notes.len() && out.len() < max {
            let mut dead = false;
            let said = if notes[i].disabled { None } else { Kqueue::ready(&notes[i], mono, wall, found, &mut dead) };
            let Some((flags, fflags, data)) = said else {
                if dead {
                    let n = notes.remove(i);
                    self.retire(&notes, n);
                } else {
                    i += 1;
                }
                continue;
            };
            let n = &mut notes[i];
            out.push(Event { ident: n.ident, filter: n.filter, flags: ev::ADD | n.flags | flags, fflags, data, udata: n.udata, qos: n.qos, ext: n.ext });
            match &mut n.what {
                What::File { file, seen, .. } => *seen = file.upgrade().and_then(|f| f.wakes()),
                What::Timer { next, period, .. } => {
                    if *period == 0 {
                        n.flags |= ev::ONESHOT;
                    } else {
                        *next += *period * data as u64;
                    }
                }
                // (Posted no longer; its flags stay as they were made.)
                What::User { triggered } => {
                    if n.flags & ev::CLEAR != 0 {
                        *triggered = false;
                    }
                }
                What::Proc { .. } => {}
            }
            if (n.flags | flags) & ev::ONESHOT != 0 {
                let n = notes.remove(i);
                self.retire(&notes, n);
                continue;
            }
            if n.flags & ev::DISPATCH != 0 {
                n.disabled = true;
            }
            i += 1;
        }
        out
    }

    /// How long until the nearest timer fires, in nanoseconds.
    pub fn next_timer(&self) -> Option<u64> {
        let (mono, wall) = self.now();
        self.notes()
            .iter()
            .filter(|n| !n.disabled)
            .filter_map(|n| match n.what {
                What::Timer { next, absolute, .. } => Some(next.saturating_sub(if absolute { wall } else { mono })),
                _ => None,
            })
            .min()
    }
}

/// A timer note for `e`: its `data` in the unit its `fflags` name, a time
/// of day if they say so and otherwise how long from now, and again every
/// so long unless it is to fire once.
fn timer(e: &Event, mono: u64, wall: u64) -> Result<What, Errno> {
    if e.data < 0 {
        return Err(Errno::EINVAL);
    }
    let unit: u64 = match e.fflags & (note::SECONDS | note::USECONDS | note::NSECONDS) {
        0 => 1_000_000,
        note::SECONDS => 1_000_000_000,
        note::USECONDS => 1_000,
        note::NSECONDS => 1,
        _ => return Err(Errno::EINVAL),
    };
    let span = (e.data as u64).saturating_mul(unit);
    if e.fflags & note::ABSOLUTE != 0 {
        return Ok(What::Timer { next: span, period: 0, absolute: true });
    }
    let _ = wall;
    // (A period of nothing would fire without end: the least there is.)
    let period = if e.flags & ev::ONESHOT != 0 { 0 } else { span.max(1) };
    Ok(What::Timer { next: mono.saturating_add(span), period, absolute: false })
}

impl FileHandle for Kqueue {
    fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> SysResult {
        Err(Errno::ENXIO)
    }
    fn write_at(&self, _offset: u64, _buf: &[u8]) -> SysResult {
        Err(Errno::ENXIO)
    }
    fn metadata(&self) -> Result<Metadata, Errno> {
        // (A fifo, as the kernel's other objects with no file behind them
        // call themselves.)
        Ok(Metadata { file_type: FileType::Fifo, size: 0, mode: 0o600, ino: 0, mtime: 0, nlink: 1, rdev: 0, times: Default::default() })
    }
    fn poll_readable(&self) -> bool {
        self.any_ready()
    }
    fn poll_writable(&self) -> bool {
        false
    }
    fn wait_queue(&self) -> u32 {
        self.queue.id()
    }
    fn as_kqueue(self: Arc<Self>) -> Option<Arc<Kqueue>> {
        Some(self)
    }
}

/// A thread in `kevent` that has made its changes and found nothing yet.
#[derive(Clone, Copy, Debug)]
pub struct Waiting {
    /// When its time is up, on the monotonic clock; `None`: never.
    deadline: Option<u64>,
}

/// What marks a thread's page while it waits ([`Kernel::darwin_kevent`]).
const WAITING: u32 = 0x6b71_7765;

/// A waiting thread's page, which its `ppoll` is given its arguments in:
/// the time to wait, the mark, how many descriptors, and each (`struct
/// pollfd`) from `FDS` on.
const MARK: u64 = 16;
const COUNT: u64 = 20;
const FDS: u64 = 24;

impl Kernel {
    /// `kqueue`: a new queue, on a descriptor.
    pub(super) fn darwin_kqueue(&mut self) -> SysResult {
        let handle: Arc<dyn FileHandle> = Arc::new(Kqueue::new(self.clock));
        let file = crate::file::OpenFile::new(handle, "anon_inode:[kqueue]".into(), 0, true, true);
        // (A queue is not passed on to a program run in this one's place.)
        self.fds.insert(file, true)
    }

    /// Where this thread's page is, made at its first need.
    fn darwin_kevent_slot(&mut self, m: &mut GuestMem<'_>) -> Result<u64, Errno> {
        if let Some(&at) = self.darwin_kevent_pages.get(&self.tid) {
            if self.mem.lock().vma_at(at).is_some() {
                return Ok(at);
            }
        }
        let at = self.mem.mmap_anon(m, 0, abi::PAGE_SIZE, 3, 0, false, "[kqueue]")?;
        self.darwin_kevent_pages.insert(self.tid, at);
        Ok(at)
    }

    /// `kevent`, `kevent64` or `kevent_qos`. The answer is left for
    /// [`Kernel::darwin_native`] (`darwin_answer`); or the call waits, as
    /// the canonical call that is returned with its arguments in `a`.
    pub(super) fn darwin_kevent(&mut self, m: &mut GuestMem<'_>, form: Form, x: [u64; 8], a: &mut [u64; 6]) -> u64 {
        let r = self.darwin_kevent_or_wait(m, form, x, a);
        match r {
            Ok(Some(call)) => call,
            Ok(None) => NATIVE + d::KEVENT,
            Err(e) => {
                self.darwin_kevents.remove(&self.tid);
                self.darwin_answer = Some(Err(e));
                NATIVE + d::KEVENT
            }
        }
    }

    fn darwin_kevent_or_wait(&mut self, m: &mut GuestMem<'_>, form: Form, x: [u64; 8], a: &mut [u64; 6]) -> Result<Option<u64>, Errno> {
        let (fd, changes, nchanges, events, nevents) = (x[0] as i32, x[1], x[2] as i32, x[3], x[4] as i32);
        if nchanges < 0 || nevents < 0 {
            return Err(Errno::EINVAL);
        }
        // How long to wait: a `timespec`, or none of one for ever; the
        // wider calls can say not at all, and `kevent_qos` says nothing
        // else. Its other flags ask for the kernel's own threads.
        let flags = match form {
            Form::Plain => 0,
            Form::Wide => x[5],
            Form::Qos => x[7],
        };
        if flags & (KEVENT_FLAG_WORKQ | KEVENT_FLAG_WORKLOOP) != 0 {
            return Err(Errno::EOPNOTSUPP);
        }
        let timeout = match form {
            Form::Plain => x[5],
            Form::Wide => x[6],
            Form::Qos => 0,
        };
        let kq = Arc::clone(&self.fds.get(fd)?.handle).as_kqueue().ok_or(Errno::EBADF)?;
        if !kq.takes(form) {
            return Err(Errno::EINVAL);
        }
        let slot = self.darwin_kevent_slot(m)?;
        let now = clock_ns(self.clock, 1);
        // Made again after its wait ([`Post::Kevent`]): the changes are
        // made, and its time has been running.
        let waiting = self.darwin_kevents.remove(&self.tid).filter(|_| m.read_u32(slot + MARK) == Ok(WAITING));
        m.write_u32(slot + MARK, 0)?;
        let mut out = Vec::new();
        let mut found = Found::new();
        let deadline = match waiting {
            Some(w) => {
                // What the wait found, of each descriptor it asked after.
                for i in 0..(m.read_u32(slot + COUNT)? as u64).min((abi::PAGE_SIZE - FDS) / 8) {
                    let (fd, got) = (m.read_u32(slot + FDS + 8 * i)? as i32, (m.read_u32(slot + FDS + 8 * i + 4)? >> 16) as u16);
                    if got != 0 && fd != x[0] as i32 {
                        found.insert(fd, got);
                    }
                }
                w.deadline
            }
            None => {
                for i in 0..nchanges as u64 {
                    let e = Event::read(m, changes + i * form.size(), form)?;
                    // (A process to watch is opened first, while the kernel
                    // is this call's to use: the note takes the descriptor
                    // if it is a new one, and it is closed again if not.)
                    let mut opened = None;
                    if e.filter == filt::PROC && e.flags & ev::ADD != 0 {
                        opened = Some(self.dispatch(m, nr::PIDFD_OPEN, [e.ident, 0, 0, 0, 0, 0]).unwrap_or(Err(Errno::ESRCH)).map(|fd| fd as i32));
                    }
                    let fds = &self.fds;
                    let mut taken = false;
                    let r = kq.change(&e, &|fd| fds.get(fd).map(|f| Arc::clone(&f.handle)), &mut |_| {
                        let fd = opened.ok_or(Errno::ESRCH)??;
                        taken = true;
                        Ok((fd, Arc::clone(&fds.get(fd)?.handle)))
                    });
                    if let (Some(Ok(fd)), false) = (opened, taken) {
                        let _ = self.sys_close(fd);
                    }
                    // A change that failed, or any that asks to be told
                    // how it went, is answered with an event of its own;
                    // with no room for it the call fails as the change did.
                    if r.is_err() || e.flags & ev::RECEIPT != 0 {
                        let code = r.err().map_or(0, |e| abi::errno_to_darwin(e) as i64);
                        if (out.len() as i32) < nevents {
                            out.push(Event { flags: e.flags | ev::ERROR, data: code, ..e });
                        } else if let Err(e) = r {
                            return Err(e);
                        }
                    }
                }
                if flags & KEVENT_FLAG_IMMEDIATE != 0 {
                    Some(now)
                } else if timeout == 0 {
                    None
                } else {
                    let (s, ns) = (m.read_u64(timeout)? as i64, m.read_u64(timeout + 8)?);
                    if s < 0 || ns >= 1_000_000_000 {
                        return Err(Errno::EINVAL);
                    }
                    Some(now.saturating_add((s as u64).saturating_mul(1_000_000_000).saturating_add(ns)))
                }
            }
        };
        // What the changes had to say is the answer by itself; otherwise
        // what is ready.
        if out.is_empty() && nevents > 0 {
            out = kq.collect(nevents as usize, &found);
        }
        for fd in kq.take_closed() {
            let _ = self.sys_close(fd);
        }
        let expired = deadline.is_some_and(|d| d <= now);
        if !out.is_empty() || nevents == 0 || expired {
            for (i, e) in out.iter().enumerate() {
                e.write(m, events + i as u64 * form.size(), form)?;
            }
            self.darwin_answer = Some(Ok(out.len() as u64));
            return Ok(None);
        }
        // Nothing yet: wait on the queue itself (a change, an event
        // posted) and on what its notes watch, until its time or the
        // nearest timer's, and then begin again.
        let left = [deadline.map(|d| d - now), kq.next_timer()].into_iter().flatten().min();
        let mut ask = vec![(fd, poll::IN)];
        ask.extend(kq.watched());
        ask.truncate(((abi::PAGE_SIZE - FDS) / 8) as usize);
        for (i, (fd, events)) in ask.iter().enumerate() {
            m.write_u32(slot + FDS + 8 * i as u64, *fd as u32)?;
            m.write_u32(slot + FDS + 8 * i as u64 + 4, *events as u32)?;
        }
        m.write_u32(slot + COUNT, ask.len() as u32)?;
        if let Some(left) = left {
            m.write_u64(slot, left / 1_000_000_000)?;
            m.write_u64(slot + 8, left % 1_000_000_000)?;
        }
        m.write_u32(slot + MARK, WAITING)?;
        self.darwin_kevents.insert(self.tid, Waiting { deadline });
        self.darwin_post = Post::Kevent { slot };
        *a = [slot + FDS, ask.len() as u64, if left.is_some() { slot } else { 0 }, 0, 8, 0];
        Ok(Some(nr::PPOLL))
    }

    /// Whether the call whose wait just ended as `r` is one to be made
    /// again ([`Kernel::finish_blocking`]): a `kevent` is, unless a signal
    /// ended its wait, and a work queue's thread that waited for work
    /// ([`super::workq`]) always. What was left to do after it is then
    /// done with.
    pub(crate) fn darwin_again(&mut self, r: SysResult) -> bool {
        let again = match self.darwin_post {
            Post::Kevent { .. } => r != Err(Errno::EINTR),
            Post::Workq => true,
            _ => false,
        };
        if again {
            self.darwin_post = Post::None;
        }
        again
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock() -> (u64, u32) {
        (1000, 0)
    }
    fn none(_: i32) -> Result<Arc<dyn FileHandle>, Errno> {
        Err(Errno::EBADF)
    }
    fn nobody(_: i32) -> Result<(i32, Arc<dyn FileHandle>), Errno> {
        Err(Errno::ESRCH)
    }
    fn user(ident: u64, flags: u16, fflags: u32) -> Event {
        Event { ident, filter: filt::USER, flags, fflags, udata: 7, ..Event::default() }
    }

    #[test]
    fn a_user_event_is_said_when_posted_and_once_if_cleared() {
        let kq = Kqueue::new(clock);
        kq.change(&user(1, ev::ADD | ev::CLEAR, 0), &none, &mut nobody).unwrap();
        assert!(kq.collect(8, &Found::new()).is_empty());
        kq.change(&user(1, 0, note::TRIGGER | note::FFOR | 5), &none, &mut nobody).unwrap();
        let got = kq.collect(8, &Found::new());
        assert_eq!((got.len(), got[0].ident, got[0].fflags, got[0].udata), (1, 1, 5, 7));
        assert!(kq.collect(8, &Found::new()).is_empty());
    }

    #[test]
    fn a_note_that_is_not_there_cannot_be_changed() {
        let kq = Kqueue::new(clock);
        assert_eq!(kq.change(&user(1, ev::DELETE, 0), &none, &mut nobody), Err(Errno::ENOENT));
        assert_eq!(kq.change(&user(1, ev::ENABLE, 0), &none, &mut nobody), Err(Errno::ENOENT));
        assert_eq!(kq.change(&Event { filter: filt::READ, flags: ev::ADD, ..Event::default() }, &none, &mut nobody), Err(Errno::EBADF));
        assert_eq!(kq.change(&Event { filter: -6, flags: ev::ADD, ..Event::default() }, &none, &mut nobody), Err(Errno::EOPNOTSUPP));
        assert_eq!(kq.change(&Event { filter: 3, flags: ev::ADD, ..Event::default() }, &none, &mut nobody), Err(Errno::EINVAL));
    }

    #[test]
    fn once_is_once_and_dispatch_waits_to_be_switched_on() {
        let kq = Kqueue::new(clock);
        kq.change(&user(1, ev::ADD | ev::ONESHOT, note::TRIGGER), &none, &mut nobody).unwrap();
        kq.change(&user(2, ev::ADD | ev::DISPATCH, note::TRIGGER), &none, &mut nobody).unwrap();
        assert_eq!(kq.collect(8, &Found::new()).len(), 2);
        assert!(kq.collect(8, &Found::new()).is_empty());
        assert_eq!(kq.change(&user(1, ev::DELETE, 0), &none, &mut nobody), Err(Errno::ENOENT));
        kq.change(&user(2, ev::ENABLE, 0), &none, &mut nobody).unwrap();
        assert_eq!(kq.collect(8, &Found::new()).len(), 1);
    }
}
