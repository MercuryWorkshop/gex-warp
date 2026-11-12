// A program's menus, presented by the page (gex_menu_v1; page/runtime
// README "Menus"): fixtures/darwin/page/window.c, the Wayland client with
// no library, run from a Mac root (pyrus's) under `HostWindows` with
// `menus: true`. It is offered the menu protocol and publishes a menu bar;
// the page gets it as plain data on the window (titles, a shortcut, a role,
// a tool tip, a picture), opening a menu waits for the program to bring it
// up to date, an item chosen in the page reaches the program with the
// modifiers held, and the menu goes with the program. (A page that does
// not present menus offers the program nothing: darwin/window.)
//
// The root and the program are the `darwin-root` image
// (test/fixtures/images.mjs).
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { HostWindows, shortcutLabel } from '../../page/runtime/src/windows/host.js';
import { bytesOf } from '../../page/runtime/src/internal.js';

export const meta = { hosts: ['browser'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 300 };

const TITLE = 'Menus of a Mac program';

export default async function (t) {
  const { check, timeout, until, say } = t;
  let prog;
  try {
    prog = await bytesOf(t.work(`darwin-window.${t.arch}`));
  } catch {
    t.skip(`no Mac root or no Mac compiler for ${t.arch} (GEX_TEST_DARWIN_ROOT${t.arch === 'x86' ? '_X86' : ''}: pyrus builds one)`);
  }
  const sys = new System({ baseUrl: t.baseUrl, debug: t.debug() });
  const fs = new MemoryFilesystem(sys);
  fs.addTar(t.work(`darwin-root.${t.arch}.tar`));
  fs.addFiles({ '/usr/local/bin/window': prog }, { mode: 0o755 });
  const hw = new HostWindows(sys, { width: 800, height: 600, scale: 1, menus: true });
  let win = null;
  const menus = [];
  hw.addEventListener('window', ({ window: w }) => {
    win = w;
    document.body.append(w.canvas);
  });
  hw.addEventListener('menu', ({ menu }) => menus.push(menu));
  await timeout(sys.boot(), 60000, 'boot');
  const p = sys.spawn(['/usr/local/bin/window', TITLE, '480', '320'], { env: { PATH: '/usr/bin:/bin', HOME: '/', ...hw.env } });
  let text = '';
  const read = async (s) => {
    const dec = new TextDecoder();
    for (const r = s.getReader(); ;) {
      const { value, done } = await r.read();
      if (done) return;
      text += dec.decode(value, { stream: true });
    }
  };
  const reading = Promise.all([read(p.stdout), read(p.stderr)]);
  await until(() => /^window: (the shell presents menus|.*: )/m.test(text), 'the program showed its window, or said why not', 120000)
    .catch((e) => { throw new Error(`${e.message}: ${text.slice(-400)}`); });
  await until(() => win, 'the page has the window', 30000).catch(() => {});
  check('the page has its window', win?.title === TITLE, text.slice(-300));

  {
    check('the program is offered the menu protocol', /^window: the shell presents menus: yes$/m.test(text), text.slice(-300));
    await until(() => menus.length, 'the page has the menu', 30000).catch(() => {});
    const menu = menus[0];
    check('the page is handed its menu bar, once', menus.length === 1 && hw.menus().length === 1 && menu?.scope === 'menu-bar', `${menus.length}`);
    check('named as the program names it', menu?.title === TITLE, menu?.title);
    check('which is its window\'s', win?.menu === menu && menu?.windows[0] === win && hw.focused?.menu === menu);
    const titles = (items) => items.map((i) => (i.separator ? '-' : i.title)).join('|');
    check('the bar: File, Help', titles(menu?.items ?? []) === 'File|Help' && menu.items[1].role === 'help' && menu.items[0].role === null, JSON.stringify(menu?.items));
    const file = menu?.items[0]?.submenu ?? [];
    check('File\'s items, in order', titles(file) === 'Save|Wrap|-|Opened 0 times', titles(file));
    const save = menu?.item(10);
    check('Save: its shortcut, role and tool tip', save?.shortcut?.key === 's' && save.shortcut.shift && save.shortcut.logo && !save.shortcut.alt && !save.shortcut.control
      && save.role === 'save' && save.tooltip === 'Keep it' && save.enabled && save.state === 'off', JSON.stringify(save));
    check('written as a menu writes it', shortcutLabel(save?.shortcut) === '⇧⌘S' && shortcutLabel(save?.shortcut, { style: 'plus' }) === 'Shift+Super+S', shortcutLabel(save?.shortcut));
    const image = save?.image;
    check('and its picture, as RGBA', image?.width === 2 && image.height === 2 && image.scale === 2 && !image.template
      && [...image.data].join() === '255,0,0,255,0,255,0,255,0,0,255,255,255,255,255,128', JSON.stringify(image && [...image.data]));
    check('the line that counts is disabled', file[3]?.enabled === false && file[1]?.state === 'off');

    // Opened: the program brings File up to date before the page has it.
    let changes = 0;
    menu?.addEventListener('change', () => changes++);
    const opened = await timeout(menu.openMenu(1, { timeout: 20000 }), 30000, 'openMenu');
    check('opening a menu waits for the program\'s word', /^window: menu 1 about to open$/m.test(text), text.slice(-300));
    check('and gives its items as the program then has them', titles(opened) === 'Save|Wrap|-|Opened 1 times' && opened[1].state === 'on', JSON.stringify(opened));
    check('with a change event, and the same numbers for the items', changes >= 1 && menu.item(11) === opened[1] && opened[0].id === 10, `${changes}`);
    await timeout(menu.openMenu(2), 30000, 'openMenu');
    check('a menu with nothing to change is answered too', /^window: menu 2 about to open$/m.test(text), text.slice(-300));
    menu.closeMenu(2);
    // Chosen, with Shift and the logo key held (the page's own event's names).
    menu.activate(10, { modifiers: { shiftKey: true, metaKey: true } });
    await until(() => /^window: menu item 10 chosen/m.test(text), 'the choice reached the program', 30000).catch(() => {});
    check('an item chosen in the page reaches the program, with the modifiers held', /^window: menu item 10 chosen, modifiers 9$/m.test(text), text.slice(-300));
    menu.closeMenu(1);
    await until(() => /^window: menu 1 closed$/m.test(text), 'the close reached the program', 30000).catch(() => {});
    check('and that the menu was put away', /^window: menu 2 closed$/m.test(text) && /^window: menu 1 closed$/m.test(text), text.slice(-300));
    const again = await timeout(menu.openMenu(1, { timeout: 20000 }), 30000, 'openMenu');
    check('opened again, it is brought up to date again', titles(again) === 'Save|Wrap|-|Opened 2 times' && again[1].state === 'off', JSON.stringify(again));
    await t.screenshot?.('menu');
    let closed = false;
    menu.addEventListener('closed', () => (closed = true));
    p.kill('SIGTERM');
    await until(() => closed, 'the menu went', 30000).catch(() => {});
    check('the menu goes when the program ends', closed && hw.menus().length === 0);
  }
  await sys.shutdown();
  await reading.catch(() => {});
}
