// A Mac program's window on the page: fixtures/darwin/page/window.c, a
// Wayland client with no library (the protocol by hand, its pixels in POSIX
// shared memory whose descriptor it passes), run from a Mac root (pyrus's)
// against the page's compositor. The compositor lists its window by its
// title, a click and a key pressed in the page reach the program, and the
// window goes when the program ends.
//
// The root and the program are the `darwin-root` image
// (test/fixtures/images.mjs). SwiftShader here.
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { CompositorWindows } from '../../page/runtime/src/windows/compositor.js';
import { bytesOf } from '../../page/runtime/src/internal.js';

export const meta = { hosts: ['browser'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 300, html: '<canvas id="out" width="1280" height="720"></canvas>' };

const TITLE = 'A window of a Mac program';

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
  const out = new CompositorWindows(sys, { canvas: document.getElementById('out') });
  await timeout(sys.boot(), 60000, 'boot');
  const p = sys.spawn(['/usr/local/bin/window', TITLE, '480', '320'], { env: { PATH: '/usr/bin:/bin', HOME: '/', ...out.env } });
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
  await until(() => /^window: (shown|.*: )/m.test(text), 'the program showed its window, or said why not', 120000)
    .catch((e) => { throw new Error(`${e.message}: ${text.slice(-400)}`); });
  say(text.trim().split('\n').map((l) => `  | ${l}`).join('\n'));
  check('it found the compositor and what it has', /^window: a compositor yes, shared memory yes, windows yes$/m.test(text), text.slice(-300));
  check('it showed its window', /^window: shown, \d+ by \d+$/m.test(text), text.slice(-300));
  let seen = null;
  await until(async () => (seen = await out.describe())?.titles?.includes(TITLE), 'the compositor has the window', 30000).catch(() => {});
  check(`the compositor lists it by its title (${JSON.stringify(seen?.titles)})`, !!seen?.titles?.includes(TITLE), (seen?.log ?? []).slice(-8).join(' / '));
  await t.screenshot?.('window');
  // A click in the window (the canvas is at the page's margin, the window
  // at its corner under a title bar), and a key pressed after it.
  const box = document.getElementById('out').getBoundingClientRect();
  await t.click(box.left + 200, box.top + 150);
  await until(() => /^window: button \d+ up/m.test(text), 'the click reached the program', 30000).catch(() => {});
  check('a click in the page reaches the program, in its window', /^window: button 272 down, the pointer in the window$/m.test(text) && /^window: button 272 up/m.test(text), text.slice(-300));
  await t.key('KeyA', 'a');
  await until(() => /^window: key \d+ up/m.test(text), 'the key reached the program', 30000).catch(() => {});
  check('and a key pressed, by its number', /^window: key 30 down$/m.test(text) && /^window: key 30 up$/m.test(text), text.slice(-300));
  // (And no menu protocol: this page's compositor draws no menus, so the
  // program is not offered `gex_menu_manager_v1`. darwin/menu is the page
  // that presents them.)
  check('a page that presents no menus offers the program no menu protocol', /^window: the shell presents menus: no$/m.test(text), text.slice(-300));
  p.kill('SIGTERM');
  await until(async () => !(seen = await out.describe())?.titles?.includes(TITLE), 'the window went', 30000).catch(() => {});
  check('its window goes when it ends', !seen?.titles?.includes(TITLE), (seen?.log ?? []).slice(-8).join(' / '));
  await sys.shutdown();
  await reading.catch(() => {});
}
