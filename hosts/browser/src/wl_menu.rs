//! Menus between a client and the page's shell (`gex_menu_v1`; the state is
//! `gex_wayland::menu`'s).
//!
//! The page says whether its shell presents menus ([`presented`]: a word of
//! the compositor's arena, set before any program runs); each client is told
//! at its connection, which is when the global is or is not advertised.
//!
//! To the page: every change is a message, JSON text, posted to the arena's
//! menu queue (`wl_menu_post`), which the page drains with its scene. A
//! tree goes whole each time it is committed ([`gex_wayland::menu::Tree::json`]);
//! its images go once each, before the first tree that shows them;
//! `{"client":C,"gone":T}` when a tree goes (`T` 0: every tree of the
//! client, which hung up); `{"client":C,"opened":{..}}` when the client
//! answers an `about_to_open`. A message the queue has no room for waits
//! here and goes with a later pump, in order.
//!
//! From the page: three kinds of input event ([`EV_MENU_OPEN`] and the two
//! after it), addressed to a client like any other.

use super::Session;
use gex_wayland::menu::Update;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;

#[link(wasm_import_module = "gex_env")]
extern "C" {
    /// Whether the page's shell presents menus (`HostWindows`'s `menus`).
    fn wl_menus() -> u32;
    /// One message for the page: `len` bytes of JSON at `ptr`. 0 when the
    /// queue has no room now (post it again later), else 1.
    fn wl_menu_post(ptr: u32, len: u32) -> u32;
}

/// The page is about to show `b`, a menu of tree `a`; `c` is the page's own
/// number for the asking, handed back in the `opened` message.
pub(super) const EV_MENU_OPEN: i32 = 32;
/// The page stopped showing menu `b` of tree `a`.
pub(super) const EV_MENU_CLOSED: i32 = 33;
/// Item `b` of tree `a` was chosen with `gex_menu_v1.modifiers` `c` held.
pub(super) const EV_MENU_ACTIVATE: i32 = 34;

/// Whether clients are offered `gex_menu_manager_v1`.
pub(super) fn presented() -> bool {
    super::have_wl() && unsafe { wl_menus() } != 0
}

/// Messages not yet posted. The process's, not a session's: a client's
/// going is said after its session is dropped.
static QUEUE: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Post what waits, oldest first, for as long as the page's queue takes it.
fn flush() {
    let mut q = QUEUE.lock().unwrap();
    while let Some(m) = q.front() {
        if unsafe { wl_menu_post(m.as_ptr() as u32, m.len() as u32) } == 0 {
            break;
        }
        q.pop_front();
    }
}

/// A session's menus, as the page has been told of them.
#[derive(Default)]
pub(super) struct SessionMenus {
    /// Images already sent, by serial.
    images: HashSet<u32>,
    /// `about_to_open` serials in flight, and the page's number for each.
    opens: HashMap<u32, u32>,
    /// Whether the page was told of any tree (so that it is told they went).
    told: bool,
}

/// A menu event from the page, for the client numbered `idx`.
pub(super) fn input(client: &mut gex_wayland::Client, menus: &mut SessionMenus, idx: u32, kind: i32, tree: u32, b: u32, c: u32) {
    match kind {
        EV_MENU_OPEN => match client.menu_about_to_open(tree, b) {
            Some(serial) => {
                menus.opens.insert(serial, c);
            }
            // No such tree (gone since the page asked): answered at once,
            // so the page's wait ends.
            None => QUEUE.lock().unwrap().push_back(opened(idx, tree, b, c)),
        },
        EV_MENU_CLOSED => client.menu_closed(tree, b),
        EV_MENU_ACTIVATE => client.menu_activate(tree, b, c),
        _ => {}
    }
}

impl Session {
    /// What the client's requests changed, to the page. `stays`: the
    /// session goes on after this pump.
    pub(super) fn menu_pump(&mut self, stays: bool) {
        let updates = self.client.take_menu_updates();
        if !updates.is_empty() || !stays {
            let mut q = QUEUE.lock().unwrap();
            for u in updates {
                match u {
                    Update::Changed(tree) => {
                        let Some(t) = self.client.menu_tree(tree) else { continue };
                        for image in t.images() {
                            if self.menus.images.insert(image.serial) {
                                q.push_back(format!("{{\"client\":{},\"picture\":{}}}", self.idx, image.json()));
                            }
                        }
                        q.push_back(t.json(self.idx));
                        self.menus.told = true;
                    }
                    Update::Destroyed(tree) => q.push_back(format!("{{\"client\":{},\"gone\":{tree}}}", self.idx)),
                    Update::Opened { tree, menu, serial } => {
                        if let Some(asked) = self.menus.opens.remove(&serial) {
                            q.push_back(opened(self.idx, tree, menu, asked));
                        }
                    }
                }
            }
            if !stays && self.menus.told {
                q.push_back(format!("{{\"client\":{},\"gone\":0}}", self.idx));
            }
        }
        flush();
    }
}

fn opened(client: u32, tree: u32, menu: u32, asked: u32) -> String {
    format!("{{\"client\":{client},\"opened\":{{\"tree\":{tree},\"menu\":{menu},\"serial\":{asked}}}}}")
}
