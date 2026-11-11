// `HostWindows`: the guest's windows handed to the page one by one, each a
// bare canvas the page frames and places itself (a desktop's window manager,
// AliceWM in Anura). The guest's compositor is gex's (page/src/dom-compositor.js
// `DomDesktop`): the same protocol in every guest process, the windows drawn
// into their own canvases, input taken from each.

import { INTERNAL, internals } from '../internal.js';
import { xOnDemand } from './x11.js';
import { mirrored } from './mirrored.js';

const KINDS = { 1: 'toplevel', 2: 'popup', 3: 'subsurface' };
/** `DomDesktop.STATE_*`. */
const STATE = { maximized: 2, resizing: 4, fullscreen: 8 };

const MENU_STATES = ['off', 'on', 'mixed'];
/** `gex_menu_v1.modifiers`, by the names a page's key and pointer events
 *  have for them (and the protocol's own). */
const MENU_MODS = [['shift', 'shiftKey', 1], ['control', 'ctrlKey', 2], ['alt', 'altKey', 4], ['logo', 'metaKey', 8]];

/** A tree's items as plain, frozen data (README "Menus"). */
function menuItems(d, tree, items) {
  return Object.freeze(items.map((i) => Object.freeze({
    id: i.id,
    title: i.title,
    enabled: !(i.flags & 1),
    hidden: !!(i.flags & 2),
    separator: !!(i.flags & 4),
    alternate: !!(i.flags & 16),
    header: !!(i.flags & 32),
    state: MENU_STATES[i.state] ?? 'off',
    indent: i.indent | 0,
    role: i.role ?? null,
    shortcut: i.key ? Object.freeze({ key: i.key, shift: !!(i.modifiers & 1), control: !!(i.modifiers & 2), alt: !!(i.modifiers & 4), logo: !!(i.modifiers & 8) }) : null,
    tooltip: i.tooltip ?? null,
    image: i.image ? d.menuImage(tree, i.image) : null,
    submenu: i.items ? menuItems(d, tree, i.items) : null,
  })));
}

function findItem(items, id) {
  for (const i of items) {
    if (i.id === id) return i;
    const below = i.submenu && findItem(i.submenu, id);
    if (below) return below;
  }
  return null;
}

/** A shortcut as a menu shows it: `'mac'` the Mac's symbols ("⇧⌘S"),
 *  `'plus'` names joined by + ("Ctrl+Shift+S", the logo key as `logo`,
 *  default "Super"). */
export function shortcutLabel(shortcut, { style = 'mac', logo = 'Super' } = {}) {
  if (!shortcut) return '';
  const k = shortcut.key;
  const mac = style === 'mac';
  const names = mac
    ? { Enter: '↩', Tab: '⇥', Backspace: '⌫', Delete: '⌦', Escape: '⎋', ArrowUp: '↑', ArrowDown: '↓', ArrowLeft: '←', ArrowRight: '→', Home: '↖', End: '↘', PageUp: '⇞', PageDown: '⇟', ' ': 'Space' }
    : { ArrowUp: 'Up', ArrowDown: 'Down', ArrowLeft: 'Left', ArrowRight: 'Right', Escape: 'Esc', ' ': 'Space' };
  const key = names[k] ?? (k.length === 1 ? k.toUpperCase() : k);
  if (mac) return `${shortcut.control ? '⌃' : ''}${shortcut.alt ? '⌥' : ''}${shortcut.shift ? '⇧' : ''}${shortcut.logo ? '⌘' : ''}${key}`;
  return [shortcut.control && 'Ctrl', shortcut.alt && 'Alt', shortcut.shift && 'Shift', shortcut.logo && logo, key].filter(Boolean).join('+');
}

/** One tree of menus a program handed to the page (README "Menus"): its
 *  menu bar, its dock menu, or one window's menu bar. Events: `change` (the
 *  items or the title are different) and `closed` (the program took it
 *  away, or ended). */
export class GuestMenu extends EventTarget {
  #d;
  #host;
  #t;
  #items;
  #gone = false;

  constructor(host, desktop, tree) {
    super();
    this.#host = host;
    this.#d = desktop;
    this[INTERNAL] = { update: (t) => this.#update(t), gone: () => this.#end(), tree: () => this.#t };
    this.#t = tree;
    this.#items = menuItems(desktop, tree, tree.items);
  }

  #update(tree) {
    this.#t = tree;
    this.#items = menuItems(this.#d, tree, tree.items);
    this.dispatchEvent(new Event('change'));
  }

  #end() {
    this.#gone = true;
    this.dispatchEvent(new Event('closed'));
  }

  /** `'menu-bar'` (the application's, shown while any of its windows has
   *  the keyboard), `'dock'` (items for the menu of the application's icon)
   *  or `'window'` (one window's own menu bar). */
  get scope() {
    return this.#t.scope;
  }

  /** The application's name, as it says it. */
  get title() {
    return this.#t.title;
  }

  /** The pid of the program whose menu this is. */
  get pid() {
    return this.#t.token;
  }

  /** The windows this is the menu of (`window.menu === this`). */
  get windows() {
    return this.#host.windows().filter((w) => w.menu === this);
  }

  /** The bar's items (a dock menu's: the items themselves), in order:
   *  plain frozen data, new objects after each `change`. */
  get items() {
    return this.#items;
  }

  /** The item numbered `id`, wherever it is in the tree, or null. */
  item(id) {
    return findItem(this.#items, id);
  }

  /** Before showing a menu: tells the program, which brings the menu's
   *  items up to date (enabled, check marks, titles, which there are), and
   *  resolves with them once it has, or after `timeout` ms with what there
   *  is (a busy program must not make the bar dead; a `change` follows when
   *  it does answer). `id`: an item with a `submenu`, or 0 for the bar
   *  itself. Pair each with `closeMenu`. */
  async openMenu(id = 0, { timeout = 300 } = {}) {
    if (!this.#gone) {
      let timer;
      await Promise.race([this.#d.menuOpen(this.#t, id), new Promise((res) => (timer = setTimeout(res, timeout)))]);
      clearTimeout(timer);
    }
    return id ? this.item(id)?.submenu ?? Object.freeze([]) : this.#items;
  }

  /** The page stopped showing the menu it opened. */
  closeMenu(id = 0) {
    if (!this.#gone) this.#d.menuClosed(this.#t, id);
  }

  /** The item was chosen: the program acts as on a click in a menu of its
   *  own (and does nothing if the item cannot be chosen now). `modifiers`:
   *  what was held, as `{ shift, control, alt, logo }` or the page's own
   *  event (`shiftKey`, `ctrlKey`, `altKey`, `metaKey`). No `openMenu` is
   *  needed first: a page may run an item from a shortcut of its own. */
  activate(id, { modifiers } = {}) {
    if (this.#gone) return;
    let bits = 0;
    for (const [name, key, bit] of MENU_MODS) if (modifiers?.[name] || modifiers?.[key]) bits |= bit;
    this.#d.menuActivate(this.#t, id, bits);
  }
}

/** One guest window, as the page sees it. Events: `title`, `appid`,
 *  `resized`, `position` (a popup's or a placed window's place changed),
 *  `request` (`kind`: 'move', 'resize' with `edges`, 'maximize',
 *  'unmaximize', 'minimize', 'fullscreen', 'unfullscreen'), `restack` (a
 *  placed window's `below`) and `closed`. */
export class GuestWindow extends EventTarget {
  #w;
  #d;
  #host;
  #states = { maximized: false, fullscreen: false, resizing: false };

  constructor(host, desktop, w) {
    super();
    this.#host = host;
    this.#d = desktop;
    this.#w = w;
    /** The window's pixels. The page puts it where it likes; its CSS size
     *  is set here (`width`, `height`) and follows the client's. */
    this.canvas = w.canvas;
    /** 'toplevel', 'popup' or 'subsurface'. */
    this.kind = KINDS[w.role] ?? 'toplevel';
    const c = this.canvas;
    c.tabIndex = 0;
    c.style.outline = 'none';
    c.dataset.gexWindow = w.key;
    this.#size();
    // Keys go to the window whose canvas has the page's focus.
    c.addEventListener('keydown', (e) => this.#host.keyEvent(e, true));
    c.addEventListener('keyup', (e) => this.#host.keyEvent(e, false));
    c.addEventListener('focus', () => desktop.focus(w));
    w.onretitle = () => this.dispatchEvent(new Event('title'));
    w.onappid = () => this.dispatchEvent(new Event('appid'));
    w.onresized = () => {
      this.#size();
      this.dispatchEvent(new Event('resized'));
    };
    w.onplace = () => this.dispatchEvent(new Event('position'));
    w.onrestack = (below) => this.dispatchEvent(Object.assign(new Event('restack'), { below }));
    w.onrequest = (kind, edges) => this.dispatchEvent(Object.assign(new Event('request'), { kind, edges }));
    w.onfullscreen = (on) => this.dispatchEvent(Object.assign(new Event('request'), { kind: on ? 'fullscreen' : 'unfullscreen' }));
  }

  #size() {
    this.canvas.style.width = `${this.width}px`;
    this.canvas.style.height = `${this.height}px`;
  }

  /** The pid of the process that made it: an X window's as its program
   *  says (`_NET_WM_PID`), else its X server's. */
  get pid() {
    return this.#w.guestPid || this.#w.token;
  }

  /** The menu a shell shows for this window (README "Menus"): its own menu
   *  bar if it has one, else its application's; null when the program
   *  published none, or the page does not present menus. */
  get menu() {
    const w = this.#w;
    return this.#host[INTERNAL].menuOf(w.token, w.client, w.surface);
  }

  /** The window it belongs to (a popup's or a subsurface's), or null. */
  get parent() {
    const w = this.#w;
    if (!w.parent || this.kind === 'toplevel') return null;
    return this.#host[INTERNAL].byKey(`${w.token}:${w.client}:${w.parent}`);
  }

  get title() {
    return this.#w.title ?? '';
  }

  /** What application it is, or null when its program does not say: a
   *  Wayland window's `xdg_toplevel` app id (by convention its desktop
   *  entry's name, `org.kde.kate`), an X window's `WM_CLASS` class (a
   *  desktop entry's `StartupWMClass`, `XTerm`). The `appid` event when it
   *  changes. */
  get appId() {
    return this.#w.appId || null;
  }

  /** Who draws its frame: 'host' (the page), 'client' (the program: a
   *  Win32 window under Wine that has no caption or was not given the
   *  page's frame, an X menu, a GTK 4 window), or 'none' (a popup, a
   *  subsurface: never framed). A placed window can be the host's: a Win32
   *  window with a caption under a Wine that asks for server-side
   *  decoration, whose client area this is. */
  get decorations() {
    if (this.kind !== 'toplevel') return 'none';
    return this.#w.decorations === 'server' ? 'host' : 'client';
  }

  /** Placed by its client (a Win32 window under Wine, an X menu): `x`, `y`
   *  are where it goes on the output, and its client moves, sizes and
   *  stacks it (`restack`). The page does not, except for one it frames
   *  (`decorations` 'host'): that one is where the page shows it, and
   *  `place` moves it there, the client following. */
  get placed() {
    return !!this.#w.placed;
  }

  /** Where its client puts it: a popup or a subsurface relative to its
   *  parent's canvas, a placed window on the output. 0 for a toplevel the
   *  page places. */
  get x() {
    return this.kind === 'toplevel' && !this.placed ? 0 : this.#w.relX | 0;
  }

  get y() {
    return this.kind === 'toplevel' && !this.placed ? 0 : this.#w.relY | 0;
  }

  /** Its size in CSS pixels. */
  get width() {
    return Math.round(this.#w.width / (this.#w.bufferScale || 1));
  }

  get height() {
    return Math.round(this.#w.height / (this.#w.bufferScale || 1));
  }

  /** What the page last said of it (`setStates`). */
  get states() {
    return { ...this.#states };
  }

  /** Ask the client to be this size (CSS pixels); it answers by drawing at
   *  it (`resized`). */
  resize(w, h) {
    this.#w.resize(w, h);
  }

  /** Tell the client it is maximized, fullscreen or being resized, at a
   *  size (default its own). */
  setStates({ maximized, fullscreen, resizing } = {}, w, h) {
    if (maximized !== undefined) this.#states.maximized = !!maximized;
    if (fullscreen !== undefined) this.#states.fullscreen = !!fullscreen;
    if (resizing !== undefined) this.#states.resizing = !!resizing;
    let bits = 0;
    for (const [k, b] of Object.entries(STATE)) if (this.#states[k]) bits |= b;
    this.#w.setStates(bits, w ?? this.width, h ?? this.height);
  }

  /** Where the page put it on the output, in CSS pixels: an X window's
   *  menus and drags read it; a placed window's pointer is relative to it.
   *  Call it after each move. */
  place(x, y) {
    this.#w.place(x, y);
  }

  /** Give it the keyboard (and the page's focus). */
  focus() {
    this.#d.focus(this.#w);
    if (globalThis.document?.activeElement !== this.canvas) this.canvas.focus({ preventScroll: true });
  }

  /** Ask it to close, as a frame's close button does. */
  close() {
    this.#w.close();
  }
}

export class HostWindows extends EventTarget {
  #i;
  #d = null;
  #mount = null;
  #size;
  #wins = new Map();
  #menus = new Map();

  /**
   * The guest's windows, each handed to the page (`window` event) to frame
   * and place. `width`, `height`: the output's size in CSS pixels, what a
   * program asking for the screen's size sees (default the viewport);
   * `scale`: device pixels per CSS pixel (default devicePixelRatio, rounded).
   * X11 programs' windows come as any other, through the runtime's X server,
   * started for the first of them. `menus`: the page presents the programs'
   * menus itself (README "Menus"); a program that can then draws no menu
   * bar and hands its menus over (`menu` events, `window.menu`).
   */
  constructor(sys, { width, height, scale, menus = false } = {}) {
    super();
    const i = internals(sys, 'HostWindows');
    i.attach('windows', this, { beforeBoot: true });
    this.#i = i;
    this.#size = this.#fit(width, height, scale);
    const x = {};
    xOnDemand(i, x);
    i.devices(async (dev) => {
      // DomDesktop's element: where it would listen for keys and set the
      // cursor. Out of sight: the windows are the page's, wherever it puts
      // them, and keys come from their canvases.
      const m = document.createElement('div');
      m.style.cssText = 'position:fixed;left:0;top:0;width:0;height:0;overflow:hidden;';
      m.dataset.gexDesktop = '';
      document.body.append(m);
      this.#mount = m;
      // The single-thread build: the scene is the system Worker's, mirrored
      // here (runtime.js `openWaylandMirror`).
      if (dev.single) {
        const { w, h, scale } = this.#size;
        mirrored(dev, m, { width: w * scale, height: h * scale, scale, menus: !!menus }, (d) => this.#start(d));
        return;
      }
      Object.assign(dev, await dev.rt.openWayland({ wl: m, onDesktop: (d) => this.#start(d), menus: !!menus, say: (t) => dev.say(`${t}\n`) }));
    });
    const self = this;
    this[INTERNAL] = {
      x,
      byKey: (k) => self.#wins.get(k) ?? null,
      menuOf: (token, client, surface) => {
        let bar = null;
        for (const m of self.#menus.values()) {
          const t = m[INTERNAL].tree();
          if (t.token !== token || t.client !== client) continue;
          if (t.scope === 'window' && t.surface === surface) return m;
          if (t.scope === 'menu-bar') bar = m;
        }
        return bar;
      },
      of: (pid) => [...self.#wins.values()].filter((w) => w.pid === pid),
      shutdown: () => {
        self.#d?.stop();
        self.#mount?.remove();
      },
    };
  }

  #fit(width, height, scale) {
    const s = Math.max(1, Math.round(scale ?? globalThis.devicePixelRatio ?? 1));
    return {
      w: Math.max(1, Math.round(width ?? globalThis.innerWidth ?? 1280)),
      h: Math.max(1, Math.round(height ?? globalThis.innerHeight ?? 720)),
      scale: s,
    };
  }

  /** The environment a program needs to reach this compositor and the X
   *  display: add it to `spawn`'s `env`. WINEGEX_FRAMES=1 tells Wine the
   *  page frames a placed window with a caption (`decorations` 'host'), so
   *  Win32 draws neither caption nor border. */
  get env() {
    return { XDG_RUNTIME_DIR: '/tmp', WAYLAND_DISPLAY: 'wayland-0', DISPLAY: ':0', WINEGEX_FRAMES: '1' };
  }

  /** The output's new size (CSS pixels) and scale, when the page's desktop
   *  changes: what programs see as their screen from now on. */
  setOutputSize(width, height, scale) {
    this.#size = this.#fit(width, height, scale ?? this.#size.scale);
    this.#d?.resizeOutput(this.#size.w * this.#size.scale, this.#size.h * this.#size.scale, this.#size.scale);
  }

  /** Every window there is, oldest first. */
  windows() {
    return [...this.#wins.values()];
  }

  /** Every menu tree programs have handed over, oldest first. */
  menus() {
    return [...this.#menus.values()];
  }

  /** The window with the keyboard, or null. */
  get focused() {
    const w = this.#d?.focused();
    return w ? this.#wins.get(w.key) ?? null : null;
  }

  /** A key the page heard somewhere other than a window's canvas (its
   *  frame, say), for the window with the keyboard. */
  keyEvent(e, pressed) {
    this.#d?.keyEvent(e, pressed);
  }

  #start(d) {
    this.#d = d;
    const { w, h, scale } = this.#size;
    d.resizeOutput(w * scale, h * scale, scale);
    // The client's cursor, on every window's canvas (a `cursor` event says
    // it too, for a page that draws its own).
    d.oncursor = (css) => {
      for (const win of this.#wins.values()) win.canvas.style.cursor = css;
      this.dispatchEvent(Object.assign(new Event('cursor'), { css }));
    };
    // A program's menus (`menus: true`): a `menu` event for each new tree,
    // `change` and `closed` on the tree after.
    d.onmenu = (key, tree) => {
      const menu = this.#menus.get(key);
      if (!tree) {
        this.#menus.delete(key);
        menu?.[INTERNAL].gone();
      } else if (menu) {
        menu[INTERNAL].update(tree);
      } else {
        const m = new GuestMenu(this, d, tree);
        this.#menus.set(key, m);
        this.dispatchEvent(Object.assign(new Event('menu'), { menu: m }));
      }
    };
    d.onwindow = (dw) => {
      const win = new GuestWindow(this, d, dw);
      this.#wins.set(dw.key, win);
      dw.onclosed = () => {
        this.#wins.delete(dw.key);
        win.canvas.remove();
        win.dispatchEvent(new Event('closed'));
      };
      this.dispatchEvent(Object.assign(new Event('window'), { window: win }));
    };
  }
}
