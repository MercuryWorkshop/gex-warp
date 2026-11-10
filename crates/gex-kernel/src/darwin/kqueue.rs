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
//! (`EVFILT_USER`), a process ending, a signal raised for the process
//! (counted, whatever the process does with it; one sent to a single
//! thread is not), a file or directory changing (`EVFILT_VNODE`), and a
//! Mach port or a set of them with a message to receive
//! (`EVFILT_MACHPORT`). The kernel's work queues delivering events to
//! threads it makes are not here, and are refused by name, `ENOTSUP`.
//!
//! A port's note says whose message waits, or, asked to (`MACH_RCV_MSG`
//! in its `fflags`), receives it: into the buffer the note was given, or
//! one cut from what `kevent_qos` was handed for the purpose, the event
//! saying where and how much. The receiving is the kernel's
//! ([`Kernel::darwin_kevent`]), which has the task's memory and its
//! names; the queue only knows that there is something.
//!
//! A file's changes are heard as the kernel's `inotify` hears them
//! ([`crate::inotify`]): a queue with such a note holds one instance, on a
//! descriptor of the kernel's that the program can see among its own (put
//! high, out of the way), and each note is a watch of it. What that says is
//! respelled: `IN_MODIFY` is `NOTE_WRITE`, with `NOTE_EXTEND` if the file
//! grew, and is `NOTE_ATTRIB` if it shrank, which is how XNU tells of a
//! truncation (a file made longer by `ftruncate` is taken for one written
//! to); `IN_ATTRIB` is `NOTE_ATTRIB`, or `NOTE_LINK` with a name more, or
//! that and `NOTE_DELETE` with one fewer; a directory's entries coming and
//! going are `NOTE_WRITE` to it, with `NOTE_LINK` when they are
//! directories.
use super::nr as d;
use super::*;
use crate::syscalls::{clock_ns, nr, ClockFn};
use crate::vfs::{FileHandle, FileType, Metadata};
use std::sync::{Mutex, Weak};

/// The filters.
mod filt {
    pub const READ: i16 = -1;
    pub const WRITE: i16 = -2;
    pub const VNODE: i16 = -4;
    pub const PROC: i16 = -5;
    pub const SIGNAL: i16 = -6;
    pub const MACHPORT: i16 = -8;
    pub const TIMER: i16 = -7;
    pub const FS: i16 = -9;
    pub const USER: i16 = -10;
    pub const SOCK: i16 = -13;
    pub const MEMORYSTATUS: i16 = -14;
    pub const EXCEPT: i16 = -15;
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
    /// The note is one of several on the same thing, told apart by what
    /// each hands back (`udata`).
    pub const UDATA_SPECIFIC: u16 = 0x0100;
    pub const ERROR: u16 = 0x4000;
    pub const EOF: u16 = 0x8000;
    /// What a note keeps of the flags it was added with.
    pub const KEPT: u16 = ONESHOT | CLEAR | DISPATCH | UDATA_SPECIFIC;
    /// A note of the kind libdispatch makes (`EV_DISPATCH2`): told once
    /// and then off, and one of several told apart. Such a note is not
    /// removed while it is off: it goes when it is next put on, and says
    /// so then.
    pub const DISPATCH2: u16 = DISPATCH | UDATA_SPECIFIC;
}

/// `fflags`, by filter.
mod note {
    // A timer's unit (milliseconds without one; `MACHTIME` is
    // `mach_absolute_time`'s, which here are nanoseconds), and a time
    // that is a moment and no span: of day, or on that clock.
    pub const SECONDS: u32 = 0x1;
    pub const USECONDS: u32 = 0x2;
    pub const NSECONDS: u32 = 0x4;
    pub const ABSOLUTE: u32 = 0x8;
    pub const MACHTIME: u32 = 0x100;
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
    // A file: what happened to it.
    pub const DELETE: u32 = 0x1;
    pub const WRITE: u32 = 0x2;
    pub const EXTEND: u32 = 0x4;
    pub const ATTRIB: u32 = 0x8;
    pub const LINK: u32 = 0x10;
    pub const RENAME: u32 = 0x20;
}

/// `kevent64`'s and `kevent_qos`'s flags: do not wait; say only what went
/// wrong with the changes; and the two that hand events to the kernel's
/// own threads.
const KEVENT_FLAG_IMMEDIATE: u64 = 0x1;
const KEVENT_FLAG_ERROR_EVENTS: u64 = 0x2;
const KEVENT_FLAG_WORKQ: u64 = 0x20;
const KEVENT_FLAG_WORKLOOP: u64 = 0x100;

/// The three layouts an event is passed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Form {
    /// `struct kevent`, 32 bytes.
    Plain,
    /// `struct kevent64_s`, with two words of the caller's.
    Wide,
    /// `struct kevent_qos_s`, 72 bytes, with a quality of service and
    /// four.
    Qos,
}

impl Form {
    fn size(self) -> u64 {
        match self {
            Form::Plain => 32,
            Form::Wide => 48,
            Form::Qos => 72,
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
                e.ext = [w(40), w(48), w(56), w(64)];
            }
        }
        Ok(e)
    }

    fn write(&self, m: &mut GuestMem<'_>, at: u64, form: Form) -> Result<(), Errno> {
        let mut b = [0u8; 72];
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
                for (i, x) in self.ext.iter().enumerate() {
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
    File { file: Weak<crate::file::OpenFile>, source: u32, seen: Option<u64> },
    /// A timer: when it next fires, on the monotonic clock in nanoseconds
    /// (or, `absolute`, the wall clock), and every how often after.
    Timer { next: u64, period: u64, absolute: bool },
    /// An event the program posts.
    User { triggered: bool },
    /// A process, to end: the kernel's descriptor for it, which the note
    /// opened and closes, and the file behind that.
    Proc { fd: i32, file: Arc<dyn FileHandle>, source: u32 },
    /// A signal (the kernel's number for it) raised for the process: the
    /// process's signals, and how many of this one had been raised when
    /// the note last said.
    Signal { of: Arc<crate::signal::ProcSig>, signal: u32, seen: u32 },
    /// A file or directory, to change: the file (while anything has it
    /// open), its watch in the queue's `inotify` instance, how long it was
    /// and how many names it had when last looked at, and what has
    /// happened to it that is yet to be said.
    Vnode { file: Weak<crate::file::OpenFile>, wd: i32, dir: bool, size: u64, nlink: u64, pending: u32 },
    /// Something XNU would tell of that does not happen here: memory
    /// running short, a filesystem mounted.
    Never,
    /// A Mach port, or a set of them, with a message to receive: the
    /// task's names, and the one it is.
    Port { space: Arc<Mutex<super::port::Space>>, name: u32 },
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
    /// To be removed when it is next put on, with an event that says so:
    /// it was removed while off, or said its one thing, and is of the kind
    /// that waits (`ev::DISPATCH2`).
    going: bool,
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
    /// What hears the changes to the files its notes watch, and the
    /// descriptor it is on.
    watcher: Mutex<Option<(i32, Arc<crate::inotify::Inotify>)>>,
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
        Kqueue { queue: crate::wait::Queue::new(), clock, notes: Mutex::new(Vec::new()), form: Mutex::new(None), closed: Mutex::new(Vec::new()), watcher: Mutex::new(None) }
    }

    /// What hears the changes to files for this queue, and its descriptor.
    pub fn watcher(&self) -> Option<(i32, Arc<crate::inotify::Inotify>)> {
        self.watcher.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// It is this from now on (or, the same one, on this descriptor).
    pub fn set_watcher(&self, fd: i32, inst: Arc<crate::inotify::Inotify>) {
        crate::wait::chain(inst.wait_queue(), self.queue.id());
        *self.watcher.lock().unwrap_or_else(|e| e.into_inner()) = Some((fd, inst));
    }

    /// Every descriptor of the kernel's that the queue holds: its notes'
    /// and its watcher's. (What a child process, which has no queue of its
    /// parent's, has no use for either.)
    pub fn holds(&self) -> Vec<i32> {
        let mut out: Vec<i32> = self.notes().iter().filter_map(|n| if let What::Proc { fd, .. } = n.what { Some(fd) } else { None }).collect();
        out.extend(self.watcher().map(|w| w.0));
        out
    }

    /// The queue is closed: its notes are gone, and what it held is
    /// returned to be closed.
    pub fn shut(&self) -> Vec<i32> {
        let mut notes = self.notes();
        while let Some(n) = notes.pop() {
            self.retire(&notes, n);
        }
        drop(notes);
        let mut out = self.take_closed();
        if let Some((fd, inst)) = self.watcher.lock().unwrap_or_else(|e| e.into_inner()).take() {
            crate::wait::unchain(inst.wait_queue(), self.queue.id());
            out.push(fd);
        }
        out
    }

    /// The descriptors notes held that are gone, to be closed.
    pub fn take_closed(&self) -> Vec<i32> {
        std::mem::take(&mut *self.closed.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// A note taken out of the queue: it hears no more, and what it held is
    /// to be closed.
    fn retire(&self, left: &[Note], gone: Note) {
        self.unchain(left, &gone);
        match gone.what {
            What::Proc { fd, .. } => self.closed.lock().unwrap_or_else(|e| e.into_inner()).push(fd),
            What::Port { space, name } => space.lock().unwrap_or_else(|e| e.into_inner()).unwatch(name, self.queue.id()),
            // (A watch is of a file, and may be more than one note's.)
            What::Vnode { wd, .. } => {
                if !left.iter().any(|n| matches!(n.what, What::Vnode { wd: other, .. } if other == wd)) {
                    if let Some((_, inst)) = self.watcher() {
                        let _ = inst.rm_watch(wd);
                    }
                }
            }
            _ => {}
        }
    }

    /// What the watcher has heard since it was last asked, to the notes it
    /// is about.
    fn hear(&self, notes: &mut [Note]) {
        use crate::inotify::mask as im;
        if !notes.iter().any(|n| matches!(n.what, What::Vnode { .. })) {
            return;
        }
        let Some((_, inst)) = self.watcher() else { return };
        let mut buf = [0u8; 4096];
        while let Ok(got) = inst.read_at(0, &mut buf) {
            let mut at = 0;
            while at + 16 <= got as usize {
                let w = |o: usize| u32::from_le_bytes(buf[at + o..at + o + 4].try_into().unwrap());
                let (wd, mask, named) = (w(0) as i32, w(4), w(12) != 0);
                at += 16 + w(12) as usize;
                for n in notes.iter_mut() {
                    let interest = n.fflags;
                    let What::Vnode { file, wd: own, dir, size, nlink, pending } = &mut n.what else { continue };
                    if *own != wd {
                        continue;
                    }
                    // What happened, each thing as XNU says it: all of one
                    // is said to a note that asked for any of it.
                    let mut said = [0u32; 5];
                    if named {
                        // An entry of the directory came or went (and
                        // nothing is said of what happens inside one).
                        if *dir && mask & (im::CREATE | im::DELETE | im::MOVED_FROM | im::MOVED_TO) != 0 {
                            said[0] = note::WRITE | if mask & im::ISDIR != 0 { note::LINK } else { 0 };
                        }
                    } else {
                        let md = file.upgrade().and_then(|f| f.handle.metadata().ok());
                        if mask & im::MODIFY != 0 {
                            let now = md.as_ref().map_or(*size, |md| md.size);
                            (said[0], said[1]) = match now.cmp(size) {
                                std::cmp::Ordering::Less => (note::ATTRIB, 0),
                                std::cmp::Ordering::Equal => (note::WRITE, 0),
                                std::cmp::Ordering::Greater => (note::WRITE, note::EXTEND),
                            };
                            *size = now;
                        }
                        if mask & im::ATTRIB != 0 {
                            let now = md.as_ref().map_or(*nlink, |md| md.nlink as u64);
                            said[2] = match now.cmp(nlink) {
                                std::cmp::Ordering::Less => note::DELETE | note::LINK,
                                std::cmp::Ordering::Equal => note::ATTRIB,
                                std::cmp::Ordering::Greater => note::LINK,
                            };
                            *nlink = now;
                        }
                        if mask & im::MOVE_SELF != 0 {
                            said[3] = note::RENAME;
                        }
                        if mask & im::DELETE_SELF != 0 {
                            said[4] = note::DELETE | if *dir { note::LINK } else { 0 };
                        }
                    }
                    *pending |= said.iter().filter(|s| *s & interest != 0).fold(0, |all, s| all | s);
                }
            }
            if got == 0 {
                break;
            }
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
                What::Vnode { .. } => match self.watcher() {
                    Some((fd, _)) => (fd, poll::IN),
                    None => continue,
                },
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
    /// descriptor for a process to watch, by its id, `watch` is the watch
    /// the queue's watcher has on a descriptor's file, `ports` the task's
    /// names if the one given is a port's to receive from; `signals` are
    /// the calling process's.
    pub fn change(
        &self,
        e: &Event,
        file: &dyn Fn(i32) -> Result<Arc<crate::file::OpenFile>, Errno>,
        process: &mut dyn FnMut(i32) -> Result<(i32, Arc<dyn FileHandle>), Errno>,
        watch: &dyn Fn(i32) -> Result<i32, Errno>,
        ports: &dyn Fn(u32) -> Result<Arc<Mutex<super::port::Space>>, Errno>,
        signals: &Arc<crate::signal::ProcSig>,
    ) -> Result<(), Errno> {
        let mut notes = self.notes();
        // (A note told apart by what it hands back is one only for a
        // change that says so, with the same.)
        let specific = e.flags & ev::UDATA_SPECIFIC;
        let at = notes.iter().position(|n| n.ident == e.ident && n.filter == e.filter && n.flags & ev::UDATA_SPECIFIC == specific && (specific == 0 || n.udata == e.udata));
        if e.flags & ev::DELETE != 0 {
            let at = at.ok_or(Errno::ENOENT)?;
            // (XNU's deferred delete: the note is on its way out, and the
            // caller is told it is not out yet.)
            if e.flags & ev::ENABLE == 0 && notes[at].flags & ev::DISPATCH2 == ev::DISPATCH2 && notes[at].disabled {
                notes[at].going = true;
                return Err(Errno::EINPROGRESS);
            }
            let n = notes.remove(at);
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
                // (A port's note takes what any change says of how and
                // where it is to receive.)
                if let What::Port { .. } = n.what {
                    n.fflags = e.fflags;
                    n.ext = e.ext;
                }
                if e.flags & ev::ADD != 0 {
                    match &mut n.what {
                        What::Timer { .. } => {
                            n.what = timer(e, mono, wall)?;
                            n.fflags = e.fflags;
                        }
                        What::User { .. } => {}
                        What::Port { .. } => {}
                        // (Watched anew: a watch ends with its file's
                        // last name.)
                        What::Vnode { wd, .. } => {
                            if let Ok(now) = watch(e.ident as i32) {
                                *wd = now;
                            }
                            n.fflags = e.fflags;
                        }
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
                        let source = f.handle.wait_queue();
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
                    // A signal, by Darwin's number for it: each raising
                    // from now on, whatever the process does with it.
                    filt::SIGNAL => {
                        // (A number that is no signal's may be watched,
                        // and is never raised.)
                        let signal = if e.ident > abi::NSIG as u64 { 0 } else { abi::sig_to_linux(e.ident as u32) };
                        crate::wait::chain(signals.raised_queue(), self.queue.id());
                        (What::Signal { of: Arc::clone(signals), signal, seen: signals.raised(signal) }, e.fflags)
                    }
                    filt::VNODE => {
                        let f = file(e.ident as i32)?;
                        let wd = watch(e.ident as i32)?;
                        let md = f.handle.metadata()?;
                        let dir = md.file_type == FileType::Directory;
                        (What::Vnode { file: Arc::downgrade(&f), wd, dir, size: md.size, nlink: md.nlink as u64, pending: 0 }, e.fflags)
                    }
                    filt::MACHPORT => {
                        let space = ports(e.ident as u32)?;
                        space.lock().unwrap_or_else(|e| e.into_inner()).watch(e.ident as u32, self.queue.id());
                        (What::Port { space, name: e.ident as u32 }, e.fflags)
                    }
                    filt::FS | filt::MEMORYSTATUS => (What::Never, e.fflags),
                    // (What a socket alone can be asked.)
                    filt::SOCK | filt::EXCEPT => {
                        if file(e.ident as i32)?.handle.metadata()?.file_type != FileType::Socket {
                            return Err(Errno::EINVAL);
                        }
                        (What::Never, e.fflags)
                    }
                    // (Asynchronous I/O's are the kernel's own to add.)
                    -3 => return Err(Errno::EPERM),
                    // A filter Darwin has and this does not yet; or none.
                    -15..=-3 => return Err(Errno::EOPNOTSUPP),
                    _ => return Err(Errno::EINVAL),
                };
                // (A timer tells of each firing once, asked to or not, as a
                // signal's note does of each raising and a process's of
                // its ending; and a timer for a moment fires once.)
                let flags = e.flags & ev::KEPT
                    | if matches!(e.filter, filt::TIMER | filt::SIGNAL | filt::PROC) { ev::CLEAR } else { 0 }
                    | if matches!(what, What::Timer { period: 0, .. }) { ev::ONESHOT } else { 0 };
                notes.push(Note { ident: e.ident, filter: e.filter, flags, disabled: false, fflags, udata: e.udata, qos: e.qos, ext: e.ext, what, going: false });
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
        let source = |n: &Note| match &n.what {
            What::File { source, .. } | What::Proc { source, .. } => *source,
            What::Signal { of, .. } => of.raised_queue(),
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
            What::Vnode { file, pending, .. } => {
                if file.strong_count() == 0 {
                    *dead = true;
                    return None;
                }
                (*pending != 0).then_some((0, *pending, 0))
            }
            What::Never => None,
            // (What is to be done about it is the note's `fflags`, for
            // the kernel; whose message it is, the data.)
            What::Port { space, name } => {
                let (whose, _) = space.lock().unwrap_or_else(|e| e.into_inner()).pending(*name, mono);
                whose.map(|member| (0, n.fflags, member as i64))
            }
            What::File { file, seen, .. } => {
                let Some(f) = file.upgrade() else {
                    *dead = true;
                    return None;
                };
                let h = &f.handle;
                // A file on a disk has something to read while there is
                // more of it past where its descriptor has read to, and how
                // much is said; nothing else is asked of it.
                if n.filter == filt::READ && h.metadata().is_ok_and(|md| md.file_type == FileType::Regular) {
                    let size = h.metadata().map_or(0, |md| md.size as i64);
                    let left = size - f.seek(0, 1).map_or(0, |at| at as i64);
                    return (left > 0).then_some((0, 0, left));
                }
                let (eof, ready) = match found.get(&(n.ident as i32)) {
                    Some(&got) => (got & (poll::HUP | poll::ERR) != 0, got & if n.filter == filt::READ { poll::IN } else { poll::OUT } != 0),
                    None => (h.poll_hup() || (n.filter == filt::READ && h.poll_rdhup()), if n.filter == filt::READ { h.poll_readable() } else { h.poll_writable() }),
                };
                if !ready && !eof {
                    return None;
                }
                // Told of each change once: nothing new since the last.
                if n.flags & ev::CLEAR != 0 && seen.is_some() && h.wakes().is_some() && h.wakes() == *seen {
                    return None;
                }
                // How much there is to read; or room to write, which no
                // file here measures: a pipe's worth.
                let data = if n.filter == filt::READ { h.available().unwrap_or(ready as u64) as i64 } else { 16384 };
                // (Something to read that is nothing is the end: what a
                // host whose processes are apart says of a pipe whose
                // other end another process has closed.)
                let eof = eof || (n.filter == filt::READ && ready && h.available() == Some(0));
                Some((if eof { ev::EOF } else { 0 }, 0, data))
            }
            What::Timer { next, period, absolute } => {
                let now = if *absolute { wall } else { mono };
                (now >= *next).then(|| (0, 0, if *period == 0 { 1 } else { 1 + ((now - *next) / *period) as i64 }))
            }
            What::User { triggered } => triggered.then_some((0, n.fflags, 0)),
            What::Signal { of, signal, seen } => {
                let more = of.raised(*signal).wrapping_sub(*seen);
                (more != 0).then_some((0, 0, more as i64))
            }
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
        let mut notes = self.notes();
        self.hear(&mut notes);
        notes.iter().any(|n| !n.disabled && (n.going || Kqueue::ready(n, mono, wall, &nothing, &mut false).is_some()))
    }

    /// Up to `max` events, from the notes that have something to say. A
    /// note that said its one thing is removed (`EV_ONESHOT`), or switched
    /// off (`EV_DISPATCH`), or starts over (`EV_CLEAR`); a timer is set for
    /// its next time.
    pub fn collect(&self, max: usize, found: &Found) -> Vec<Event> {
        let (mono, wall) = self.now();
        let mut notes = self.notes();
        self.hear(&mut notes);
        let mut out = Vec::new();
        let mut i = 0;
        while i < notes.len() && out.len() < max {
            // A note on its way out, put on: gone, and this says so.
            if notes[i].going && !notes[i].disabled {
                let n = notes.remove(i);
                out.push(Event { ident: n.ident, filter: n.filter, flags: ev::DELETE | ev::ONESHOT | ev::DISPATCH2, udata: n.udata, ..Event::default() });
                self.retire(&notes, n);
                continue;
            }
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
                What::File { file, seen, .. } => *seen = file.upgrade().and_then(|f| f.handle.wakes()),
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
                What::Signal { of, signal, seen } => *seen = of.raised(*signal),
                What::Proc { .. } | What::Never | What::Port { .. } => {}
                // (Said, and said again until it is cleared.)
                What::Vnode { pending, .. } => {
                    if n.flags & ev::CLEAR != 0 {
                        *pending = 0;
                    }
                }
            }
            if (n.flags | flags) & ev::ONESHOT != 0 {
                // (Its one thing said: it goes now, or when it is put on.)
                if n.flags & ev::DISPATCH2 == ev::DISPATCH2 {
                    (n.going, n.disabled) = (true, true);
                    i += 1;
                    continue;
                }
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
            .filter_map(|n| match &n.what {
                What::Timer { next, absolute, .. } => Some(next.saturating_sub(if *absolute { wall } else { mono })),
                // (A port that is a timer's has its message at its moment.)
                What::Port { space, name } => space.lock().unwrap_or_else(|e| e.into_inner()).pending(*name, mono).1.map(|when| when.saturating_sub(mono).max(1)),
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
    let unit: u64 = match e.fflags & (note::SECONDS | note::USECONDS | note::NSECONDS | note::MACHTIME) {
        0 => 1_000_000,
        note::SECONDS => 1_000_000_000,
        note::USECONDS => 1_000,
        note::NSECONDS | note::MACHTIME => 1,
        _ => return Err(Errno::EINVAL),
    };
    let span = (e.data as u64).saturating_mul(unit);
    // A moment: on the clock its unit is of, once.
    if e.fflags & note::ABSOLUTE != 0 {
        return Ok(What::Timer { next: span, period: 0, absolute: e.fflags & note::MACHTIME == 0 });
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
                    // (And a file to watch is watched first.)
                    let watch = (e.filter == filt::VNODE && e.flags & ev::ADD != 0).then(|| self.darwin_vnode_watch(&kq, e.ident as i32));
                    let fds = &self.fds;
                    let mut taken = false;
                    let r = kq.change(
                        &e,
                        &|fd| fds.get(fd),
                        &mut |_| {
                            let fd = opened.ok_or(Errno::ESRCH)??;
                            taken = true;
                            Ok((fd, Arc::clone(&fds.get(fd)?.handle)))
                        },
                        &|_| watch.unwrap_or(Err(Errno::EBADF)),
                        &|name| {
                            self.darwin_ports.lock().unwrap_or_else(|e| e.into_inner()).watchable(self.pid, name)?;
                            Ok(Arc::clone(&self.darwin_ports))
                        },
                        self.signals.process(),
                    );
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
        // what is ready, unless only the first was asked for.
        if flags & KEVENT_FLAG_ERROR_EVENTS != 0 && waiting.is_none() {
            for (i, e) in out.iter().enumerate() {
                e.write(m, events + i as u64 * form.size(), form)?;
            }
            self.darwin_answer = Some(Ok(out.len() as u64));
            return Ok(None);
        }
        if out.is_empty() && nevents > 0 {
            out = kq.collect(nevents as usize, &found);
            // What a port's note has to say is the kernel's to finish:
            // whose message waits, or the message received.
            let room = if form == Form::Qos && x[5] != 0 && x[6] != 0 { Some((x[5], m.read_u64(x[6])?)) } else { None };
            if let Some(left) = self.darwin_kevent_ports(m, &mut out, room) {
                m.write_u64(x[6], left)?;
            }
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
        self.darwin_vnode_watcher(&kq, false)?;
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

    /// Finish the events `out` that are of Mach ports
    /// (`EVFILT_MACHPORT`), each as its note asked: told whose message
    /// waits, or the message received -- into the note's own buffer
    /// (`ext[0]`, of `ext[1]` bytes), or with none the next of `room`,
    /// what `kevent_qos` was given for the call's messages (where, and
    /// how much). How much of `room` is left, if there was any.
    fn darwin_kevent_ports(&mut self, m: &mut GuestMem<'_>, out: &mut Vec<Event>, room: Option<(u64, u64)>) -> Option<u64> {
        use super::port::{opt, Received, MACH_RCV_TOO_LARGE};
        let (mut at, mut left) = room.unwrap_or((0, 0));
        let space = Arc::clone(&self.darwin_ports);
        let mut i = 0;
        while i < out.len() {
            let e = &mut out[i];
            if e.filter != filt::MACHPORT || e.flags & ev::ERROR != 0 {
                i += 1;
                continue;
            }
            let options = e.fflags as u64;
            if options & opt::RCV_MSG == 0 {
                (e.fflags, e.ext) = (0, [0; 4]);
                i += 1;
                continue;
            }
            // (A message with no room in a buffer of the call's own stays
            // where it is, and whose it is is said: the caller receives
            // it itself.)
            let own = e.ext[1] != 0;
            let (to, size) = if own { (e.ext[0], e.ext[1]) } else { (at, left) };
            let large = !own || options & opt::RCV_LARGE != 0;
            let got = {
                let mut sp = space.lock().unwrap_or_else(|e| e.into_inner());
                self.darwin_port_receive(m, &mut sp, e.ident as u32, to, size, large)
            };
            let e = &mut out[i];
            match got {
                Received::Got(n) => {
                    (e.fflags, e.data, e.ext) = (0, 0, [to, n, 0, 0]);
                    if !own {
                        (at, left) = (at + n, left - n);
                    }
                }
                Received::TooLarge { size: n, name } => {
                    let whose = if !own || options & opt::RCV_LARGE_IDENTITY != 0 { name as i64 } else { 0 };
                    (e.fflags, e.data, e.ext) = (MACH_RCV_TOO_LARGE as u32, if large { whose } else { 0 }, [to, if large { n } else { size }, 0, 0]);
                }
                // (Received by another thread since the queue looked.)
                Received::Nothing(_) | Received::NoName | Received::BadBuffer => {
                    out.remove(i);
                    continue;
                }
            }
            i += 1;
        }
        room.map(|_| left)
    }

    /// What hears of files changing for `kq`, made at its first need, on a
    /// descriptor of this process's: a high one, out of the way of the
    /// program's own. A program that closes every descriptor it does not
    /// know closes this one too; it is then put back (unless there is
    /// none yet and none is being `made`).
    fn darwin_vnode_watcher(&self, kq: &Arc<Kqueue>, made: bool) -> Result<Option<Arc<crate::inotify::Inotify>>, Errno> {
        let inst = match kq.watcher() {
            Some((fd, inst)) => {
                let there = self.fds.get(fd).ok().and_then(|f| Arc::clone(&f.handle).as_inotify()).is_some_and(|i| Arc::ptr_eq(&i, &inst));
                if there {
                    return Ok(Some(inst));
                }
                inst
            }
            None if made => Arc::new(crate::inotify::Inotify::new()?),
            None => return Ok(None),
        };
        let handle: Arc<dyn FileHandle> = inst.clone();
        let file = crate::file::OpenFile::new(handle, "anon_inode:[kqueue]".into(), crate::syscalls::oflags::O_NONBLOCK, true, false);
        let high = (self.fds.limit() as i32 - 8).clamp(0, 240);
        let fd = self.fds.insert_at(Arc::clone(&file), high, true).or_else(|_| self.fds.insert(file, true))?;
        kq.set_watcher(fd as i32, Arc::clone(&inst));
        Ok(Some(inst))
    }

    /// The watch `kq`'s watcher has on the file open as `fd`, for a note
    /// (`EVFILT_VNODE`): on the file its name names now, if that is still
    /// the file, and otherwise one that hears nothing.
    fn darwin_vnode_watch(&self, kq: &Arc<Kqueue>, fd: i32) -> Result<i32, Errno> {
        use crate::inotify::mask as im;
        let f = self.fds.get(fd)?;
        let md = f.handle.metadata()?;
        // (A pipe or a socket is no file to watch.)
        if matches!(md.file_type, FileType::Fifo | FileType::Socket) {
            return Err(Errno::EINVAL);
        }
        let Some(inst) = self.darwin_vnode_watcher(kq, true)? else { return Err(Errno::ENOMEM) };
        let all = im::MODIFY | im::ATTRIB | im::MOVE_SELF | im::DELETE_SELF | im::CREATE | im::DELETE | im::MOVED_FROM | im::MOVED_TO;
        let dir = md.file_type == FileType::Directory;
        let wd = match self.vfs.watch_target(&f.path, true) {
            Ok((fs, rel)) if fs.metadata(&rel).is_ok_and(|now| now.ino == md.ino) => {
                if fs.watched_remotely() {
                    inst.add_remote(&rel, dir, all)
                } else {
                    let (key, dir) = fs.watch_key(&rel)?;
                    inst.add_local(key, dir, all)
                }
            }
            _ => inst.add_local(None, dir, all),
        }?;
        Ok(wd as i32)
    }

    /// Descriptor `fd` is about to be closed: if it is the last of a
    /// queue's, the queue's notes go with it, and what it held.
    pub(super) fn darwin_kqueue_closing(&mut self, fd: i32) {
        let Ok(file) = self.fds.get(fd) else { return };
        let Some(kq) = Arc::clone(&file.handle).as_kqueue() else { return };
        if self.fds.entries().iter().filter(|(_, other, _)| Arc::ptr_eq(other, &file)).count() > 1 {
            return;
        }
        for held in kq.shut() {
            let _ = self.sys_close(held);
        }
    }

    /// Whether the call whose wait just ended as `r` is one to be made
    /// again ([`Kernel::finish_blocking`]): a `kevent` is, unless a signal
    /// ended its wait, and a work queue's thread that waited for work
    /// ([`super::workq`]) always. What was left to do after it is then
    /// done with.
    pub(crate) fn darwin_again(&mut self, r: SysResult) -> bool {
        let again = match self.darwin_post {
            Post::Kevent { .. } => r != Err(Errno::EINTR),
            Post::Workq | Post::Again => true,
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
    fn none(_: i32) -> Result<Arc<crate::file::OpenFile>, Errno> {
        Err(Errno::EBADF)
    }
    fn nobody(_: i32) -> Result<(i32, Arc<dyn FileHandle>), Errno> {
        Err(Errno::ESRCH)
    }
    fn unwatched(_: i32) -> Result<i32, Errno> {
        Err(Errno::EBADF)
    }
    fn no_port(_: u32) -> Result<Arc<Mutex<crate::darwin::port::Space>>, Errno> {
        Err(Errno::ENOENT)
    }
    fn quiet() -> Arc<crate::signal::ProcSig> {
        Arc::clone(crate::signal::Signals::new().process())
    }
    fn user(ident: u64, flags: u16, fflags: u32) -> Event {
        Event { ident, filter: filt::USER, flags, fflags, udata: 7, ..Event::default() }
    }

    #[test]
    fn a_user_event_is_said_when_posted_and_once_if_cleared() {
        let kq = Kqueue::new(clock);
        kq.change(&user(1, ev::ADD | ev::CLEAR, 0), &none, &mut nobody, &unwatched, &no_port, &quiet()).unwrap();
        assert!(kq.collect(8, &Found::new()).is_empty());
        kq.change(&user(1, 0, note::TRIGGER | note::FFOR | 5), &none, &mut nobody, &unwatched, &no_port, &quiet()).unwrap();
        let got = kq.collect(8, &Found::new());
        assert_eq!((got.len(), got[0].ident, got[0].fflags, got[0].udata), (1, 1, 5, 7));
        assert!(kq.collect(8, &Found::new()).is_empty());
    }

    #[test]
    fn a_note_that_is_not_there_cannot_be_changed() {
        let kq = Kqueue::new(clock);
        assert_eq!(kq.change(&user(1, ev::DELETE, 0), &none, &mut nobody, &unwatched, &no_port, &quiet()), Err(Errno::ENOENT));
        assert_eq!(kq.change(&user(1, ev::ENABLE, 0), &none, &mut nobody, &unwatched, &no_port, &quiet()), Err(Errno::ENOENT));
        assert_eq!(kq.change(&Event { filter: filt::READ, flags: ev::ADD, ..Event::default() }, &none, &mut nobody, &unwatched, &no_port, &quiet()), Err(Errno::EBADF));
        assert_eq!(kq.change(&Event { filter: -11, flags: ev::ADD, ..Event::default() }, &none, &mut nobody, &unwatched, &no_port, &quiet()), Err(Errno::EOPNOTSUPP));
        // (What never happens here may be watched; a port that is no name cannot.)
        assert_eq!(kq.change(&Event { filter: filt::FS, flags: ev::ADD, ..Event::default() }, &none, &mut nobody, &unwatched, &no_port, &quiet()), Ok(()));
        assert_eq!(kq.change(&Event { filter: filt::MACHPORT, flags: ev::ADD, ..Event::default() }, &none, &mut nobody, &unwatched, &no_port, &quiet()), Err(Errno::ENOENT));
        assert_eq!(kq.change(&Event { filter: -3, flags: ev::ADD, ..Event::default() }, &none, &mut nobody, &unwatched, &no_port, &quiet()), Err(Errno::EPERM));
        assert_eq!(kq.change(&Event { filter: 3, flags: ev::ADD, ..Event::default() }, &none, &mut nobody, &unwatched, &no_port, &quiet()), Err(Errno::EINVAL));
    }

    #[test]
    fn once_is_once_and_dispatch_waits_to_be_switched_on() {
        let kq = Kqueue::new(clock);
        kq.change(&user(1, ev::ADD | ev::ONESHOT, note::TRIGGER), &none, &mut nobody, &unwatched, &no_port, &quiet()).unwrap();
        kq.change(&user(2, ev::ADD | ev::DISPATCH, note::TRIGGER), &none, &mut nobody, &unwatched, &no_port, &quiet()).unwrap();
        assert_eq!(kq.collect(8, &Found::new()).len(), 2);
        assert!(kq.collect(8, &Found::new()).is_empty());
        assert_eq!(kq.change(&user(1, ev::DELETE, 0), &none, &mut nobody, &unwatched, &no_port, &quiet()), Err(Errno::ENOENT));
        kq.change(&user(2, ev::ENABLE, 0), &none, &mut nobody, &unwatched, &no_port, &quiet()).unwrap();
        assert_eq!(kq.collect(8, &Found::new()).len(), 1);
    }
}

#[cfg(test)]
mod signal_tests {
    use super::*;

    #[test]
    fn a_signal_is_counted_from_when_it_is_watched() {
        let kq = Kqueue::new(|| (1000, 0));
        let sigs = crate::signal::Signals::new();
        let p = sigs.process();
        // Darwin's SIGUSR1 is 30, the kernel's 10.
        p.raise(10, None);
        let watch = Event { ident: 30, filter: filt::SIGNAL, flags: ev::ADD, udata: 9, ..Event::default() };
        kq.change(&watch, &|_| Err(Errno::EBADF), &mut |_| Err(Errno::ESRCH), &|_| Err(Errno::EBADF), &|_| Err(Errno::ENOENT), p).unwrap();
        assert!(kq.collect(8, &Found::new()).is_empty());
        p.raise(10, None);
        p.raise(10, None);
        p.raise(12, None);
        let got = kq.collect(8, &Found::new());
        assert_eq!((got.len(), got[0].ident, got[0].data, got[0].flags & ev::CLEAR), (1, 30, 2, ev::CLEAR));
        assert!(kq.collect(8, &Found::new()).is_empty());
        assert_eq!(kq.change(&Event { ident: 99, filter: filt::SIGNAL, flags: ev::ADD, ..Event::default() }, &|_| Err(Errno::EBADF), &mut |_| Err(Errno::ESRCH), &|_| Err(Errno::EBADF), &|_| Err(Errno::ENOENT), p), Ok(()));
        assert!(kq.collect(8, &Found::new()).is_empty());
    }
}
