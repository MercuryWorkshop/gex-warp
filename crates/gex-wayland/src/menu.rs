//! Menus a client hands to the shell: `gex_menu_manager_v1` and `gex_menu_v1`
//! (`protocols/gex-menu-v1.xml`, where the design is written down).
//!
//! A client publishes a tree of items per menu bar -- its application's, or
//! one toplevel's -- and the host's shell draws it: the page's own menu bar,
//! in a browser. The global is advertised only when the host says its shell
//! presents menus ([`Client::set_presents_menus`], before the client asks
//! for the registry), which is the whole negotiation: a client that does not
//! see it draws its menus itself, as before.
//!
//! What a host does with it: after each batch of requests,
//! [`Client::take_menu_updates`] says which trees changed, went away, or
//! answered an `about_to_open`; [`Client::menus`] are the trees as last
//! committed, plain data ([`Tree::json`] for a host that carries them to a
//! page, [`Tree::describe`] for one that prints them). The shell's side is
//! [`Client::menu_about_to_open`], [`Client::menu_closed`] and
//! [`Client::menu_activate`].

use crate::wire::{Reader, Writer};
use crate::{Buffers, Client, Object};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

/// The version of both interfaces offered.
pub const VERSION: u32 = 1;
/// `gex_menu_manager_v1`, as the registry names it.
pub const GLOBAL: &str = "gex_menu_manager_v1";

/// `gex_menu_v1.flags`.
pub const DISABLED: u32 = 1;
pub const HIDDEN: u32 = 2;
pub const SEPARATOR: u32 = 4;
pub const SUBMENU: u32 = 8;
pub const ALTERNATE: u32 = 16;
pub const HEADER: u32 = 32;

/// `gex_menu_v1.modifiers`.
pub const SHIFT: u32 = 1;
pub const CONTROL: u32 = 2;
pub const ALT: u32 = 4;
pub const LOGO: u32 = 8;

/// The longest side of an item's image, in pixels, and the most items a
/// tree may hold.
pub const IMAGE_MAX: u32 = 128;
pub const ITEMS_MAX: usize = 4096;

mod manager_error {
    pub const ROLE_TAKEN: u32 = 0;
    pub const SURFACE_TAKEN: u32 = 1;
    pub const BAD_KIND: u32 = 2;
}

mod menu_error {
    pub const BAD_ITEM: u32 = 0;
    pub const BAD_TREE: u32 = 1;
    pub const BAD_IMAGE: u32 = 2;
    pub const TOO_LARGE: u32 = 3;
    pub const BAD_SERIAL: u32 = 4;
}

/// What a tree belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The client's menu bar: shown while any of its toplevels has the
    /// keyboard (and has no menu of its own).
    MenuBar,
    /// The menu a shell shows for the application's icon.
    Dock,
    /// One toplevel's menu bar: the `wl_surface` id.
    Surface(u32),
}

impl Scope {
    /// As a host's listing and a page's model name it.
    pub fn name(self) -> &'static str {
        match self {
            Scope::MenuBar => "menu-bar",
            Scope::Dock => "dock",
            Scope::Surface(_) => "window",
        }
    }
}

/// An item's picture, as the client sent it.
#[derive(Debug, PartialEq, Eq)]
pub struct Image {
    /// A number for these pixels, unique within the client: a host that has
    /// carried an image once need not carry it again.
    pub serial: u32,
    pub width: u32,
    pub height: u32,
    pub scale: u32,
    /// Only the alpha means anything.
    pub template: bool,
    /// `width * height` pixels, `wl_shm` argb8888 (premultiplied, the
    /// guest's byte order: little-endian, so B, G, R, A in memory).
    pub argb: Vec<u8>,
}

impl Image {
    /// The pixels as a page's `ImageData` wants them: R, G, B, A, not
    /// premultiplied.
    pub fn rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.argb.len());
        for p in self.argb.chunks_exact(4) {
            let a = u32::from(p[3]);
            let un = |c: u8| if a == 0 { 0 } else { (u32::from(c) * 255 / a).min(255) as u8 };
            out.extend_from_slice(&[un(p[2]), un(p[1]), un(p[0]), p[3]]);
        }
        out
    }

    /// `{"image":serial,"width":..,"height":..,"scale":..,"template":..,"rgba":"<base64>"}`.
    pub fn json(&self) -> String {
        format!(
            "{{\"image\":{},\"width\":{},\"height\":{},\"scale\":{},\"template\":{},\"rgba\":\"{}\"}}",
            self.serial,
            self.width,
            self.height,
            self.scale,
            self.template,
            base64(&self.rgba())
        )
    }
}

/// One item, as last described.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Item {
    /// `gex_menu_v1.flags` bits.
    pub flags: u32,
    /// 0 off, 1 on, 2 mixed.
    pub state: u32,
    pub indent: u32,
    pub title: String,
    /// The shortcut's key (a W3C `key` value), empty for none, and its
    /// modifiers.
    pub key: String,
    pub modifiers: u32,
    pub role: String,
    pub tooltip: String,
    pub image: Option<Arc<Image>>,
}

impl Item {
    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }

    /// The shortcut as a listing writes it: `logo+shift+s`.
    pub fn shortcut(&self) -> Option<String> {
        if self.key.is_empty() {
            return None;
        }
        let mut s = String::new();
        for (bit, name) in [(CONTROL, "control"), (ALT, "alt"), (SHIFT, "shift"), (LOGO, "logo")] {
            if self.modifiers & bit != 0 {
                s += name;
                s.push('+');
            }
        }
        Some(s + if self.key == " " { "Space" } else { &self.key })
    }
}

#[derive(Clone, Debug, Default)]
struct Model {
    title: String,
    items: HashMap<u32, Item>,
    children: HashMap<u32, Vec<u32>>,
}

/// One `gex_menu_v1`: a tree of menus.
#[derive(Debug)]
pub struct Tree {
    /// The `gex_menu_v1` object's id, which is the host's handle for it.
    pub object: u32,
    pub scope: Scope,
    /// The state the shell shows: `None` until the first commit.
    committed: Option<Model>,
    /// The state being described, from the first pending request to commit.
    pending: Option<Model>,
    /// `about_to_open` events not yet acknowledged: serial and menu.
    opens: Vec<(u32, u32)>,
}

static NO_ITEMS: [u32; 0] = [];

impl Tree {
    /// Whether the client has committed it: a tree is not shown before.
    pub fn published(&self) -> bool {
        self.committed.is_some()
    }

    pub fn title(&self) -> &str {
        self.committed.as_ref().map_or("", |m| m.title.as_str())
    }

    pub fn item(&self, id: u32) -> Option<&Item> {
        self.committed.as_ref()?.items.get(&id)
    }

    /// The children of `menu` (0: the root), in order. None for an item
    /// that is not a submenu.
    pub fn children(&self, menu: u32) -> &[u32] {
        let Some(m) = self.committed.as_ref() else { return &NO_ITEMS };
        if menu != 0 && !m.items.get(&menu).is_some_and(|i| i.has(SUBMENU)) {
            return &NO_ITEMS;
        }
        m.children.get(&menu).map_or(&NO_ITEMS, |v| v.as_slice())
    }

    /// Every image the tree shows.
    pub fn images(&self) -> Vec<Arc<Image>> {
        let mut out = Vec::new();
        self.walk(0, 0, &mut |_, _, item| out.extend(item.image.clone()));
        out
    }

    /// Every item reachable from `menu`, parents before children, with its
    /// depth below `menu`.
    fn walk<'a>(&'a self, menu: u32, depth: usize, f: &mut dyn FnMut(usize, u32, &'a Item)) {
        for &id in self.children(menu) {
            let Some(item) = self.item(id) else { continue };
            f(depth, id, item);
            self.walk(id, depth + 1, f);
        }
    }

    /// The item a script names: a number, or the titles down from the root
    /// separated by `>` (`Window > Bring All to Front`). The root is `0`.
    pub fn find(&self, path: &str) -> Option<u32> {
        let path = path.trim();
        if let Ok(id) = path.parse::<u32>() {
            return (id == 0 || self.item(id).is_some()).then_some(id);
        }
        let mut at = 0;
        for name in path.split('>') {
            let name = name.trim();
            at = *self.children(at).iter().find(|c| self.item(**c).is_some_and(|i| i.title == name))?;
        }
        Some(at)
    }

    /// One line an item, for a host that prints: `id "title"` and what is
    /// set, indented by depth, the children of `menu` and everything below
    /// (`deep`) or only they.
    pub fn describe(&self, menu: u32, deep: bool) -> Vec<String> {
        let mut out = Vec::new();
        let mut line = |depth: usize, id: u32, item: &Item| {
            let mut s = format!("{}{id} ", "  ".repeat(depth));
            if item.has(SEPARATOR) {
                s += "---";
            } else {
                s += &format!("{:?}", item.title);
            }
            if let Some(k) = item.shortcut() {
                s += &format!(" {k}");
            }
            for (bit, name) in [(SUBMENU, "submenu"), (DISABLED, "disabled"), (HIDDEN, "hidden"), (ALTERNATE, "alternate"), (HEADER, "header")] {
                if item.has(bit) {
                    s += &format!(" {name}");
                }
            }
            match item.state {
                1 => s += " on",
                2 => s += " mixed",
                _ => {}
            }
            if item.indent != 0 {
                s += &format!(" indent={}", item.indent);
            }
            if !item.role.is_empty() {
                s += &format!(" role={}", item.role);
            }
            if let Some(i) = &item.image {
                s += &format!(" image={}x{}@{}{}", i.width, i.height, i.scale, if i.template { " template" } else { "" });
            }
            if !item.tooltip.is_empty() {
                s += &format!(" tip={:?}", item.tooltip);
            }
            out.push(s);
        };
        if deep {
            self.walk(menu, 0, &mut line);
        } else {
            for &id in self.children(menu) {
                if let Some(item) = self.item(id) {
                    line(0, id, item);
                }
            }
        }
        out
    }

    /// The whole tree as JSON, for a host whose shell is a page:
    ///
    /// ```text
    /// {"client":C,"tree":T,"scope":"menu-bar"|"dock"|"window","surface":S,
    ///  "title":"..","items":[ITEM..]}
    /// ITEM = {"id":N,"title":"..","flags":F,"state":S,"indent":I,
    ///         "key":"..","modifiers":M,"role":"..","tooltip":"..",
    ///         "image":SERIAL,"items":[ITEM..]}
    /// ```
    ///
    /// `key`, `role`, `tooltip` and `image` only when set, `items` only on a
    /// submenu. An image is named by its serial; its pixels are
    /// [`Image::json`]'s, carried once.
    pub fn json(&self, client: u32) -> String {
        let mut s = format!(
            "{{\"client\":{client},\"tree\":{},\"scope\":\"{}\",\"surface\":{},\"title\":",
            self.object,
            self.scope.name(),
            if let Scope::Surface(s) = self.scope { s } else { 0 }
        );
        json_string(&mut s, self.title());
        s += ",\"items\":";
        self.json_items(0, &mut s);
        s.push('}');
        s
    }

    fn json_items(&self, menu: u32, s: &mut String) {
        s.push('[');
        let mut first = true;
        for &id in self.children(menu) {
            let Some(item) = self.item(id) else { continue };
            if !first {
                s.push(',');
            }
            first = false;
            *s += &format!("{{\"id\":{id},\"title\":");
            json_string(s, &item.title);
            *s += &format!(",\"flags\":{},\"state\":{},\"indent\":{}", item.flags, item.state, item.indent);
            if !item.key.is_empty() {
                *s += ",\"key\":";
                json_string(s, &item.key);
                *s += &format!(",\"modifiers\":{}", item.modifiers);
            }
            if !item.role.is_empty() {
                *s += ",\"role\":";
                json_string(s, &item.role);
            }
            if !item.tooltip.is_empty() {
                *s += ",\"tooltip\":";
                json_string(s, &item.tooltip);
            }
            if let Some(i) = &item.image {
                *s += &format!(",\"image\":{}", i.serial);
            }
            if item.has(SUBMENU) {
                *s += ",\"items\":";
                self.json_items(id, s);
            }
            s.push('}');
        }
        s.push(']');
    }
}

/// `text` as a JSON string, appended.
fn json_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// What changed in a client's menus since a host last asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Update {
    /// The tree (its `gex_menu_v1` id) was committed: published, or changed.
    Changed(u32),
    /// The tree is gone: destroyed, or its surface was.
    Destroyed(u32),
    /// The client answered an `about_to_open`: `menu` of `tree` is current.
    Opened { tree: u32, menu: u32, serial: u32 },
}

/// A client's menus.
#[derive(Debug, Default)]
pub(crate) struct MenuState {
    /// The host's shell presents menus: the global is advertised.
    presents: bool,
    trees: BTreeMap<u32, Tree>,
    updates: Vec<Update>,
    next_serial: u32,
    next_image: u32,
}

impl Client {
    /// Whether the host's shell presents menus, so that
    /// `gex_menu_manager_v1` is advertised. Said before the client asks for
    /// the registry; off unless said.
    pub fn set_presents_menus(&mut self, on: bool) {
        self.menus.presents = on;
    }

    /// The registry name of the menu global: the one after the fixed list's.
    fn menu_global_name() -> u32 {
        crate::GLOBALS.len() as u32 + 1
    }

    /// `wl_registry.global` for the menu manager, when the shell presents
    /// menus.
    pub(crate) fn advertise_menus(&mut self, registry: u32) {
        if self.menus.presents {
            self.send(Writer::new(registry, 0).uint(Self::menu_global_name()).string(GLOBAL).uint(VERSION));
        }
    }

    /// `wl_registry.bind` of the menu manager: whether this was it.
    pub(crate) fn bind_menus(&mut self, name: u32, interface: &str, id: u32) -> bool {
        if !self.menus.presents || name != Self::menu_global_name() || interface != GLOBAL {
            return false;
        }
        self.objects.insert(id, Object::MenuManager);
        true
    }

    /// `gex_menu_manager_v1`: destroy(0), get_application_menu(1: new_id,
    /// kind), get_surface_menu(2: new_id, surface).
    pub(crate) fn menu_manager(&mut self, object: u32, opcode: u16, r: &mut Reader<'_>) -> bool {
        let scope = match opcode {
            0 => {
                self.objects.remove(&object);
                self.delete_id(object);
                return true;
            }
            1 => {
                let (Some(id), Some(kind)) = (r.new_id(), r.uint()) else { return false };
                let scope = match kind {
                    1 => Scope::MenuBar,
                    2 => Scope::Dock,
                    _ => {
                        self.error(object, manager_error::BAD_KIND, &format!("gex_menu_manager_v1: no kind {kind}"));
                        return true;
                    }
                };
                if self.menus.trees.values().any(|t| t.scope == scope) {
                    self.error(object, manager_error::ROLE_TAKEN, "gex_menu_manager_v1: the client has that application menu already");
                    return true;
                }
                (id, scope)
            }
            2 => {
                let (Some(id), Some(surface)) = (r.new_id(), r.object()) else { return false };
                if self.menus.trees.values().any(|t| t.scope == Scope::Surface(surface)) {
                    self.error(object, manager_error::SURFACE_TAKEN, "gex_menu_manager_v1: the surface has a menu already");
                    return true;
                }
                (id, Scope::Surface(surface))
            }
            _ => return false,
        };
        let (id, scope) = scope;
        self.objects.insert(id, Object::Menu);
        self.menus.trees.insert(id, Tree { object: id, scope, committed: None, pending: None, opens: Vec::new() });
        true
    }

    /// `gex_menu_v1`: destroy(0), set_title(1), item(2), item_shortcut(3),
    /// item_role(4), item_tooltip(5), item_image(6), item_no_image(7),
    /// remove_item(8), set_items(9), commit(10), ack_open(11).
    pub(crate) fn menu(&mut self, object: u32, opcode: u16, r: &mut Reader<'_>, buffers: &dyn Buffers) -> bool {
        if opcode == 0 {
            if let Some(t) = self.menus.trees.remove(&object) {
                if t.published() {
                    self.menus.updates.push(Update::Destroyed(object));
                }
            }
            self.objects.remove(&object);
            self.delete_id(object);
            return true;
        }
        // Read the request whole before touching the tree, so a malformed
        // one changes nothing.
        let req = match opcode {
            1 => match r.string() {
                Some(t) => Req::Title(t),
                None => return false,
            },
            2 => {
                let (Some(id), Some(flags), Some(state), Some(indent), Some(title)) = (r.uint(), r.uint(), r.uint(), r.uint(), r.string()) else {
                    return false;
                };
                Req::Item(id, Item { flags, state: state.min(2), indent, title, ..Item::default() })
            }
            3 => {
                let (Some(id), Some(key), Some(mods)) = (r.uint(), r.string(), r.uint()) else { return false };
                Req::Shortcut(id, key, mods & 15)
            }
            4 => {
                let (Some(id), Some(role)) = (r.uint(), r.string()) else { return false };
                Req::Role(id, role)
            }
            5 => {
                let (Some(id), Some(text)) = (r.uint(), r.string()) else { return false };
                Req::Tooltip(id, text)
            }
            6 => {
                let (Some(id), Some(fd), Some(w), Some(h), Some(scale), Some(flags)) = (r.uint(), r.fd(), r.uint(), r.uint(), r.uint(), r.uint()) else {
                    return false;
                };
                let size = (w as usize) * (h as usize) * 4;
                let pixels = if w == 0 || h == 0 || w > IMAGE_MAX || h > IMAGE_MAX { None } else { buffers.map(fd, size) };
                let Some(mut argb) = pixels.filter(|p| p.len() >= size) else {
                    self.error(object, menu_error::BAD_IMAGE, &format!("gex_menu_v1: no {w}x{h} image in the file"));
                    return true;
                };
                argb.truncate(size);
                self.menus.next_image += 1;
                let image = Image { serial: self.menus.next_image, width: w, height: h, scale: scale.max(1), template: flags & 1 != 0, argb };
                Req::Image(id, Some(Arc::new(image)))
            }
            7 => match r.uint() {
                Some(id) => Req::Image(id, None),
                None => return false,
            },
            8 => match r.uint() {
                Some(id) => Req::Remove(id),
                None => return false,
            },
            9 => {
                let (Some(menu), Some(items)) = (r.uint(), r.array()) else { return false };
                if items.len() % 4 != 0 {
                    return false;
                }
                Req::Items(menu, items.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
            }
            10 => Req::Commit,
            11 => match r.uint() {
                Some(serial) => Req::Ack(serial),
                None => return false,
            },
            _ => return false,
        };
        let menus = &mut self.menus;
        let Some(tree) = menus.trees.get_mut(&object) else { return false };
        let fail = match req {
            Req::Commit => {
                let m = tree.pending.take().or_else(|| tree.committed.clone()).unwrap_or_default();
                match check(&m) {
                    Ok(()) => {
                        tree.committed = Some(m);
                        menus.updates.push(Update::Changed(object));
                        None
                    }
                    Err(e) => Some(e),
                }
            }
            Req::Ack(serial) => {
                if tree.opens.iter().any(|o| o.0 == serial) {
                    let (done, rest): (Vec<_>, Vec<_>) = tree.opens.iter().partition(|o| o.0 <= serial);
                    tree.opens = rest;
                    for (s, menu) in done {
                        menus.updates.push(Update::Opened { tree: object, menu, serial: s });
                    }
                    None
                } else {
                    Some((menu_error::BAD_SERIAL, format!("gex_menu_v1: no about_to_open had serial {serial}")))
                }
            }
            // The pending state starts as a copy of what is committed.
            req => {
                let committed = &tree.committed;
                req.apply(tree.pending.get_or_insert_with(|| committed.clone().unwrap_or_default()))
            }
        };
        if let Some((code, message)) = fail {
            self.error(object, code, &message);
        }
        true
    }

    /// The trees a shell shows: committed, and (a surface's) of a surface
    /// that is still there.
    pub fn menus(&self) -> impl Iterator<Item = &Tree> {
        self.menus.trees.values().filter(|t| t.published() && self.menu_alive(t))
    }

    /// One of them, by its `gex_menu_v1` id.
    pub fn menu_tree(&self, tree: u32) -> Option<&Tree> {
        self.menus.trees.get(&tree).filter(|t| t.published() && self.menu_alive(t))
    }

    fn menu_alive(&self, t: &Tree) -> bool {
        match t.scope {
            Scope::Surface(s) => self.surfaces.contains_key(&s),
            _ => true,
        }
    }

    /// What changed since the last call, in order. A tree whose surface was
    /// destroyed is `Destroyed` here, once.
    pub fn take_menu_updates(&mut self) -> Vec<Update> {
        let orphans: Vec<u32> =
            self.menus.trees.values().filter(|t| !self.menu_alive(t)).map(|t| t.object).collect();
        for object in orphans {
            // The object stays (the client destroys it); the tree is a new,
            // unpublished one, so nothing more is said of it.
            if let Some(t) = self.menus.trees.get_mut(&object) {
                let was = t.published();
                t.scope = Scope::Surface(0);
                t.committed = None;
                t.pending = None;
                if was {
                    self.menus.updates.push(Update::Destroyed(object));
                }
            }
        }
        std::mem::take(&mut self.menus.updates)
    }

    /// The shell is about to show `menu` (0: the root) of `tree`:
    /// `about_to_open`. The serial the client's answer will carry
    /// ([`Update::Opened`]), or `None` when there is no such tree.
    pub fn menu_about_to_open(&mut self, tree: u32, menu: u32) -> Option<u32> {
        self.menus.next_serial += 1;
        let serial = self.menus.next_serial;
        self.menus.trees.get_mut(&tree)?.opens.push((serial, menu));
        self.send(Writer::new(tree, 0).uint(menu).uint(serial));
        Some(serial)
    }

    /// The shell stopped showing `menu`: `closed`.
    pub fn menu_closed(&mut self, tree: u32, menu: u32) {
        if self.menus.trees.contains_key(&tree) {
            self.send(Writer::new(tree, 1).uint(menu));
        }
    }

    /// An item was chosen, with these `gex_menu_v1.modifiers` held:
    /// `activate`.
    pub fn menu_activate(&mut self, tree: u32, item: u32, modifiers: u32) {
        if self.menus.trees.contains_key(&tree) {
            self.send(Writer::new(tree, 2).uint(item).uint(modifiers & 15));
        }
    }
}

/// A `gex_menu_v1` request, read.
enum Req {
    Title(String),
    Item(u32, Item),
    Shortcut(u32, String, u32),
    Role(u32, String),
    Tooltip(u32, String),
    Image(u32, Option<Arc<Image>>),
    Remove(u32),
    Items(u32, Vec<u32>),
    Commit,
    Ack(u32),
}

impl Req {
    /// Change the pending state; the protocol error, if it is one.
    fn apply(self, m: &mut Model) -> Option<(u32, String)> {
        let no_item = |id: u32| Some((menu_error::BAD_ITEM, format!("gex_menu_v1: no item {id}")));
        match self {
            Req::Title(t) => m.title = t,
            Req::Item(0, _) => return Some((menu_error::BAD_ITEM, "gex_menu_v1: 0 is the root, not an item".to_string())),
            Req::Item(id, new) => {
                let old = m.items.remove(&id).unwrap_or_default();
                m.items.insert(id, Item { flags: new.flags, state: new.state, indent: new.indent, title: new.title, ..old });
            }
            Req::Shortcut(id, key, mods) => {
                let Some(i) = m.items.get_mut(&id) else { return no_item(id) };
                i.modifiers = if key.is_empty() { 0 } else { mods };
                i.key = key;
            }
            Req::Role(id, role) => match m.items.get_mut(&id) {
                Some(i) => i.role = role,
                None => return no_item(id),
            },
            Req::Tooltip(id, text) => match m.items.get_mut(&id) {
                Some(i) => i.tooltip = text,
                None => return no_item(id),
            },
            Req::Image(id, image) => match m.items.get_mut(&id) {
                Some(i) => i.image = image,
                None => return no_item(id),
            },
            Req::Remove(id) => {
                if m.items.remove(&id).is_none() {
                    return no_item(id);
                }
                m.children.remove(&id);
                for c in m.children.values_mut() {
                    c.retain(|x| *x != id);
                }
            }
            Req::Items(menu, items) => {
                m.children.insert(menu, items);
            }
            Req::Commit | Req::Ack(_) => {}
        }
        None
    }
}

/// A pending state is a tree: every child described, in one menu, and not
/// inside itself; and not too large.
fn check(m: &Model) -> Result<(), (u32, String)> {
    if m.items.len() > ITEMS_MAX {
        return Err((menu_error::TOO_LARGE, format!("gex_menu_v1: {} items", m.items.len())));
    }
    let mut seen = HashSet::new();
    for (menu, children) in &m.children {
        if *menu != 0 && !m.items.contains_key(menu) {
            // Children set for an item never described: nothing shows them.
            continue;
        }
        for c in children {
            if !m.items.contains_key(c) {
                return Err((menu_error::BAD_TREE, format!("gex_menu_v1: menu {menu} has a child {c} that was never described")));
            }
            if !seen.insert(*c) {
                return Err((menu_error::BAD_TREE, format!("gex_menu_v1: item {c} is in two menus")));
            }
        }
    }
    // Each item has at most one parent now; a loop is a chain of parents
    // that never reaches the root or an item in no menu.
    let parent: HashMap<u32, u32> = m
        .children
        .iter()
        .filter(|(menu, _)| **menu == 0 || m.items.contains_key(menu))
        .flat_map(|(menu, cs)| cs.iter().map(move |c| (*c, *menu)))
        .collect();
    for start in parent.keys() {
        let (mut at, mut steps) = (*start, 0);
        while let Some(p) = parent.get(&at) {
            at = *p;
            steps += 1;
            if at == 0 {
                break;
            }
            if steps > m.items.len() {
                return Err((menu_error::BAD_TREE, format!("gex_menu_v1: item {start} is inside itself")));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::header;

    struct Files(Vec<u8>);
    impl Buffers for Files {
        fn map(&self, _fd: u32, size: usize) -> Option<Vec<u8>> {
            Some(self.0[..size.min(self.0.len())].to_vec())
        }
    }

    fn messages(bytes: &[u8]) -> Vec<(u32, u16, Vec<u8>)> {
        let mut out = Vec::new();
        let mut at = 0;
        while let Some(h) = header(&bytes[at..]) {
            let size = h.size as usize;
            out.push((h.object, h.opcode, bytes[at + 8..at + size].to_vec()));
            at += size;
        }
        out
    }

    /// The globals a client is told of, by name.
    fn globals(c: &mut Client) -> Vec<(u32, String)> {
        c.receive(&Writer::new(1, 1).new_id(2).finish(), &[], &Files(Vec::new()));
        messages(&c.take_outgoing())
            .iter()
            .map(|(_, _, body)| {
                let mut fds = Vec::new();
                let mut r = Reader::new(body, &mut fds);
                (r.uint().unwrap(), r.string().unwrap())
            })
            .collect()
    }

    /// A client with the manager bound as object 3 and a menu bar tree as 4.
    fn client() -> Client {
        let mut c = Client::new();
        c.set_presents_menus(true);
        let name = globals(&mut c).into_iter().find(|g| g.1 == GLOBAL).expect("advertised").0;
        send(&mut c, Writer::new(2, 0).uint(name).string(GLOBAL).uint(1).new_id(3));
        send(&mut c, Writer::new(3, 1).new_id(4).uint(1));
        assert!(c.failed.is_none(), "{:?}", c.failed);
        c
    }

    fn send(c: &mut Client, w: Writer) {
        c.receive(&w.finish(), &[], &Files(Vec::new()));
    }

    fn item(c: &mut Client, id: u32, flags: u32, title: &str) {
        send(c, Writer::new(4, 2).uint(id).uint(flags).uint(0).uint(0).string(title));
    }

    fn items(c: &mut Client, menu: u32, ids: &[u32]) {
        let bytes: Vec<u8> = ids.iter().flat_map(|i| i.to_le_bytes()).collect();
        send(c, Writer::new(4, 9).uint(menu).array(&bytes));
    }

    fn commit(c: &mut Client) {
        send(c, Writer::new(4, 10));
    }

    /// File and Edit, with Save (logo+s), a separator and Close under File.
    fn published() -> Client {
        let mut c = client();
        send(&mut c, Writer::new(4, 1).string("Gallery"));
        item(&mut c, 1, SUBMENU, "File");
        item(&mut c, 2, SUBMENU, "Edit");
        item(&mut c, 10, 0, "Save");
        item(&mut c, 11, SEPARATOR, "");
        item(&mut c, 12, DISABLED, "Close");
        send(&mut c, Writer::new(4, 3).uint(10).string("s").uint(LOGO));
        send(&mut c, Writer::new(4, 4).uint(10).string("save"));
        items(&mut c, 0, &[1, 2]);
        items(&mut c, 1, &[10, 11, 12]);
        commit(&mut c);
        assert!(c.failed.is_none(), "{:?}", c.failed);
        c
    }

    #[test]
    fn the_global_is_advertised_only_when_the_shell_presents_menus() {
        let mut c = Client::new();
        assert!(globals(&mut c).iter().all(|g| g.1 != GLOBAL));
        // And cannot be bound by guessing its name.
        send(&mut c, Writer::new(2, 0).uint(crate::GLOBALS.len() as u32 + 1).string(GLOBAL).uint(1).new_id(3));
        assert!(c.failed.is_some());

        let mut c = Client::new();
        c.set_presents_menus(true);
        let g = globals(&mut c);
        assert_eq!(g.last().map(|g| g.1.as_str()), Some(GLOBAL));
        assert_eq!(g.len(), crate::GLOBALS.len() + 1, "and every other global as before");
    }

    #[test]
    fn nothing_is_shown_before_the_first_commit() {
        let mut c = client();
        item(&mut c, 1, SUBMENU, "File");
        items(&mut c, 0, &[1]);
        assert_eq!(c.menus().count(), 0);
        assert!(c.take_menu_updates().is_empty());
        commit(&mut c);
        assert_eq!(c.take_menu_updates(), vec![Update::Changed(4)]);
        let t = c.menu_tree(4).expect("published");
        assert_eq!(t.scope, Scope::MenuBar);
        assert_eq!(t.children(0), &[1]);
    }

    #[test]
    fn a_tree_is_published_with_its_items_in_order() {
        let c = published();
        let t = c.menu_tree(4).unwrap();
        assert_eq!(t.title(), "Gallery");
        assert_eq!(t.children(0), &[1, 2]);
        assert_eq!(t.children(1), &[10, 11, 12]);
        assert_eq!(t.children(2), &[] as &[u32]);
        assert_eq!(t.children(10), &[] as &[u32], "not a submenu");
        let save = t.item(10).unwrap();
        assert_eq!((save.title.as_str(), save.key.as_str(), save.modifiers, save.role.as_str()), ("Save", "s", LOGO, "save"));
        assert_eq!(save.shortcut().as_deref(), Some("logo+s"));
        assert_eq!(
            t.describe(0, true),
            vec![
                "1 \"File\" submenu",
                "  10 \"Save\" logo+s role=save",
                "  11 ---",
                "  12 \"Close\" disabled",
                "2 \"Edit\" submenu",
            ]
        );
        assert_eq!(t.describe(1, false).len(), 3);
        assert_eq!(t.find("File > Close"), Some(12));
        assert_eq!(t.find("12"), Some(12));
        assert_eq!(t.find("0"), Some(0));
        assert_eq!(t.find("File > Open"), None);
        assert_eq!(t.find("99"), None);
    }

    #[test]
    fn changes_wait_for_commit_and_leave_the_rest_alone() {
        let mut c = published();
        c.take_menu_updates();
        // Close becomes enabled and checked; its shortcut and Save's stay.
        send(&mut c, Writer::new(4, 2).uint(12).uint(0).uint(1).uint(1).string("Close Window"));
        send(&mut c, Writer::new(4, 2).uint(10).uint(DISABLED).uint(0).uint(0).string("Save"));
        let t = c.menu_tree(4).unwrap();
        assert_eq!(t.item(12).unwrap().title, "Close", "pending until commit");
        assert!(c.take_menu_updates().is_empty());
        commit(&mut c);
        assert_eq!(c.take_menu_updates(), vec![Update::Changed(4)]);
        let t = c.menu_tree(4).unwrap();
        let close = t.item(12).unwrap();
        assert_eq!((close.title.as_str(), close.flags, close.state, close.indent), ("Close Window", 0, 1, 1));
        let save = t.item(10).unwrap();
        assert_eq!((save.flags, save.key.as_str(), save.role.as_str()), (DISABLED, "s", "save"), "item() leaves the shortcut and role");
        assert_eq!(t.children(1), &[10, 11, 12], "and the menu's children");
        // An empty key takes the shortcut away.
        send(&mut c, Writer::new(4, 3).uint(10).string("").uint(LOGO));
        commit(&mut c);
        let save = c.menu_tree(4).unwrap().item(10).unwrap();
        assert_eq!((save.key.as_str(), save.modifiers), ("", 0));
    }

    #[test]
    fn a_removed_item_leaves_its_menu_and_its_number_is_free() {
        let mut c = published();
        send(&mut c, Writer::new(4, 8).uint(11));
        commit(&mut c);
        assert_eq!(c.menu_tree(4).unwrap().children(1), &[10, 12]);
        assert!(c.menu_tree(4).unwrap().item(11).is_none());
        item(&mut c, 11, 0, "Revert");
        items(&mut c, 1, &[10, 12, 11]);
        commit(&mut c);
        assert!(c.failed.is_none(), "{:?}", c.failed);
        assert_eq!(c.menu_tree(4).unwrap().find("File > Revert"), Some(11));
        // Removing a menu leaves its children described, in no menu.
        send(&mut c, Writer::new(4, 8).uint(1));
        commit(&mut c);
        let t = c.menu_tree(4).unwrap();
        assert_eq!(t.children(0), &[2]);
        assert!(t.item(10).is_some());
    }

    #[test]
    fn a_commit_that_is_not_a_tree_is_a_protocol_error() {
        // A child never described.
        let mut c = client();
        item(&mut c, 1, SUBMENU, "File");
        items(&mut c, 0, &[1, 7]);
        commit(&mut c);
        assert!(c.failed.as_deref().is_some_and(|m| m.contains("never described")), "{:?}", c.failed);
        // An item in two menus.
        let mut c = client();
        item(&mut c, 1, SUBMENU, "File");
        item(&mut c, 2, 0, "Save");
        items(&mut c, 0, &[1, 2]);
        items(&mut c, 1, &[2]);
        commit(&mut c);
        assert!(c.failed.as_deref().is_some_and(|m| m.contains("two menus")), "{:?}", c.failed);
        // A menu inside itself (and so not under the root).
        let mut c = client();
        item(&mut c, 1, SUBMENU, "A");
        item(&mut c, 2, SUBMENU, "B");
        items(&mut c, 1, &[2]);
        items(&mut c, 2, &[1]);
        commit(&mut c);
        assert!(c.failed.as_deref().is_some_and(|m| m.contains("inside itself")), "{:?}", c.failed);
        // Item zero, and a property of an item never described.
        let mut c = client();
        item(&mut c, 0, 0, "Root");
        assert!(c.failed.is_some());
        let mut c = client();
        send(&mut c, Writer::new(4, 4).uint(9).string("quit"));
        assert!(c.failed.is_some());
    }

    #[test]
    fn about_to_open_is_answered_by_ack_open_after_the_commit() {
        let mut c = published();
        c.take_menu_updates();
        c.take_outgoing();
        let serial = c.menu_about_to_open(4, 1).expect("the tree");
        let out = messages(&c.take_outgoing());
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].0, out[0].1), (4, 0), "about_to_open on the menu");
        assert_eq!(out[0].2, [1u32.to_le_bytes(), serial.to_le_bytes()].concat());
        // The client validates: Close is enabled now. Commit, then ack.
        item(&mut c, 12, 0, "Close");
        commit(&mut c);
        send(&mut c, Writer::new(4, 11).uint(serial));
        assert_eq!(c.take_menu_updates(), vec![Update::Changed(4), Update::Opened { tree: 4, menu: 1, serial }]);
        assert!(!c.menu_tree(4).unwrap().item(12).unwrap().has(DISABLED));
        // An ack covers the earlier serials; one never sent is an error.
        let a = c.menu_about_to_open(4, 1).unwrap();
        let b = c.menu_about_to_open(4, 2).unwrap();
        send(&mut c, Writer::new(4, 11).uint(b));
        assert_eq!(
            c.take_menu_updates(),
            vec![Update::Opened { tree: 4, menu: 1, serial: a }, Update::Opened { tree: 4, menu: 2, serial: b }]
        );
        send(&mut c, Writer::new(4, 11).uint(b + 5));
        assert!(c.failed.is_some());
        assert_eq!(Client::new().menu_about_to_open(4, 0), None, "no such tree");
    }

    #[test]
    fn closed_and_activate_reach_the_client() {
        let mut c = published();
        c.take_outgoing();
        c.menu_closed(4, 1);
        c.menu_activate(4, 10, SHIFT | LOGO);
        c.menu_activate(9, 10, 0);
        let out = messages(&c.take_outgoing());
        assert_eq!(out.len(), 2, "nothing for a tree that is not there");
        assert_eq!((out[0].0, out[0].1, out[0].2.clone()), (4, 1, 1u32.to_le_bytes().to_vec()));
        assert_eq!((out[1].0, out[1].1), (4, 2));
        assert_eq!(out[1].2, [10u32.to_le_bytes(), (SHIFT | LOGO).to_le_bytes()].concat());
    }

    #[test]
    fn a_client_has_one_tree_of_each_kind_and_a_surface_one_menu() {
        let mut c = client();
        send(&mut c, Writer::new(3, 1).new_id(5).uint(2));
        assert!(c.failed.is_none(), "a dock menu beside the menu bar");
        send(&mut c, Writer::new(3, 1).new_id(6).uint(1));
        assert!(c.failed.is_some(), "a second menu bar");
        let mut c = client();
        send(&mut c, Writer::new(3, 1).new_id(5).uint(7));
        assert!(c.failed.is_some(), "no such kind");
        // Destroyed, the kind is free again.
        let mut c = published();
        c.take_menu_updates();
        send(&mut c, Writer::new(4, 0));
        assert_eq!(c.take_menu_updates(), vec![Update::Destroyed(4)]);
        assert_eq!(c.menus().count(), 0);
        send(&mut c, Writer::new(3, 1).new_id(4).uint(1));
        assert!(c.failed.is_none(), "{:?}", c.failed);
    }

    #[test]
    fn a_surfaces_menu_goes_with_its_surface() {
        let mut c = client();
        // bind wl_compositor (name 1) as 7, create_surface(8).
        send(&mut c, Writer::new(2, 0).uint(1).string("wl_compositor").uint(4).new_id(7));
        send(&mut c, Writer::new(7, 0).new_id(8));
        send(&mut c, Writer::new(3, 2).new_id(9).object(8));
        send(&mut c, Writer::new(9, 2).uint(1).uint(SUBMENU).uint(0).uint(0).string("File"));
        send(&mut c, Writer::new(9, 9).uint(0).array(&1u32.to_le_bytes()));
        send(&mut c, Writer::new(9, 10));
        assert!(c.failed.is_none(), "{:?}", c.failed);
        assert_eq!(c.take_menu_updates(), vec![Update::Changed(9)]);
        assert_eq!(c.menu_tree(9).unwrap().scope, Scope::Surface(8));
        send(&mut c, Writer::new(3, 2).new_id(10).object(8));
        assert!(c.failed.is_some(), "a second menu for the surface");
        c.failed = None;
        // wl_surface.destroy
        send(&mut c, Writer::new(8, 0));
        assert_eq!(c.take_menu_updates(), vec![Update::Destroyed(9)]);
        assert!(c.menu_tree(9).is_none());
        assert!(c.take_menu_updates().is_empty(), "said once");
    }

    #[test]
    fn an_image_is_copied_from_its_file_and_bounded() {
        let mut c = published();
        // 2x1: opaque red, half-transparent premultiplied green (B G R A).
        let file = Files(vec![0, 0, 255, 255, 0, 128, 0, 128]);
        c.receive(&Writer::new(4, 6).uint(10).uint(2).uint(1).uint(2).uint(1).finish(), &[5], &file);
        c.receive(&Writer::new(4, 10).finish(), &[], &file);
        assert!(c.failed.is_none(), "{:?}", c.failed);
        let t = c.menu_tree(4).unwrap();
        let image = t.item(10).unwrap().image.clone().expect("the image");
        assert_eq!((image.width, image.height, image.scale, image.template), (2, 1, 2, true));
        assert_eq!(image.rgba(), vec![255, 0, 0, 255, 0, 255, 0, 128]);
        assert_eq!(t.images().len(), 1);
        assert!(t.describe(1, false)[0].ends_with("image=2x1@2 template"), "{:?}", t.describe(1, false));
        assert!(image.json().starts_with(&format!("{{\"image\":{},\"width\":2,\"height\":1,\"scale\":2,\"template\":true,\"rgba\":\"/wAA/wD/AIA=\"", image.serial)));
        // Taken away again.
        send(&mut c, Writer::new(4, 7).uint(10));
        commit(&mut c);
        assert!(c.menu_tree(4).unwrap().item(10).unwrap().image.is_none());
        // Too large, and a file too short.
        c.receive(&Writer::new(4, 6).uint(10).uint(200).uint(1).uint(1).uint(0).finish(), &[6], &file);
        assert!(c.failed.is_some());
        let mut c = published();
        c.receive(&Writer::new(4, 6).uint(10).uint(4).uint(4).uint(1).uint(0).finish(), &[5], &file);
        assert!(c.failed.is_some());
    }

    #[test]
    fn the_tree_as_json() {
        let mut c = published();
        send(&mut c, Writer::new(4, 5).uint(12).string("Close the \"front\" window\n"));
        commit(&mut c);
        assert_eq!(
            c.menu_tree(4).unwrap().json(3),
            concat!(
                r#"{"client":3,"tree":4,"scope":"menu-bar","surface":0,"title":"Gallery","items":["#,
                r#"{"id":1,"title":"File","flags":8,"state":0,"indent":0,"items":["#,
                r#"{"id":10,"title":"Save","flags":0,"state":0,"indent":0,"key":"s","modifiers":8,"role":"save"},"#,
                r#"{"id":11,"title":"","flags":4,"state":0,"indent":0},"#,
                r#"{"id":12,"title":"Close","flags":1,"state":0,"indent":0,"tooltip":"Close the \"front\" window\n"}]},"#,
                r#"{"id":2,"title":"Edit","flags":8,"state":0,"indent":0,"items":[]}]}"#
            )
        );
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
    }
}
