//! Hosting the Wayland compositor for a guest (native debug host).
//!
//! [`gex_wayland`] is the protocol; this joins it to a guest -- binds a name in
//! the unix-socket namespace, pumps bytes between each client and its
//! compositor state, and answers where a client's pixels are. A client maps its
//! buffer `MAP_SHARED`, which in gex aliases a range of guest memory, so the
//! compositor reads the framebuffer directly with no copy. Pumped at the end of
//! every syscall, so a client's wait always has a serviced reply.

use gex_kernel::file::OpenFile;
use gex_kernel::syscalls::Kernel;
use gex_kernel::unix::{HostListener, MemFd, UnixSocket};
use gex_kernel::vfs::FileHandle;
use gex_kernel::GuestMem;
use gex_wayland::{Buffers, Client, Frame, Placement};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// One connected client: its socket, its protocol state, and the descriptors
/// it has sent.
struct Session {
    /// Which connection this is, for the screen's keys: surface ids are only
    /// unique within one client.
    id: u64,
    /// The guest process that connected. Its requests are read only by a
    /// pump running in that process: a buffer's pixels are found through the
    /// process's own shared mappings and memory, and a commit read anywhere
    /// else found neither and was dropped -- most of a Wine session's frames
    /// never reached the screen.
    owner: Option<i32>,
    socket: Arc<UnixSocket>,
    client: Client,
    /// Descriptors received over `SCM_RIGHTS`, by arrival index (the
    /// protocol refers to one by that, not the client's descriptor number),
    /// and the next index. Held only while the compositor may still need
    /// one: a client makes a pool, and so a descriptor, per buffer.
    fds: HashMap<u32, Arc<OpenFile>>,
    next_fd: u32,
    /// Indices that have arrived and that nothing has used yet: a message
    /// can arrive in pieces, so a descriptor whose `create_pool` is still in
    /// flight is not one the compositor is done with.
    fresh: Vec<u32>,
    /// Event bytes the client's socket had no room for yet, and how many have
    /// gone. Usually empty (events are small), but a dropped tail does not fail
    /// -- it desyncs the stream and leaves the client waiting forever.
    out: Vec<u8>,
    sent: usize,
    /// Where requests are read into, kept: `pump` runs at every syscall of
    /// every thread, and a fresh 64 KiB zeroed buffer each time (calloc's
    /// memset, and glibc growing and trimming its heap around it) was most
    /// of what a Wine game's threads spent their CPU on; keeping it made such
    /// a game more than ten times faster.
    rbuf: Vec<u8>,
    /// The scripted drag over this client: its text/uri-list, and what the
    /// client last said of it (printed when it changes). A client's own drag
    /// over it is `Inner::own`'s, not this.
    drag: Option<String>,
    drag_said: Option<gex_wayland::data::DragStatus>,
}

/// The compositor, as the guest sees it: a name that accepts connections.
#[derive(Clone)]
pub struct Wayland {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    sessions: Mutex<Vec<Session>>,
    size: Mutex<(i32, i32)>,
    /// Where to write each frame, and how many have been written.
    out: Mutex<Option<String>>,
    written: Mutex<u32>,
    /// Presents counted for `GEX_FPS` (count since the last report, and when).
    fps: Mutex<Option<(u32, std::time::Instant)>>,
    /// The GPU compositor, when one could be started. Optional: a machine with
    /// no graphics adapter still runs, compositing on the host instead.
    gpu: Mutex<Option<gex_gpu::Compositor>>,
    /// `GEX_WL_INPUT`: scripted input, timed from the first frame.
    input: Mutex<Option<Input>>,
    /// What is on screen once a client places windows.
    screen: Mutex<Screen>,
    next_session: Mutex<u64>,
    /// The paced frame clock (`frame_tick`): when it started, and its last
    /// tick.
    clock: Mutex<Option<(std::time::Instant, std::time::Instant)>>,
    /// A drag a client started, carried across the clients' windows.
    own: Mutex<Option<OwnDrag>>,
    /// `over TITLE`: the window, by its title, that scripted input to
    /// windows nobody places is in (else every client's focused window).
    over: Mutex<Option<String>>,
    /// The scripted pointer's last point: the screen's with placed
    /// windows, else the window's.
    at: Mutex<(i32, i32)>,
    /// The scripted host drag over windows nobody places: its file list,
    /// and the surface it is over.
    host_drag: Mutex<Option<(usize, Option<Key>)>>,
}

/// A drag a client started (`wl_data_device.start_drag`), carried by this
/// host: the pointer's moves take it to the surface under them, of any
/// client, which is offered the source's types; the release drops it there
/// or cancels it; the target's `receive` is handed to the source's `send`.
struct OwnDrag {
    /// The session that started it, and what it started.
    src: u64,
    start: gex_wayland::data::DragStart,
    /// The surface it is over.
    over: Option<Key>,
    /// Dropped on a session that took it: waiting for its finish, since.
    dropped: Option<(u64, std::time::Instant)>,
}

impl OwnDrag {
    /// The session whose offer is this drag's: over, or dropped on.
    fn target(&self) -> Option<u64> {
        self.dropped.map(|d| d.0).or(self.over.map(|k| k.0))
    }
}

/// How long a target that took a drop has to say it is done with it.
const DROP_FINISH: std::time::Duration = std::time::Duration::from_secs(10);

/// How often frames are "shown" natively, which is when a client's frame
/// callbacks are answered (`GEX_WL_HZ`, default 60; 0: at once, at the
/// commit, as before). This host copies a buffer's pixels at its commit, so
/// a buffer is released then; only the frame callbacks wait.
fn frame_hz() -> u32 {
    static HZ: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *HZ.get_or_init(|| std::env::var("GEX_WL_HZ").ok().and_then(|v| v.parse().ok()).unwrap_or(60))
}

fn keytrace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("GEX_WL_KEYTRACE").is_some())
}

/// One scripted input event (`GEX_WL_INPUT`, [`Input::load`]).
#[derive(Debug, Clone, Copy)]
enum Ev {
    Motion(i32, i32),
    Button(u32, bool),
    Key(u32, bool),
    /// Scroll: axis (0 vertical, 1 horizontal) and wl_fixed pixels.
    Axis(u32, i32),
    /// A drag from the host at a point, of the script's Nth file list
    /// (`Input::drags`); its moves; its release; its cancel.
    Drag(i32, i32, usize),
    DragMove(i32, i32),
    Drop,
    DragLeave,
    /// `over TITLE`: the script's Nth title (`Input::titles`), or none.
    Over(usize),
    /// A menu verb ([`MenuVerb`]) on the script's Nth menu path
    /// (`Input::menus`), with `gex_menu_v1.modifiers` held.
    Menu(MenuVerb, usize, u32),
}

/// What a script does to a published menu (`gex_menu_v1`), the shell's part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuVerb {
    List,
    Open,
    Close,
    Activate,
}

/// Whether this host presents menus (`GEX_WL_MENUS=1`): then
/// `gex_menu_manager_v1` is advertised, a program that sees it publishes its
/// menu bar and draws none, and a script's `menu` lines are the shell.
fn presents_menus() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("GEX_WL_MENUS").is_ok_and(|v| !v.is_empty() && v != "0"))
}

/// A script of input events at times (seconds) after the compositor's Nth
/// frame (`frames N`, default 1: a game's first frame is often a splash long
/// before its menu; 0: from the compositor's start), delivered to the clients by [`Wayland::pump`] as they fall due:
/// what an automated game test needs to get past menus. One event per line,
/// `#` comments:
///
/// ```text
/// frames 300                 # time from the 300th frame
/// 30    click 100 328        # motion, then BTN_LEFT down and (0.1 s later) up
/// 31.5  move 400 300
/// 32    button 272 1         # a Linux BTN_* code, 1 down / 0 up
/// 40    tap enter            # key down, and up 0.1 s later
/// 41    key z 1              # a key name or Linux KEY_* code, 1 down / 0 up
/// 42    scroll 100           # pixels down (negative: up); 100 is a wheel notch
/// 43    scroll 2.5 x 40      # 40 events of 2.5 px, 1/60 s apart (a trackpad)
/// 44    hscroll -100         # pixels right (negative: left), `x N` as scroll
/// 50    drag 300 200 /a /b   # a drag of files (text/uri-list) from the host enters here
/// 50.5  dragmove 320 210     # and moves; the screen's hit test picks the window
/// 51    drop                 # released: the window under it takes it or refuses it
/// 52    dragleave            # (or cancelled)
/// 60    output 1000 650      # the output is resized: every client is told
/// 70    over Some Title      # what follows is in the window titled so
/// 71    over                 # and back to every client's focused window
/// 61    menu                 # print every published menu (`GEX_WL_MENUS=1`)
/// 62    menu-open Window     # about_to_open; the menu is printed when the client acks
/// 63    menu-activate Window > Zoom mods=alt+shift   # an item chosen, modifiers held
/// 64    menu-close Window
/// ```
///
/// A menu or item is named by its number, as `menu` prints it, or by the
/// titles down from the menu bar separated by `>` (`0`, or nothing, is the
/// bar itself); `dock:` in front names the application's dock menu instead.
/// What is printed starts `[wl-menu]`.
///
/// `window W H` in place of `frames N` starts the clock once a placed window
/// of W x H is on the screen: what a test waits for when a loaded machine
/// makes any fixed delay after the first frame too short. `clients N` starts
/// it once the Nth client has connected: for a client that shows no window
/// (one that only listens to its outputs), where `frames 0` would count the
/// time its program takes to load.
///
/// Coordinates are the focused window's pixels, its toplevel's: a point
/// over one of the window's popups (or subsurfaces) reaches that surface in
/// its own coordinates, with the pointer's enter and leave as it moves
/// between them; a point over none of them reaches nothing, and a press
/// there or on the window outside an open popup dismisses the popup. With
/// placed windows (`gex_window_placement_v1`), the screen's pixels.
///
/// With windows nobody places, every client's focused window is at the
/// same place: `over TITLE` says which window (of any client) the events
/// after it are in -- they go to that one alone, and the pointer leaves the
/// window it was in -- and `over` alone goes back. A drag a client starts
/// (`wl_data_device.start_drag`, at a press) has the pointer until the
/// release: a `move` takes it to the surface there -- of the window `over`
/// names, or the one it started in -- and the release drops it or cancels
/// it. The script's own drag (`drag`, `dragmove`, `drop`, `dragleave`) of
/// files goes the same way: to the window `over` names, or the newest
/// client's. With placed windows the screen's hit test picks the window.
struct Input {
    events: Vec<(f64, Ev, String)>,
    /// Each `drag` line's files, as the text/uri-list the host offers.
    drags: Vec<String>,
    /// Each `over` line's title.
    titles: Vec<String>,
    /// Each `menu` line's path.
    menus: Vec<String>,
    next: usize,
    start: Option<std::time::Instant>,
    /// Frames to see before the clock starts, and how many have been.
    after: u64,
    seen: u64,
    /// `window W H`: the clock starts once a placed window this size is up.
    window: Option<(u32, u32)>,
    /// `clients N`: the clock starts once N clients have connected, and how
    /// many have.
    clients: Option<u32>,
    connected: u32,
    /// `output W H` lines: the output's new size, and when. Apart from the
    /// events, which wait for a client that takes input; a resize does not.
    outputs: Vec<(f64, i32, i32)>,
}

/// Paths as a text/uri-list: `file://` URIs, CRLF after each, with every
/// byte outside RFC 3986's unreserved set and `/` percent-encoded.
pub fn uri_list(paths: &[&str]) -> String {
    let mut out = String::new();
    for p in paths {
        out.push_str("file://");
        for &b in p.as_bytes() {
            if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        out.push_str("\r\n");
    }
    out
}

/// Linux `KEY_*` codes for the names a script may use (else a number).
fn key_code(name: &str) -> Option<u32> {
    let k = match name.to_ascii_lowercase().as_str() {
        "esc" | "escape" => 1,
        "1" => 2, "2" => 3, "3" => 4, "4" => 5, "5" => 6, "6" => 7, "7" => 8, "8" => 9, "9" => 10, "0" => 11,
        "backspace" => 14, "tab" => 15,
        "q" => 16, "w" => 17, "e" => 18, "r" => 19, "t" => 20, "y" => 21, "u" => 22, "i" => 23, "o" => 24, "p" => 25,
        "enter" | "return" => 28, "ctrl" | "leftctrl" => 29,
        "a" => 30, "s" => 31, "d" => 32, "f" => 33, "g" => 34, "h" => 35, "j" => 36, "k" => 37, "l" => 38,
        "shift" | "leftshift" => 42,
        "z" => 44, "x" => 45, "c" => 46, "v" => 47, "b" => 48, "n" => 49, "m" => 50,
        "alt" | "leftalt" => 56, "space" => 57,
        "f1" => 59, "f2" => 60, "f3" => 61, "f4" => 62, "f5" => 63,
        "up" => 103, "left" => 105, "right" => 106, "down" => 108,
        n => return n.parse().ok(),
    };
    Some(k)
}

impl Input {
    fn load(path: &str) -> Result<Input, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let mut events = Vec::new();
        let mut drags = Vec::new();
        let mut titles = Vec::new();
        let mut menus = Vec::new();
        let mut after = 1;
        let mut window = None;
        let mut clients = None;
        let mut outputs = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let w: Vec<&str> = line.split_whitespace().collect();
            let bad = || format!("{path}:{}: cannot read `{line}`", n + 1);
            if w[0] == "frames" {
                after = w.get(1).and_then(|v| v.parse().ok()).ok_or_else(bad)?;
                continue;
            }
            if w[0] == "clients" {
                clients = Some(w.get(1).and_then(|v| v.parse().ok()).ok_or_else(bad)?);
                continue;
            }
            if w[0] == "window" {
                let n = |i: usize| -> Result<u32, String> { w.get(i).and_then(|v| v.parse().ok()).ok_or_else(bad) };
                window = Some((n(1)?, n(2)?));
                continue;
            }
            let t: f64 = w[0].parse().map_err(|_| bad())?;
            let num = |i: usize| -> Result<i32, String> { w.get(i).and_then(|v| v.parse().ok()).ok_or_else(bad) };
            let key = |i: usize| -> Result<u32, String> { w.get(i).and_then(|v| key_code(v)).ok_or_else(bad) };
            let down = |i: usize| -> Result<bool, String> { Ok(num(i)? != 0) };
            let l = line.to_string();
            match w.get(1).copied() {
                Some("move") => events.push((t, Ev::Motion(num(2)?, num(3)?), l)),
                Some("click") => {
                    events.push((t, Ev::Motion(num(2)?, num(3)?), l.clone()));
                    events.push((t, Ev::Button(0x110, true), l.clone()));
                    events.push((t + 0.1, Ev::Button(0x110, false), l));
                }
                Some("button") => events.push((t, Ev::Button(num(2)? as u32, down(3)?), l)),
                Some("key") => events.push((t, Ev::Key(key(2)?, down(3)?), l)),
                Some(c @ ("scroll" | "hscroll")) => {
                    let px: f64 = w.get(2).and_then(|v| v.parse().ok()).ok_or_else(bad)?;
                    let n: u32 = match w.get(3).copied() {
                        None => 1,
                        Some("x") => w.get(4).and_then(|v| v.parse().ok()).ok_or_else(bad)?,
                        Some(_) => return Err(bad()),
                    };
                    let axis = (c == "hscroll") as u32;
                    for i in 0..n {
                        events.push((t + i as f64 / 60.0, Ev::Axis(axis, (px * 256.0).round() as i32), l.clone()));
                    }
                }
                Some("drag") => {
                    if w.len() < 5 {
                        return Err(bad());
                    }
                    drags.push(uri_list(&w[4..]));
                    events.push((t, Ev::Drag(num(2)?, num(3)?, drags.len() - 1), l));
                }
                Some("dragmove") => events.push((t, Ev::DragMove(num(2)?, num(3)?), l)),
                Some("drop") => events.push((t, Ev::Drop, l)),
                Some("dragleave") => events.push((t, Ev::DragLeave, l)),
                Some("over") => {
                    let title = w[2..].join(" ");
                    let n = if title.is_empty() {
                        usize::MAX
                    } else {
                        titles.push(title);
                        titles.len() - 1
                    };
                    events.push((t, Ev::Over(n), l));
                }
                Some("output") => outputs.push((t, num(2)?.max(1), num(3)?.max(1))),
                Some(c @ ("menu" | "menu-open" | "menu-close" | "menu-activate")) => {
                    let verb = match c {
                        "menu" => MenuVerb::List,
                        "menu-open" => MenuVerb::Open,
                        "menu-close" => MenuVerb::Close,
                        _ => MenuVerb::Activate,
                    };
                    let mut mods = 0;
                    let mut path = Vec::new();
                    for word in &w[2..] {
                        match word.strip_prefix("mods=") {
                            Some(m) => {
                                for name in m.split('+') {
                                    mods |= match name {
                                        "shift" => gex_wayland::menu::SHIFT,
                                        "control" | "ctrl" => gex_wayland::menu::CONTROL,
                                        "alt" => gex_wayland::menu::ALT,
                                        "logo" | "super" | "cmd" => gex_wayland::menu::LOGO,
                                        _ => return Err(bad()),
                                    };
                                }
                            }
                            None => path.push(*word),
                        }
                    }
                    menus.push(path.join(" "));
                    events.push((t, Ev::Menu(verb, menus.len() - 1, mods), l));
                }
                Some("tap") => {
                    events.push((t, Ev::Key(key(2)?, true), l.clone()));
                    events.push((t + 0.1, Ev::Key(key(2)?, false), l));
                }
                _ => return Err(bad()),
            }
        }
        events.sort_by(|a, b| a.0.total_cmp(&b.0));
        outputs.sort_by(|a, b| a.0.total_cmp(&b.0));
        // `frames 0`: from the compositor's start.
        let start = if after == 0 && window.is_none() && clients.is_none() { Some(std::time::Instant::now()) } else { None };
        Ok(Input { events, drags, titles, menus, next: 0, start, after, seen: 0, window, clients, connected: 0, outputs })
    }
}

impl std::fmt::Debug for Wayland {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Wayland")
    }
}

impl Wayland {
    pub fn new(width: i32, height: i32) -> Wayland {
        let input = match std::env::var("GEX_WL_INPUT") {
            Ok(p) => match Input::load(&p) {
                Ok(i) => Some(i),
                Err(e) => {
                    eprintln!("[gex] GEX_WL_INPUT: {e}");
                    None
                }
            },
            Err(_) => None,
        };
        Wayland {
            inner: Arc::new(Inner {
                size: Mutex::new((width, height)),
                input: Mutex::new(input),
                ..Default::default()
            }),
        }
    }

    /// Whether a paced frame is due now, and its time in milliseconds since
    /// the compositor's first: every 1/`GEX_WL_HZ` s, from the last.
    fn frame_tick(&self) -> Option<u32> {
        let hz = frame_hz();
        if hz == 0 {
            return None;
        }
        let now = std::time::Instant::now();
        let mut c = self.inner.clock.lock().unwrap();
        let (start, last) = c.get_or_insert((now, now - std::time::Duration::from_secs(1)));
        let period = std::time::Duration::from_secs(1) / hz;
        if now.duration_since(*last) < period {
            return None;
        }
        *last = now;
        Some(now.duration_since(*start).as_millis() as u32)
    }

    /// A frame was shown (a client's commit, or GL's own present): the
    /// input script's clock starts at its `frames N`th.
    fn note_frame(&self) {
        crate::stall::frame();
        if let Some(inp) = self.inner.input.lock().unwrap().as_mut() {
            inp.seen += 1;
            if inp.window.is_none() && inp.clients.is_none() && inp.seen >= inp.after && inp.start.is_none() {
                eprintln!("[gex] input: the script's clock starts at frame {}", inp.seen);
                inp.start = Some(std::time::Instant::now());
            }
        }
    }

    /// The screen changed: the input script's clock starts once its
    /// `window W H` is up.
    fn note_screen(&self) {
        let mut g = self.inner.input.lock().unwrap();
        let Some(inp) = g.as_mut() else { return };
        let (Some((w, h)), None) = (inp.window, inp.start) else { return };
        let screen = self.inner.screen.lock().unwrap();
        let up = screen
            .placed
            .keys()
            .any(|k| screen.frames.get(k).is_some_and(|f| f.width as u32 == w && f.height as u32 == h));
        if up {
            eprintln!("[gex] input: the script's clock starts with a {w}x{h} window up");
            inp.start = Some(std::time::Instant::now());
        }
    }

    /// A client connected: the input script's clock starts at its `clients N`th.
    fn note_client(&self) {
        let mut g = self.inner.input.lock().unwrap();
        let Some(inp) = g.as_mut() else { return };
        inp.connected += 1;
        if inp.start.is_none() && inp.clients.is_some_and(|n| inp.connected >= n) {
            eprintln!("[gex] input: the script's clock starts with client {}", inp.connected);
            inp.start = Some(std::time::Instant::now());
        }
    }

    /// The script's Nth `over` title (none for `usize::MAX`).
    fn title(&self, n: usize) -> Option<String> {
        self.inner.input.lock().unwrap().as_ref()?.titles.get(n).cloned()
    }

    /// The script's Nth drag's text/uri-list.
    fn drag_list(&self, n: usize) -> Option<String> {
        self.inner.input.lock().unwrap().as_ref()?.drags.get(n).cloned()
    }

    /// The script's Nth menu path.
    fn menu_path(&self, n: usize) -> String {
        self.inner.input.lock().unwrap().as_ref().and_then(|i| i.menus.get(n).cloned()).unwrap_or_default()
    }

    /// The scripted events now due (`GEX_WL_INPUT`), with their time in ms.
    fn due_input(&self) -> Vec<(Ev, u32)> {
        let mut g = self.inner.input.lock().unwrap();
        let Some(inp) = g.as_mut() else { return Vec::new() };
        let Some(start) = inp.start else { return Vec::new() };
        let now = start.elapsed().as_secs_f64();
        let mut due = Vec::new();
        while inp.next < inp.events.len() && inp.events[inp.next].0 <= now {
            let (t, ev, line) = &inp.events[inp.next];
            eprintln!("[gex] input at {now:.1} s: {ev:?} ({line})");
            due.push((*ev, (t * 1000.0) as u32));
            inp.next += 1;
        }
        due
    }

    /// The output's size, after any scripted resize now due (`output W H`).
    /// Only what clients are told: frames are still written at the size the
    /// compositor started with.
    fn output_size(&self) -> (i32, i32) {
        let mut g = self.inner.input.lock().unwrap();
        if let Some(inp) = g.as_mut() {
            if let Some(start) = inp.start {
                let now = start.elapsed().as_secs_f64();
                let due = inp.outputs.iter().take_while(|o| o.0 <= now).count();
                if let Some(&(_, w, h)) = inp.outputs[..due].last() {
                    eprintln!("[gex] input at {now:.1} s: the output is now {w}x{h}");
                    *self.inner.size.lock().unwrap() = (w, h);
                }
                inp.outputs.drain(..due);
            }
        }
        *self.inner.size.lock().unwrap()
    }

    /// Composite on the GPU rather than handing single frames to the host.
    /// Returns the adapter, or the reason there is none; either way the
    /// compositor keeps working.
    pub fn enable_gpu(&self, width: i32, height: i32) -> Result<String, String> {
        match gex_gpu::Compositor::new(width.max(1) as u32, height.max(1) as u32) {
            Ok(c) => {
                let name = c.adapter.clone();
                *self.inner.gpu.lock().unwrap() = Some(c);
                Ok(name)
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Write every committed frame (every Nth with `GEX_PPM_EVERY=N`) as a
    /// PPM at `prefix-NNN.ppm` -- a file is what a test can compare and what
    /// works with no display.
    pub fn write_frames_to(&self, prefix: &str) {
        *self.inner.out.lock().unwrap() = Some(prefix.to_string());
    }

    /// `GEX_PPM_EVERY=N` (crate::ppm_every): count a frame that is not
    /// written, so the ones that are keep their real index.
    fn skip_frame(&self) -> bool {
        let mut n = self.inner.written.lock().unwrap();
        if *n % crate::ppm_every() == 0 {
            return false;
        }
        *n += 1;
        true
    }

    /// Write out what was drawn: with a GPU, the whole composited screen;
    /// without one, a single client buffer (nothing here would combine them).
    fn save(&self, frames: &[Frame]) {
        let Some(prefix) = self.inner.out.lock().unwrap().clone() else {
            return;
        };
        // The placement log ([wl-place]) for every batch, written or not:
        // under `--wayland-out null` and GEX_PPM_EVERY too.
        {
            let mut screen = self.inner.screen.lock().unwrap();
            if screen.active() {
                screen.list();
            }
        }
        // `GEX_PPM_GL_ONLY=1`: only gex-gl's presented frames are written
        // (`present_external`), not client buffers -- with a GL game on
        // Xwayland, the X window's own buffer is black and would take the
        // GEX_PPM_EVERY turns.
        static GL_ONLY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *GL_ONLY.get_or_init(|| std::env::var_os("GEX_PPM_GL_ONLY").is_some_and(|v| v != "0")) {
            return;
        }
        // Before the GPU read-back, so a skipped frame costs nothing.
        let Some(path) = self.frame_path(&prefix) else { return };
        let mut gpu = self.inner.gpu.lock().unwrap();
        let screen = self.inner.screen.lock().unwrap();
        let (width, height, rgb) = if let Some(c) = gpu.as_mut() {
            let drawn = if screen.active() || screen.icon.is_some() { c.draw_at(&screen.items(c.size())) } else { c.draw(frames) };
            match drawn.and_then(|()| c.read_back()) {
                Ok(rgba) => {
                    let (w, h) = c.size();
                    (w as i32, h as i32, gex_gpu::rgba_to_rgb(&rgba))
                }
                Err(e) => {
                    eprintln!("gex: the GPU compositor failed: {e}");
                    return;
                }
            }
        } else {
            let Some(frame) = frames.last() else { return };
            (frame.width, frame.height, gex_wayland::to_rgb(frame))
        };
        let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
        out.extend_from_slice(&rgb);
        if let Err(e) = std::fs::write(&path, out) {
            eprintln!("gex: could not write {path}: {e}");
        }
    }

    /// Count a presented frame and say where to write it, if anywhere:
    /// `GEX_PPM_EVERY=N` keeps every Nth ([`Self::skip_frame`]), and the
    /// prefix `null` keeps none. `GEX_FPS=1` prints presents per second to
    /// stderr, which is what a measurement script reads.
    fn frame_path(&self, prefix: &str) -> Option<String> {
        if std::env::var_os("GEX_FPS").is_some_and(|v| v != "0") {
            let mut fps = self.inner.fps.lock().unwrap();
            let (count, since) = fps.get_or_insert((0, std::time::Instant::now()));
            *count += 1;
            let dt = since.elapsed().as_secs_f64();
            if dt >= 1.0 {
                eprintln!("[gex] fps {:.1}", *count as f64 / dt);
                *count = 0;
                *since = std::time::Instant::now();
            }
        }
        if prefix == "null" || self.skip_frame() {
            return None;
        }
        let mut n = self.inner.written.lock().unwrap();
        let path = format!("{prefix}-{:03}.ppm", *n);
        *n += 1;
        Some(path)
    }

    /// Write a frame drawn somewhere other than a client buffer (a GL
    /// client's host surface): RGBA rows, top first.
    pub fn present_external(&self, width: i32, height: i32, rgba: &[u8]) {
        self.note_frame();
        let Some(prefix) = self.inner.out.lock().unwrap().clone() else {
            return;
        };
        let Some(path) = self.frame_path(&prefix) else { return };
        let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
        out.extend(rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]));
        if let Err(e) = std::fs::write(&path, out) {
            eprintln!("gex: could not write {path}: {e}");
        }
    }

    pub fn client_count(&self) -> usize {
        self.inner.sessions.lock().unwrap().len()
    }

    /// Move bytes in both directions for every client, and collect commits. The
    /// kernel locates a buffer's pixels through its shared mappings; guest
    /// memory is where they are.
    /// Process `pid` has ended: its clients, which only it reads, are gone
    /// with their windows.
    pub fn forget_process(&self, pid: i32) {
        let logged = self.inner.out.lock().unwrap().is_some();
        let mut gone = Vec::new();
        self.inner.sessions.lock().unwrap().retain(|s| {
            let mine = s.owner == Some(pid);
            if mine {
                gone.push(s.id);
            }
            !mine
        });
        if gone.is_empty() {
            return;
        }
        let mut screen = self.inner.screen.lock().unwrap();
        for sid in gone {
            screen.forget(sid);
        }
        if logged {
            screen.list();
        }
    }

    pub fn pump(&self, kernel: &Kernel, mem: &GuestMem<'_>) {
        PROCESS.with(|p| p.set(Some(kernel.pid)));
        // Scripted input waits for a client that can take it (a seat and a
        // surface); a GL-only window gets the keyboard by default.
        if self.inner.input.lock().unwrap().is_some() {
            for s in self.inner.sessions.lock().unwrap().iter_mut() {
                s.client.focus_fallback();
            }
        }
        // Placed windows: the screen knows where each is, so it decides which
        // client an event is for, as a compositor does.
        let placing = self.inner.screen.lock().unwrap().active();
        let ready = self.inner.sessions.lock().unwrap().iter().any(|s| {
            if placing { s.client.wants_input() } else { s.client.input_ready() }
        });
        let due = if ready { self.due_input() } else { Vec::new() };
        // Each event with the window `over` names in effect for it, and the
        // scripted pointer's last point.
        let over_start = self.inner.over.lock().unwrap().clone();
        let mut cur = over_start.clone();
        let due: Vec<(Ev, u32, Option<String>)> = due
            .into_iter()
            .map(|(ev, ms)| {
                match ev {
                    Ev::Over(n) => cur = if n == usize::MAX { None } else { self.title(n) },
                    Ev::Motion(x, y) => *self.inner.at.lock().unwrap() = (x, y),
                    _ => {}
                }
                (ev, ms, cur.clone())
            })
            .collect();
        *self.inner.over.lock().unwrap() = cur;
        // A client's own drag has the pointer: its moves and its release are
        // the drag's (`carry`), not any client's.
        let (dragging, own_target) = {
            let own = self.inner.own.lock().unwrap();
            (own.is_some(), own.as_ref().and_then(|d| d.target()))
        };
        let pointer = |ev: &Ev| matches!(ev, Ev::Motion(..) | Ev::Button(..));
        let mut routed = if placing && !due.is_empty() {
            let plain: Vec<(Ev, u32)> = due.iter().filter(|e| !(dragging && pointer(&e.0))).map(|e| (e.0, e.1)).collect();
            self.inner.screen.lock().unwrap().route(&plain)
        } else {
            Vec::new()
        };
        // Read every turn: a resized output is `mode` and `done` on every
        // bound `wl_output` (nothing when a client was told this size).
        let (out_w, out_h) = self.output_size();
        let mut sessions = self.inner.sessions.lock().unwrap();
        for s in sessions.iter_mut() {
            s.client.set_output_size(out_w, out_h);
        }
        let mut produced = Vec::new();
        let tick = self.frame_tick();
        sessions.retain_mut(|s| {
            let mut gone = false;
            let mine = s.owner.is_none_or(|o| o == kernel.pid);
            // A big read: a toolkit's startup is a few KB of requests at once.
            let mut buf = std::mem::take(&mut s.rbuf);
            if buf.is_empty() {
                buf = vec![0u8; 64 * 1024];
            }
            while mine {
                let n = match s.socket.recv(&mut buf, 32, false) {
                    Ok(0) => {
                        gone = true;
                        break;
                    }
                    Ok(n) => n as usize,
                    // Nothing more waiting.
                    Err(_) => break,
                };
                let mut tokens = Vec::new();
                for file in s.socket.take_received() {
                    s.fds.insert(s.next_fd, file);
                    tokens.push(s.next_fd);
                    s.next_fd += 1;
                }
                s.fresh.extend_from_slice(&tokens);
                let view = PoolView {
                    kernel,
                    mem,
                    fds: &s.fds,
                };
                s.client.receive(&buf[..n], &tokens, &view);
                if n < buf.len() {
                    break;
                }
            }
            s.rbuf = buf;
            s.release_unused();
            if !due.is_empty() {
                eprintln!("[gex] input: {}", s.client.describe_input());
            }
            let take = s.client.input_ready() && !placing;
            let mut prev = over_start.clone();
            for (ev, ms, over) in due.iter() {
                let (ev, ms) = (*ev, *ms);
                // The window `over` names: this client's or another's.
                let top = over.as_ref().map(|t| s.client.toplevel_titled(t));
                if let Ev::Over(_) = ev {
                    // The pointer was in one of this client's windows and is
                    // now in another client's: it leaves.
                    let was = prev.as_ref().is_some_and(|t| s.client.toplevel_titled(t).is_some());
                    if was && !top.is_some_and(|t| t.is_some()) {
                        s.client.pointer_clear();
                    }
                    s.client.point_window(top.flatten());
                    prev = over.clone();
                    continue;
                }
                if !take || top == Some(None) {
                    continue;
                }
                match ev {
                    // (the drag's)
                    Ev::Motion(..) | Ev::Button(..) if dragging => {}
                    // (the window's own hit test: a popup's point in the
                    // popup's coordinates, as the page's desktop sends it)
                    Ev::Motion(x, y) => s.client.pointer_in_window(x, y, ms),
                    Ev::Button(b, down) => s.client.pointer_button_in_window(b, down, ms),
                    Ev::Key(k, down) => s.client.keyboard_key(k, down, ms),
                    Ev::Axis(axis, v) => s.client.pointer_axis(axis, v, ms),
                    // (the script's drag of files: `carry`)
                    Ev::Drag(..) | Ev::DragMove(..) | Ev::Drop | Ev::DragLeave | Ev::Over(_) => {}
                    Ev::Menu(..) => {}
                }
            }
            // The shell's part of `gex_menu_v1`, from the script; and what
            // the client answered.
            for (ev, _, _) in &due {
                if let Ev::Menu(verb, n, mods) = *ev {
                    s.menu_script(verb, &self.menu_path(n), mods);
                }
            }
            s.menu_answers();
            for (_, ev) in routed.iter().filter(|r| r.0 == s.id) {
                match *ev {
                    Routed::Origin(surface, x, y) => s.client.placement_origin(surface, x, y),
                    Routed::PointerTo(surface, x, y, ms) => s.client.pointer_to(surface, x, y, ms),
                    Routed::PointerClear => s.client.pointer_clear(),
                    Routed::Button(b, down, ms) => s.client.pointer_button(b, down, ms),
                    Routed::Axis(axis, v, ms) => s.client.pointer_axis(axis, v, ms),
                    Routed::Key(k, down, ms) => s.client.keyboard_key(k, down, ms),
                    Routed::KeyboardFocus(surface) => s.client.keyboard_focus(surface),
                    Routed::KeyboardLeave => s.client.keyboard_focus(0),
                    Routed::DragEnter(surface, x, y, n) => {
                        s.drag = self.drag_list(n);
                        let ok = s.client.drag_enter(surface, x, y, &["text/uri-list"], gex_wayland::data::DND_COPY);
                        eprintln!("[gex] dnd: enter {}:{surface} at {x},{y}{}", s.id, if ok { "" } else { ": the client has no data device" });
                    }
                    Routed::DragMotion(x, y, ms) => s.client.drag_motion(x, y, ms),
                    Routed::Drop => {
                        let taken = s.client.drag_drop();
                        eprintln!("[gex] dnd: drop on {}: {}", s.id, if taken { "taken" } else { "refused" });
                    }
                    Routed::DragLeave => s.client.drag_leave(),
                }
            }
            s.answer_drag(own_target == Some(s.id));
            routed.retain(|r| r.0 != s.id);
            // A window that asked to be fullscreen (a rootful Xwayland, a
            // nested compositor) is given the whole output, and one that took
            // it back is left to choose its size again.
            for (surface, on) in s.client.take_fullscreen_changes() {
                let (w, h) = if on { *self.inner.size.lock().unwrap() } else { (0, 0) };
                let states = gex_wayland::STATE_ACTIVATED | if on { gex_wayland::STATE_FULLSCREEN } else { 0 };
                s.client.configure(surface, w, h, states);
            }
            let frames = s.client.take_frames();
            self.inner.screen.lock().unwrap().update(s.id, &mut s.client, &frames);
            produced.extend(frames);
            // A paced frame: every commit so far is shown, and the frame
            // callbacks they carried are answered.
            if let Some(ms) = tick {
                let seq = s.client.commit_seq();
                s.client.present_upto(seq, ms);
            }
            s.out.extend_from_slice(&s.client.take_outgoing());
            // A `wl_keyboard.keymap` event owes a descriptor for its `h`
            // argument: an in-memory file holding the keymap, staged on the
            // socket so it rides the next send (as the browser host does).
            if let Some(km) = s.client.take_keymap() {
                let memfd = Arc::new(MemFd::new("gex-keymap"));
                let _ = memfd.write_at(0, &km);
                let handle: Arc<dyn FileHandle> = memfd;
                let file = OpenFile::new(handle, "anon:wl-keymap".into(), 0, true, false);
                s.socket.stage_send(vec![file]);
            }
            let pending = s.out.len() - s.sent;
            let mut failed = None;
            while s.sent < s.out.len() {
                match s.socket.send(&s.out[s.sent..]) {
                    Ok(0) => break,
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                    Ok(n) => s.sent += n as usize,
                }
            }
            if keytrace() && (failed.is_some() || s.sent < s.out.len() || !due.is_empty()) {
                eprintln!("[wl-key] pump: {pending} bytes to send, {} left, error {failed:?}", s.out.len() - s.sent);
            }
            if s.sent == s.out.len() {
                s.out.clear();
                s.sent = 0;
            }
            let keep = !gone && s.client.failed.is_none();
            if !keep {
                let logged = self.inner.out.lock().unwrap().is_some();
                let mut screen = self.inner.screen.lock().unwrap();
                screen.forget(s.id);
                // Its windows' going is a change of the placement log too.
                if logged {
                    screen.list();
                }
            }
            keep
        });
        self.carry(&mut sessions, &due, dragging, placing);
        drop(sessions);
        if !produced.is_empty() {
            self.note_frame();
            // One screen per batch of commits, not one per surface.
            self.save(&produced);
            self.note_screen();
        }
    }
}

impl Wayland {
    /// The drags, after every session's requests are read: a client's own
    /// (`OwnDrag`) -- its start, the pointer's moves and release, what the
    /// target says told to the source, the target's receives handed to the
    /// source's sends, the target's finish -- and the script's drag of files
    /// over windows nobody places.
    fn carry(&self, sessions: &mut [Session], due: &[(Ev, u32, Option<String>)], dragging: bool, placing: bool) {
        let mut own = self.inner.own.lock().unwrap();
        // A drag started: the pointer is its own from now on, where it is.
        for i in 0..sessions.len() {
            let Some(start) = sessions[i].client.take_drag_start() else { continue };
            if let Some(old) = own.take() {
                eprintln!("[gex] dnd: a new drag ends the one from {}", old.src);
                Self::end_own(sessions, old, false);
            }
            eprintln!(
                "[gex] dnd: {} starts a drag of {:?} (actions {}) from surface {}{}",
                sessions[i].id,
                start.mimes,
                start.actions,
                start.origin,
                start.icon.map(|i| format!(", icon {i}")).unwrap_or_default()
            );
            let mut d = OwnDrag { src: sessions[i].id, start, over: None, dropped: None };
            let at = *self.inner.at.lock().unwrap();
            let over = self.inner.over.lock().unwrap().clone();
            self.drag_to(sessions, &mut d, over.as_ref(), placing, at, 0);
            *own = Some(d);
        }
        // Its moves and its release (what was due before it started went to
        // the clients as ever).
        if dragging {
            for (ev, ms, over) in due {
                let Some(d) = own.as_mut().filter(|d| d.dropped.is_none()) else { break };
                match *ev {
                    Ev::Motion(x, y) => self.drag_to(sessions, d, over.as_ref(), placing, (x, y), *ms),
                    Ev::Button(_, false) => {
                        let idx = |sessions: &[Session], id: u64| sessions.iter().position(|s| s.id == id);
                        let target = d.over.and_then(|k| idx(sessions, k.0).map(|i| (i, k)));
                        let src = idx(sessions, d.src);
                        let taken = target.is_some_and(|(i, _)| sessions[i].client.drag_drop());
                        match (taken, target, src) {
                            (true, Some((ti, k)), Some(si)) => {
                                eprintln!("[gex] dnd: dropped on {}:{}: taken", k.0, k.1);
                                sessions[si].client.drag_source_drop(d.start.source);
                                d.dropped = Some((sessions[ti].id, std::time::Instant::now()));
                                d.over = None;
                            }
                            _ => {
                                eprintln!("[gex] dnd: released over {:?}: cancelled", d.over);
                                if let Some(d) = own.take() {
                                    Self::end_own(sessions, d, false);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut end = None;
        if let Some(d) = own.as_mut() {
            let idx = |id: u64| sessions.iter().position(|s| s.id == id);
            let src = idx(d.src);
            // The source gone, or its drag ended by itself (it destroyed its
            // source): over.
            if src.is_none() || (d.dropped.is_none() && !src.is_some_and(|i| sessions[i].client.is_dragging())) {
                eprintln!("[gex] dnd: the drag's source has gone");
                end = Some(false);
            }
            let target = d.target().and_then(idx);
            if d.target().is_some() && target.is_none() {
                // Its target went: over nothing.
                d.over = None;
                if d.dropped.is_some() {
                    end = Some(false);
                }
            }
            let status = target.and_then(|i| sessions[i].client.drag_status());
            // What the target takes, to the source, as it changes.
            if let (Some(si), Some(source), None) = (src, d.start.source, d.dropped) {
                sessions[si].client.drag_source_status(source, status.as_ref());
            }
            // The target's receives: its descriptor, for the source to write
            // into, with the source's `send`.
            if let Some(ti) = target {
                for r in sessions[ti].client.take_receives() {
                    let file = sessions[ti].fds.remove(&r.fd);
                    sessions[ti].fresh.retain(|t| *t != r.fd);
                    match (file, src, d.start.source) {
                        (Some(file), Some(si), Some(source)) => {
                            eprintln!("[gex] dnd: {} asks for {}: {}'s source is asked to send it", sessions[ti].id, r.mime, d.src);
                            sessions[si].socket.stage_send(vec![file]);
                            sessions[si].client.ask_source(source, &r.mime);
                            sessions[si].flush();
                        }
                        // (dropped here: the receiver reads its end)
                        _ => eprintln!("[gex] dnd: nothing to send as {}", r.mime),
                    }
                }
            }
            // Dropped: the source is told when the target is done with it.
            if let Some((_, since)) = d.dropped {
                if status.as_ref().is_none_or(|st| st.finished) {
                    eprintln!("[gex] dnd: the target is done with the drop");
                    if let (Some(si), Some(source)) = (src, d.start.source) {
                        sessions[si].client.drag_source_finished(source);
                    }
                    end = Some(true);
                } else if since.elapsed() > DROP_FINISH {
                    eprintln!("[gex] dnd: the target never finished the drop: cancelled");
                    end = Some(false);
                }
            }
        }
        if let Some(finished) = end {
            if let Some(d) = own.take() {
                Self::end_own(sessions, d, finished);
            }
        }
        // Its icon, at the pointer while the pointer is the drag's.
        {
            let mut screen = self.inner.screen.lock().unwrap();
            screen.icon = own.as_ref().filter(|d| d.dropped.is_none()).and_then(|d| {
                let s = sessions.iter().find(|s| s.id == d.src)?;
                let (icon, hx, hy) = s.client.drag_icon()?;
                let at = screen.icon_at?;
                Some(((d.src, icon), at, hx, hy))
            });
        }
        drop(own);
        if !placing {
            self.carry_host_drag(sessions, due);
        }
        for s in sessions.iter_mut() {
            s.flush();
        }
    }

    /// A client's drag is at `(x, y)` (the screen's, or the window's): it
    /// enters the surface there (leaving the one it was over), or moves on
    /// it. With placed windows the screen's hit test picks the surface; else
    /// the window `over` names, or the one the drag started in.
    fn drag_to(&self, sessions: &mut [Session], d: &mut OwnDrag, over: Option<&String>, placing: bool, (x, y): (i32, i32), ms: u32) {
        let mut screen = self.inner.screen.lock().unwrap();
        let hit = if placing {
            let h = screen.hit(x, y);
            screen.icon_at = Some(IconAt::Screen(x, y));
            h
        } else {
            let i = match over {
                Some(t) => sessions.iter().position(|s| s.client.toplevel_titled(t).is_some()),
                None => sessions.iter().position(|s| s.id == d.src),
            };
            i.and_then(|i| {
                let s = &mut sessions[i];
                let top = match over {
                    Some(t) => s.client.toplevel_titled(t),
                    None => s.client.toplevel_of(d.start.origin),
                };
                s.client.point_window(top);
                if let Some(top) = top {
                    screen.icon_at = Some(IconAt::Window((s.id, top), x, y));
                }
                s.client.window_point(x, y).map(|(surface, lx, ly)| ((s.id, surface), lx, ly))
            })
        };
        drop(screen);
        let idx = |sessions: &[Session], id: u64| sessions.iter().position(|s| s.id == id);
        match (d.over, hit) {
            (Some(o), Some((k, lx, ly))) if o == k => {
                if let Some(i) = idx(sessions, k.0) {
                    sessions[i].client.drag_motion(lx, ly, ms);
                }
            }
            (old, new) => {
                if let Some(i) = old.and_then(|o| idx(sessions, o.0)) {
                    sessions[i].client.drag_leave();
                }
                d.over = None;
                let Some((k, lx, ly)) = new else {
                    eprintln!("[gex] dnd: the drag at {x},{y} is over no window");
                    return;
                };
                let Some(i) = idx(sessions, k.0) else { return };
                if placing {
                    sessions[i].client.placement_origin(k.1, x - lx, y - ly);
                }
                let mimes: Vec<&str> = d.start.mimes.iter().map(String::as_str).collect();
                // A source below version 3 says no actions: its drops are copies.
                let actions = if d.start.actions == 0 { gex_wayland::data::DND_COPY } else { d.start.actions };
                let ok = sessions[i].client.drag_enter(k.1, lx, ly, &mimes, actions);
                eprintln!("[gex] dnd: the drag enters {}:{} at {lx},{ly}{}", k.0, k.1, if ok { "" } else { ": the client has no data device" });
                if ok {
                    d.over = Some(k);
                }
            }
        }
    }

    /// A client's drag is over: the target it is over left (a dropped one's
    /// offer is the target's until it destroys it), the source told it was
    /// cancelled unless it `finished`.
    fn end_own(sessions: &mut [Session], d: OwnDrag, finished: bool) {
        for s in sessions.iter_mut() {
            if Some(s.id) == d.target() {
                s.client.drag_leave();
            }
            if s.id == d.src {
                if !finished {
                    s.client.drag_source_cancel(d.start.source);
                }
                s.client.point_window(None);
            }
        }
    }

    /// The script's drag of files over windows nobody places: to the window
    /// `over` names, or the newest client's.
    fn carry_host_drag(&self, sessions: &mut [Session], due: &[(Ev, u32, Option<String>)]) {
        let mut hd = self.inner.host_drag.lock().unwrap();
        let idx = |sessions: &[Session], id: u64| sessions.iter().position(|s| s.id == id);
        for (ev, ms, over) in due {
            let to = match *ev {
                Ev::Drag(x, y, n) => {
                    if let Some(i) = hd.take().and_then(|h| h.1).and_then(|k| idx(sessions, k.0)) {
                        sessions[i].client.drag_leave();
                    }
                    *hd = Some((n, None));
                    Some((x, y))
                }
                Ev::DragMove(x, y) => Some((x, y)),
                Ev::Drop => {
                    match hd.take().and_then(|h| h.1).and_then(|k| idx(sessions, k.0)) {
                        Some(i) => {
                            let taken = sessions[i].client.drag_drop();
                            eprintln!("[gex] dnd: drop on {}: {}", sessions[i].id, if taken { "taken" } else { "refused" });
                        }
                        None => eprintln!("[gex] input: drop over no window"),
                    }
                    None
                }
                Ev::DragLeave => {
                    if let Some(i) = hd.take().and_then(|h| h.1).and_then(|k| idx(sessions, k.0)) {
                        sessions[i].client.drag_leave();
                    }
                    None
                }
                _ => None,
            };
            let (Some((x, y)), Some((n, was))) = (to, hd.as_mut()) else { continue };
            let i = match over {
                Some(t) => sessions.iter().position(|s| s.client.toplevel_titled(t).is_some()),
                None => sessions.iter().rposition(|s| s.client.wants_input() && s.client.has_data_device()),
            };
            let hit = i.and_then(|i| {
                let s = &mut sessions[i];
                let top = over.as_ref().and_then(|t| s.client.toplevel_titled(t));
                s.client.point_window(top);
                s.client.window_point(x, y).map(|(surface, lx, ly)| ((s.id, surface), lx, ly))
            });
            eprintln!("[gex] input: drag at {x},{y} is over {:?}", hit.map(|h| h.0));
            match (*was, hit) {
                (Some(o), Some((k, lx, ly))) if o == k => {
                    if let Some(i) = idx(sessions, k.0) {
                        sessions[i].client.drag_motion(lx, ly, *ms);
                    }
                }
                (o, h) => {
                    if let Some(i) = o.and_then(|o| idx(sessions, o.0)) {
                        sessions[i].client.drag_leave();
                    }
                    *was = None;
                    if let Some((k, lx, ly)) = h {
                        if let Some(i) = idx(sessions, k.0) {
                            let s = &mut sessions[i];
                            s.drag = self.drag_list(*n);
                            let ok = s.client.drag_enter(k.1, lx, ly, &["text/uri-list"], gex_wayland::data::DND_COPY);
                            eprintln!("[gex] dnd: enter {}:{} at {lx},{ly}{}", k.0, k.1, if ok { "" } else { ": the client has no data device" });
                            *was = ok.then_some(k);
                        }
                    }
                }
            }
        }
    }
}

thread_local! {
    /// The guest process this host thread runs, as its last pump said: a
    /// connection is accepted inside the connecting process's own `connect`.
    static PROCESS: std::cell::Cell<Option<i32>> = const { std::cell::Cell::new(None) };
}

/// A surface on the screen: its client's connection and its id there.
type Key = (u64, u32);

/// The screen, natively, once a client places windows
/// (`gex_window_placement_v1`): each surface's last frame, and the placed
/// ones where their clients put them, in the order they stacked them. Enough
/// of a window manager to check placement, stacking and click-through on a
/// host with no browser: the browser's compositor (`compositor-worker.js`)
/// is the one that matters, and does the same with the same records.
///
/// Until anything is placed it is not used, and frames are drawn as they
/// always were: each batch of commits, centred.
#[derive(Default)]
struct Screen {
    frames: HashMap<Key, Frame>,
    /// Placed surfaces: position and placement.
    placed: HashMap<Key, (i32, i32, Placement)>,
    /// Placed surfaces bottom to top, before layers are applied.
    order: Vec<Key>,
    /// The restack and activate serials already acted on.
    acted: HashMap<Key, (u32, u32)>,
    /// Who has the keyboard (by `activate`) and the pointer (by hit test).
    focus: Option<Key>,
    pointer: Option<Key>,
    /// The scripted drag: which file list, and the surface it is over.
    drag: Option<(usize, Option<Key>)>,
    /// A client's drag's icon: its surface, where the pointer is, and its
    /// hotspot; and where the pointer was last put by a drag.
    icon: Option<(Key, IconAt, i32, i32)>,
    icon_at: Option<IconAt>,
    /// The last frame of each surface that has no role: a drag's icon is
    /// drawn before its drag starts, and is no window until then.
    spare: HashMap<Key, Frame>,
    /// Sessions owed a keyboard leave: the keyboard went to another client.
    leaves: Vec<u64>,
    /// The window list last printed.
    listed: String,
}

/// Where a drag's pointer is: a point of the screen, or of a window (one
/// nobody places, drawn centred).
#[derive(Clone, Copy, Debug)]
enum IconAt {
    Screen(i32, i32),
    Window(Key, i32, i32),
}

/// Scripted input, routed by the screen to one client.
enum Routed {
    /// A placed surface's origin, which the pointer positions after it are
    /// relative to (`gex_window_placement_v1.origin`).
    Origin(u32, i32, i32),
    PointerTo(u32, i32, i32, u32),
    PointerClear,
    Button(u32, bool, u32),
    Axis(u32, i32, u32),
    Key(u32, bool, u32),
    /// The keyboard to this surface (the screen chose it, no client did).
    KeyboardFocus(u32),
    KeyboardLeave,
    /// The host's drag: enters a surface at a point with the script's Nth
    /// file list, moves on it, is dropped, leaves.
    DragEnter(u32, i32, i32, usize),
    DragMotion(i32, i32, u32),
    Drop,
    DragLeave,
}

impl Screen {
    fn active(&self) -> bool {
        !self.placed.is_empty()
    }

    /// A client hung up: its windows are gone.
    fn forget(&mut self, sid: u64) {
        self.frames.retain(|k, _| k.0 != sid);
        self.spare.retain(|k, _| k.0 != sid);
        self.placed.retain(|k, _| k.0 != sid);
        self.order.retain(|k| k.0 != sid);
        self.acted.retain(|k, _| k.0 != sid);
        if self.focus.is_some_and(|k| k.0 == sid) {
            self.focus = None;
        }
        if self.pointer.is_some_and(|k| k.0 == sid) {
            self.pointer = None;
        }
    }

    /// The placed surfaces bottom to top, layers kept apart.
    fn stack(&self) -> Vec<Key> {
        let mut v = self.order.clone();
        v.sort_by_key(|k| self.placed.get(k).map(|p| p.2.layer).unwrap_or(0));
        v
    }

    /// Take in one client's state after its requests were read.
    fn update(&mut self, sid: u64, client: &mut Client, produced: &[Frame]) {
        for f in produced {
            self.frames.insert((sid, f.surface), f.clone());
        }
        // (a surface with no role yet, kept aside: a drag's icon)
        let unmapped: Vec<Key> = self.frames.keys().filter(|k| k.0 == sid && !client.is_mapped(k.1)).copied().collect();
        for k in unmapped {
            if let Some(f) = self.frames.remove(&k) {
                self.spare.insert(k, f);
            }
        }
        self.spare.retain(|k, _| k.0 != sid || (client.has_surface(k.1) && !client.is_mapped(k.1)));
        let now = client.placed();
        self.placed.retain(|k, _| k.0 != sid || now.iter().any(|p| p.0 == k.1));
        self.order.retain(|k| self.placed.contains_key(k));
        self.acted.retain(|k, _| k.0 != sid || self.placed.contains_key(k));
        let mut restacks = Vec::new();
        for (surface, x, y, p) in now {
            let k = (sid, surface);
            self.placed.insert(k, (x, y, p));
            if !self.order.contains(&k) {
                self.order.push(k);
            }
            let (restack, activate) = self.acted.get(&k).copied().unwrap_or((0, 0));
            if p.restack != restack && p.restack != 0 {
                restacks.push((k, p.below));
            }
            if p.activate != activate && p.activate != 0 {
                if let Some(old) = self.focus.filter(|o| o.0 != sid) {
                    self.leaves.push(old.0);
                }
                self.focus = Some(k);
                client.keyboard_focus(surface);
            }
            self.acted.insert(k, (p.restack, p.activate));
        }
        for (k, below) in restacks {
            self.restack(k, below);
        }
        if self.focus.is_some_and(|f| f.0 == sid && !self.placed.contains_key(&f)) {
            self.focus = None;
        }
    }

    fn restack(&mut self, k: Key, below: u32) {
        self.order.retain(|o| *o != k);
        let at = if below == Placement::BOTTOM {
            0
        } else if below == 0 {
            self.order.len()
        } else {
            self.order
                .iter()
                .position(|o| self.placed.get(o).is_some_and(|p| p.2.id == below))
                .unwrap_or(self.order.len())
        };
        self.order.insert(at, k);
    }

    /// The topmost placed surface at a point, and the point in it.
    fn hit(&self, x: i32, y: i32) -> Option<(Key, i32, i32)> {
        for k in self.stack().into_iter().rev() {
            let (Some(&(px, py, _)), Some(f)) = (self.placed.get(&k), self.frames.get(&k)) else {
                continue;
            };
            if x >= px && x < px + f.width && y >= py && y < py + f.height {
                return Some((k, x - px, y - py));
            }
        }
        None
    }

    /// Route scripted events: the pointer to whatever placed window is under
    /// it, buttons with it, keys to the window its client activated.
    fn route(&mut self, due: &[(Ev, u32)]) -> Vec<(u64, Routed)> {
        let mut out: Vec<(u64, Routed)> = self.leaves.drain(..).map(|s| (s, Routed::KeyboardLeave)).collect();
        for &(ev, ms) in due {
            match ev {
                Ev::Motion(x, y) => {
                    let hit = self.hit(x, y);
                    if let Some(old) = self.pointer {
                        if hit.map(|h| h.0 .0) != Some(old.0) {
                            out.push((old.0, Routed::PointerClear));
                        }
                    }
                    eprintln!("[gex] input: pointer at {x},{y} is over {:?}", hit.map(|h| h.0));
                    self.pointer = hit.map(|h| h.0);
                    if let Some(((sid, surface), lx, ly)) = hit {
                        out.push((sid, Routed::Origin(surface, x - lx, y - ly)));
                        out.push((sid, Routed::PointerTo(surface, lx, ly, ms)));
                    }
                }
                Ev::Button(b, down) => {
                    if let Some((sid, _)) = self.pointer {
                        out.push((sid, Routed::Button(b, down, ms)));
                    }
                }
                Ev::Drag(x, y, n) => {
                    if let Some((_, Some(old))) = self.drag.take() {
                        out.push((old.0, Routed::DragLeave));
                    }
                    self.drag = Some((n, None));
                    self.drag_to(x, y, ms, &mut out);
                }
                Ev::DragMove(x, y) => self.drag_to(x, y, ms, &mut out),
                Ev::Drop => {
                    if let Some((_, Some(k))) = self.drag.take() {
                        out.push((k.0, Routed::Drop));
                    } else {
                        eprintln!("[gex] input: drop over no window");
                    }
                }
                Ev::DragLeave => {
                    if let Some((_, Some(k))) = self.drag.take() {
                        out.push((k.0, Routed::DragLeave));
                    }
                }
                // (for windows nobody places)
                Ev::Over(_) => {}
                Ev::Axis(axis, v) => {
                    if let Some((sid, _)) = self.pointer {
                        out.push((sid, Routed::Axis(axis, v, ms)));
                    } else {
                        eprintln!("[gex] input: scroll dropped: the pointer is over no window (axis {axis}, {v}/256 px)");
                    }
                }
                Ev::Menu(..) => {}
                Ev::Key(k, down) => {
                    // No client has activated a window (Wine activates its
                    // own, and a slow start can leave none active when the
                    // script's keys come): the topmost window gets the
                    // keyboard, as a compositor focuses the window on top.
                    if self.focus.is_none() {
                        if let Some(top) = self.stack().last().copied() {
                            eprintln!("[gex] input: no window has the keyboard; it goes to the top one, {}:{}", top.0, top.1);
                            self.focus = Some(top);
                            out.push((top.0, Routed::KeyboardFocus(top.1)));
                        }
                    }
                    if let Some((sid, _)) = self.focus {
                        out.push((sid, Routed::Key(k, down, ms)));
                    }
                }
            }
        }
        out
    }

    /// The scripted drag is at a screen point: it enters the window there
    /// (leaving the one it was over) or moves on it.
    fn drag_to(&mut self, x: i32, y: i32, ms: u32, out: &mut Vec<(u64, Routed)>) {
        let Some((n, over)) = self.drag else { return };
        let hit = self.hit(x, y);
        eprintln!("[gex] input: drag at {x},{y} is over {:?}", hit.map(|h| h.0));
        match (over, hit) {
            (Some(o), Some((k, lx, ly))) if o == k => out.push((k.0, Routed::DragMotion(lx, ly, ms))),
            (o, h) => {
                if let Some(o) = o {
                    out.push((o.0, Routed::DragLeave));
                }
                if let Some((k, lx, ly)) = h {
                    out.push((k.0, Routed::Origin(k.1, x - lx, y - ly)));
                    out.push((k.0, Routed::DragEnter(k.1, lx, ly, n)));
                }
                self.drag = Some((n, h.map(|h| h.0)));
            }
        }
    }

    /// What to draw, bottom to top: windows nobody placed centred, as
    /// before, then the placed ones where they are, then a drag's icon at
    /// the pointer. `size`: the screen's, which centres a window.
    fn items(&self, size: (u32, u32)) -> Vec<(u64, &Frame, Option<(i32, i32)>)> {
        let key = |k: &Key| (k.0 << 32) | u64::from(k.1);
        let icon = self.icon.map(|i| i.0);
        let mut loose: Vec<_> = self.frames.iter().filter(|(k, _)| !self.placed.contains_key(k) && Some(**k) != icon).collect();
        loose.sort_by_key(|(k, _)| **k);
        let mut out: Vec<_> = loose.into_iter().map(|(k, f)| (key(k), f, None)).collect();
        for k in self.stack() {
            if let (Some(f), Some(&(x, y, _))) = (self.frames.get(&k), self.placed.get(&k)) {
                out.push((key(&k), f, Some((x, y))));
            }
        }
        if let Some((k, at, hx, hy)) = self.icon {
            let point = match at {
                IconAt::Screen(x, y) => Some((x, y)),
                IconAt::Window(w, x, y) => match (self.placed.get(&w), self.frames.get(&w)) {
                    (Some(&(px, py, _)), _) => Some((px + x, py + y)),
                    (None, Some(f)) => Some((((size.0 as i32 - f.width) / 2).max(0) + x, ((size.1 as i32 - f.height) / 2).max(0) + y)),
                    _ => None,
                },
            };
            if let (Some((x, y)), Some(f)) = (point, self.frames.get(&k).or_else(|| self.spare.get(&k))) {
                out.push((key(&k), f, Some((x - hx, y - hy))));
            }
        }
        out
    }

    /// The window list, bottom to top, printed when it changes: what a test
    /// reads to check placement and stacking.
    fn list(&mut self) {
        let mut s = String::new();
        for k in self.stack() {
            let (Some(&(x, y, p)), Some(f)) = (self.placed.get(&k), self.frames.get(&k)) else {
                continue;
            };
            s += &format!(
                " {}:{} id={:#x} {}x{}+{}+{} layer={}{}{}",
                k.0, k.1, p.id, f.width, f.height, x, y, p.layer,
                // the client draws its whole window: a page compositor puts no frame round it
                if p.unmanaged { " unmanaged" } else { "" },
                if self.focus == Some(k) { " focus" } else { "" }
            );
        }
        if s != self.listed {
            eprintln!("[wl-place]{s}");
            self.listed = s;
        }
    }
}

impl HostListener for Wayland {
    fn accept(&self, peer: Arc<UnixSocket>) {
        let mut client = Client::new();
        let (w, h) = *self.inner.size.lock().unwrap();
        client.set_default_size(w, h);
        client.set_presents_menus(presents_menus());
        // Frame callbacks at the paced clock's frames (`frame_hz`).
        client.set_held(frame_hz() != 0);
        // Windows are shown bare here: no frame is taken off a window's
        // bounds, and KDE's protocol tells clients to draw their own.
        client.set_host_frames(false);
        let id = {
            let mut n = self.inner.next_session.lock().unwrap();
            *n += 1;
            *n
        };
        self.inner.sessions.lock().unwrap().push(Session {
            id,
            owner: PROCESS.with(|p| p.get()),
            socket: peer,
            client,
            fds: HashMap::new(),
            next_fd: 0,
            fresh: Vec::new(),
            out: Vec::new(),
            sent: 0,
            rbuf: Vec::new(),
            drag: None,
            drag_said: None,
        });
        self.note_client();
    }
}

impl Session {
    /// Send what the client is owed now, as far as its socket takes it.
    fn flush(&mut self) {
        self.out.extend_from_slice(&self.client.take_outgoing());
        while self.sent < self.out.len() {
            match self.socket.send(&self.out[self.sent..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => self.sent += n as usize,
            }
        }
        if self.sent == self.out.len() {
            self.out.clear();
            self.sent = 0;
        }
    }

    /// A script's menu line, if this client has the tree it names: the
    /// menu bar's, or with `dock:` the dock menu's.
    fn menu_script(&mut self, verb: MenuVerb, path: &str, mods: u32) {
        use gex_wayland::menu::Scope;
        let (scope, path) = match path.strip_prefix("dock:") {
            Some(p) => (Scope::Dock, p.trim()),
            None => (Scope::MenuBar, path),
        };
        if verb == MenuVerb::List {
            for t in self.client.menus() {
                eprintln!("[wl-menu] tree {}:{} {} {:?}", self.id, t.object, t.scope.name(), t.title());
                for line in t.describe(0, true) {
                    eprintln!("[wl-menu]   {line}");
                }
            }
            return;
        }
        let Some(t) = self.client.menus().find(|t| t.scope == scope) else { return };
        let tree = t.object;
        let Some(id) = (if path.is_empty() { Some(0) } else { t.find(path) }) else {
            eprintln!("[wl-menu] no item {path:?} in tree {}:{tree}", self.id);
            return;
        };
        let title = t.item(id).map(|i| i.title.clone()).unwrap_or_default();
        match verb {
            MenuVerb::Open => {
                self.client.menu_about_to_open(tree, id);
            }
            MenuVerb::Close => {
                eprintln!("[wl-menu] close {id} {title:?}");
                self.client.menu_closed(tree, id);
            }
            _ => {
                eprintln!("[wl-menu] activate {id} {title:?}");
                self.client.menu_activate(tree, id, mods);
            }
        }
    }

    /// A menu the client says is up to date (`ack_open`), printed: what a
    /// shell would now show.
    fn menu_answers(&mut self) {
        for u in self.client.take_menu_updates() {
            let gex_wayland::menu::Update::Opened { tree, menu, .. } = u else { continue };
            let Some(t) = self.client.menu_tree(tree) else { continue };
            eprintln!("[wl-menu] opened {menu} {:?}", t.item(menu).map_or(t.title(), |i| i.title.as_str()));
            for line in t.describe(menu, false) {
                eprintln!("[wl-menu]   {line}");
            }
        }
    }

    /// The drag's data to every client that asked (`wl_data_offer.receive`):
    /// written into its descriptor, which is then closed. And what the client
    /// says of the drag, when that changes. Not the receives of a client's
    /// own drag over this one (`own`): those go to its source (`carry`).
    fn answer_drag(&mut self, own: bool) {
        if own {
            return;
        }
        for r in self.client.take_receives() {
            let file = self.fds.remove(&r.fd);
            self.fresh.retain(|t| *t != r.fd);
            let (Some(file), Some(data)) = (file, self.drag.as_ref().filter(|_| r.mime == "text/uri-list")) else {
                eprintln!("[gex] dnd: nothing to send as {}", r.mime);
                continue;
            };
            let mut at = 0;
            while at < data.len() {
                match file.write(&data.as_bytes()[at..]) {
                    Ok(n) if n > 0 => at += n as usize,
                    _ => break,
                }
            }
            eprintln!("[gex] dnd: sent {at} of {} bytes as {}", data.len(), r.mime);
        }
        let st = self.client.drag_status();
        if st != self.drag_said {
            if let Some(st) = &st {
                eprintln!("[gex] dnd: {}:{} accepts {:?}, action {}{}{}", self.id, st.surface, st.accepted, st.action,
                    if st.dropped { ", dropped" } else { "" }, if st.finished { ", finished" } else { "" });
            }
            self.drag_said = st;
        }
    }

    /// Drop every descriptor the compositor has finished with: no pool, no
    /// buffer and no surface on screen names it, and it is not one still
    /// waiting for the message that uses it.
    fn release_unused(&mut self) {
        if self.fds.is_empty() {
            return;
        }
        let used = self.client.fds_in_use();
        let fresh = std::mem::take(&mut self.fresh);
        self.fresh = fresh.into_iter().filter(|t| !used.contains(t)).collect();
        let fresh = &self.fresh;
        self.fds.retain(|t, _| used.contains(t) || fresh.contains(t));
    }
}

/// Resolves a descriptor the client sent to the pixels behind it.
struct PoolView<'a, 'm> {
    kernel: &'a Kernel,
    mem: &'a GuestMem<'m>,
    fds: &'a HashMap<u32, Arc<OpenFile>>,
}

impl Buffers for PoolView<'_, '_> {
    fn map(&self, fd: u32, size: usize) -> Option<Vec<u8>> {
        let file = self.fds.get(&fd)?;
        // The client mapped this shared, so the live pixels are in the guest's
        // memory; the file's own bytes are what was there before it drew (zero).
        // Its inode asked before the lock, as `mmap` asks it (file I/O).
        let _ = file.ino();
        let mm = self.kernel.mem.lock();
        let map = gex_kernel::syscalls::pool_map(mm.log.shared_maps.iter(), file, size as u64).cloned()?;
        drop(mm);
        let len = size.min(map.len as usize);
        self.mem.read(map.start, len).ok().map(|b| b.to_vec())
    }

    fn read(&self, fd: u32, size: usize) -> Option<Vec<u8>> {
        let file = self.fds.get(&fd)?;
        let mut out = vec![0u8; size];
        let mut at = 0;
        while at < size {
            match file.handle.read_at(at as u64, &mut out[at..]) {
                Ok(n) if n > 0 => at += n as usize,
                _ => break,
            }
        }
        out.truncate(at);
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(s: &mut Screen, k: Key, x: i32) {
        let p = Placement { placed: true, id: k.1, ..Placement::default() };
        s.placed.insert(k, (x, 0, p));
        s.order.push(k);
        let f = Frame { surface: k.1, width: 100, height: 100, stride: 400, format: 0, pixels: Vec::new(), title: String::new() };
        s.frames.insert(k, f);
    }

    /// No client activated a window: a scripted key gives the keyboard to the
    /// top window first, then goes to it.
    #[test]
    fn key_with_no_active_window_focuses_the_top_one() {
        let mut s = Screen::default();
        window(&mut s, (1, 5), 0);
        window(&mut s, (2, 7), 200);
        let out = s.route(&[(Ev::Key(28, true), 10), (Ev::Key(28, false), 20)]);
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0], (2, Routed::KeyboardFocus(7))));
        assert!(matches!(out[1], (2, Routed::Key(28, true, 10))));
        assert!(matches!(out[2], (2, Routed::Key(28, false, 20))));
        assert_eq!(s.focus, Some((2, 7)));
    }

    /// A window its client activated keeps the keyboard.
    #[test]
    fn key_goes_to_the_activated_window() {
        let mut s = Screen::default();
        window(&mut s, (1, 5), 0);
        window(&mut s, (2, 7), 200);
        s.focus = Some((1, 5));
        let out = s.route(&[(Ev::Key(28, true), 10)]);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], (1, Routed::Key(28, true, 10))));
    }

    /// With no window on the screen, a key goes nowhere.
    #[test]
    fn key_with_no_window_is_dropped() {
        let mut s = Screen::default();
        assert!(s.route(&[(Ev::Key(28, true), 10)]).is_empty());
        assert_eq!(s.focus, None);
    }
}
