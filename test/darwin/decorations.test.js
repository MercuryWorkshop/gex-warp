// Who draws a window's frame, and the room a window is given
// (fixtures/darwin/page/window.c, a Mac program with no Wayland library).
// A window that says nothing is framed by the page (HostWindows'
// `decorations` 'host'), and the room it is given (configure_bounds) is the
// output less that frame. A window that draws its own frame or wants none
// (`bare`: zxdg_toplevel_decoration_v1.set_mode(client_side), what pyrus
// asks for a borderless window) is shown bare ('client') and is given the
// whole output: a borderless 256x256 window on a 256x256 output came out
// 254x229 when the frame it never has was taken off. Its window geometry
// (xdg_surface.set_window_geometry) is the page's to read. Natively the
// same room is given (the native host draws no frames, but what is
// checked there is the room the page would give).
export const meta = { hosts: ['browser', 'node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 300, html: '<style>html, body { margin: 0; } #desk { position: relative; width: 900px; height: 400px; }</style><div id="desk"></div>' };

async function nativeLeg(t) {
  const { join } = await import('node:path');
  const { existsSync } = await import('node:fs');
  const { native, exec } = await import('../lib/native.mjs');
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const n = native(t);
  const d = n.dir('decorations');
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(import.meta.dirname, '..', '..', '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac}`);
  const prog = join(d, 'window');
  const cc = await exec(['xcrun', 'clang', '-arch', x86 ? 'x86_64' : 'arm64', '-O1', '-o', prog, join(import.meta.dirname, '..', 'fixtures', 'darwin', 'page', 'window.c')]);
  if (cc.code !== 0) t.skip(`xcrun clang: ${cc.stderr.trim().split('\n').pop()}`);
  for (const [what, extra, room] of [['framed', [], '254 by 229'], ['bare', ['bare'], '256 by 256']]) {
    // Ended by its input's end (stdin is closed): it shows its window and goes.
    const r = await n.gexc(['run', '--sysroot', mac, '--wayland', `/tmp/gex-deco-${what}/wayland-0`, '--wayland-size', '256x256', '--wayland-out', 'null', prog, what, '256', '256', ...extra], { timeout: 120000 });
    t.check(`natively, a ${what} window on a 256x256 output is given ${room}`, new RegExp(`^window: bounds ${room}$`, 'm').test(r.stdout), r.stdout.slice(-300));
    t.check(`natively, a ${what} window is shown at 256 by 256`, /^window: shown, 256 by 256$/m.test(r.stdout), r.stdout.slice(-300));
  }
}

async function pageLeg(t) {
  const { check, eq, timeout, until } = t;
  const { System } = await import('../../page/runtime/src/index.js');
  const { MemoryFilesystem } = await import('../../page/runtime/src/fs.js');
  const { HostWindows } = await import('../../page/runtime/src/windows/host.js');
  const { bytesOf } = await import('../../page/runtime/src/internal.js');
  let prog;
  try {
    prog = await bytesOf(t.work(`darwin-window.${t.arch}`));
  } catch {
    t.skip(`no Mac root or no Mac compiler for ${t.arch}`);
  }
  const sys = new System({ baseUrl: t.baseUrl, debug: t.debug() });
  const fs = new MemoryFilesystem(sys);
  fs.addTar(t.work(`darwin-root.${t.arch}.tar`));
  fs.addFiles({ '/usr/local/bin/window': prog }, { mode: 0o755 });
  const hw = new HostWindows(sys, { width: 900, height: 400, scale: 1 });
  const wins = [];
  hw.addEventListener('window', (e) => wins.push(e.window));
  await timeout(sys.boot(), 60000, 'boot');
  const env = { PATH: '/usr/bin:/bin', HOME: '/', ...hw.env };
  let text = '';
  const read = async (s) => {
    const dec = new TextDecoder();
    for (const r = s.getReader(); ;) {
      const { value, done } = await r.read();
      if (done) return;
      text += dec.decode(value, { stream: true });
    }
  };
  const framed = sys.spawn(['/usr/local/bin/window', 'framed', '300', '200'], { env });
  const bare = sys.spawn(['/usr/local/bin/window', 'bare', '300', '200', 'bare', 'geometry', '10', '12', '280', '176'], { env });
  const reading = Promise.all([framed, bare].flatMap((p) => [read(p.stdout), read(p.stderr)]));
  const w = (title) => wins.find((x) => x.title === title);
  await until(() => w('framed') && w('bare') && (text.match(/^window: shown/gm) ?? []).length === 2, 'both windows', 120000)
    .catch((e) => { throw new Error(`${e.message}: ${text.slice(-500)}`); });
  eq('a window that says nothing is framed by the page', w('framed').decorations, 'host');
  check('and is given the output less the frame', /^window: bounds 898 by 373$/m.test(text), text);
  eq('a window that draws its own frame is shown bare', w('bare').decorations, 'client');
  check('it was told so', /^window: its frame is its own$/m.test(text), text);
  check('and is given the whole output', /^window: bounds 900 by 400$/m.test(text), text);
  eq('its window geometry is the page\'s to read', w('bare').geometry, { x: 10, y: 12, w: 280, h: 176 });
  eq('a window that set none has none', w('framed').geometry, null);
  framed.kill('SIGTERM');
  bare.kill('SIGTERM');
  await sys.shutdown();
  await reading.catch(() => {});
}

export default async function (t) {
  if (t.host === 'node') return nativeLeg(t);
  return pageLeg(t);
}
