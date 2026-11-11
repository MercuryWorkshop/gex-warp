// Drag and drop between two programs' windows through gex's compositor
// (fixtures/darwin/page/dnd.c, a Wayland client with no library): a press
// in the source's window starts a drag of text (wl_data_device.start_drag,
// with an icon), the pointer takes it into the target's window, the release
// drops it there, the target reads the text through a pipe whose end the
// source is handed (its `send`), and finishes; the source is told the
// target's type and action as it answers, then dropped, then finished.
//
// Native (node host): under one gexc and its compositor, a program and its
// forked child (`dnd pair`), the scripted pointer taking the drag from one
// window to the other (GEX_WL_INPUT's `over`): a Mac program from a Mac root
// (pyrus's), and the same program built for Linux. On the page (browser
// host): a Mac program's text dragged into a Linux program's window, two
// processes each with its own compositor, the page's (HostWindows, its
// windows the page's canvases) carrying the drag between them, with the
// browser's own pointer.
export const meta = { hosts: ['browser', 'node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 300, html: '<style>html, body { margin: 0; } #desk { position: relative; width: 900px; height: 400px; }</style><div id="desk"></div>' };

const TEXT = 'hello from the Mac';

/** What a run of the pair must have said, as checks. */
function checkPair(t, what, out) {
  const { check } = t;
  const has = (re) => re.test(out);
  check(`${what}: the source's press starts a drag`, has(/^dnd: source: pressed: start_drag$/m), out.slice(-600));
  check(`${what}: the drag goes to the target's window, with the source's types`, has(/^dnd: target: enter at \d+,\d+ offering text\/plain;charset=utf-8 text\/plain$/m), out.slice(-600));
  check(`${what}: and moves there`, has(/^dnd: target: motion at \d+,\d+$/m), out.slice(-600));
  check(`${what}: the source is told the target's type and action`, has(/^dnd: source: told target text\/plain;charset=utf-8$/m) && has(/^dnd: source: told action 1$/m), out.slice(-600));
  check(`${what}: the drop is the target's, with the source's text`, has(new RegExp(`^dnd: target: dropped: "${TEXT}"$`, 'm')), out.slice(-600));
  check(`${what}: the source was asked for it, told dropped, then finished`, has(/^dnd: source: asked to send text\/plain;charset=utf-8$/m) && has(/^dnd: source: told dropped$/m) && has(/^dnd: source: told finished$/m), out.slice(-600));
  check(`${what}: and not cancelled`, !has(/told cancelled/), out.slice(-600));
}

async function nativeLeg(t) {
  const { existsSync, writeFileSync } = await import('node:fs');
  const { join } = await import('node:path');
  const { native, exec } = await import('../lib/native.mjs');
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const n = native(t);
  const d = n.dir('dnd');
  const src = join(import.meta.dirname, '..', 'fixtures', 'darwin', 'page', 'dnd.c');
  // The scripted pointer: into the source's window, a press, into the
  // target's (another client's), a move, the release.
  const script = join(d, 'dnd.input');
  writeFileSync(script, ['clients 2', '1.0 over dnd source', '1.1 move 50 50', '1.3 button 272 1', '1.8 move 60 60',
    '2.2 over dnd target', '2.3 move 40 40', '2.7 move 120 90', '3.3 button 272 0', ''].join('\n'));
  const run = async (what, argv) => {
    // (a name in the guest's socket namespace, short enough for sun_path)
    const r = await n.gexc(['run', ...argv.slice(0, -1), '--wayland', `/tmp/gex-dnd-${what}/wayland-0`, '--wayland-size', '800x600', '--wayland-out', 'null', argv.at(-1), 'pair', TEXT],
      { env: { GEX_WL_INPUT: script, GEX_WL_HZ: '60' }, timeout: 120000 });
    t.say(`${what}:\n${r.stdout.trim().split('\n').map((l) => `  | ${l}`).join('\n')}`);
    t.check(`${what}: it ran`, r.code === 0, `exit ${r.code}: ${r.stderr.trim().split('\n').slice(-6).join(' / ')}`);
    checkPair(t, what, r.stdout);
    t.check(`${what}: the compositor carried it`, /\[gex\] dnd: dropped on \d+:\d+: taken/.test(r.stderr) && /the target is done with the drop/.test(r.stderr), r.stderr.split('\n').filter((l) => l.includes('dnd')).join(' / '));
  };
  // Linux's.
  const lin = join(d, 'dnd.linux');
  await n.cc(['-O1', '-static', '-Wl,-Ttext-segment=0x1000000', '-o', lin, src]);
  await run('Linux', [lin]);
  // The Mac's.
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(import.meta.dirname, '..', '..', '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  const prog = join(d, 'dnd.mac');
  const cc = await exec(['xcrun', 'clang', '-arch', x86 ? 'x86_64' : 'arm64', '-O1', '-Wall', '-Werror', '-o', prog, src]);
  if (cc.code !== 0) t.skip(`xcrun clang: ${cc.stderr.trim().split('\n').pop()}`);
  await run('Mac', ['--sysroot', mac, prog]);
}

async function pageLeg(t) {
  const { check, timeout, until, say } = t;
  const { System } = await import('../../page/runtime/src/index.js');
  const { MemoryFilesystem } = await import('../../page/runtime/src/fs.js');
  const { HostWindows } = await import('../../page/runtime/src/windows/host.js');
  const { bytesOf } = await import('../../page/runtime/src/internal.js');
  let mac, linux;
  try {
    mac = await bytesOf(t.work(`darwin-dnd.${t.arch}`));
    linux = await bytesOf(t.work(`darwin-dnd-linux.${t.arch}`));
  } catch {
    t.skip(`no Mac root or no compiler for ${t.arch} (GEX_TEST_DARWIN_ROOT${t.arch === 'x86' ? '_X86' : ''}: pyrus builds one; zig or GEX_TOOLCHAIN for the Linux half)`);
  }
  const sys = new System({ baseUrl: t.baseUrl, debug: t.debug() });
  const fs = new MemoryFilesystem(sys);
  fs.addTar(t.work(`darwin-root.${t.arch}.tar`));
  fs.addFiles({ '/usr/local/bin/dnd': mac, '/usr/local/bin/dnd-linux': linux }, { mode: 0o755 });
  const hw = new HostWindows(sys, { width: 900, height: 400, scale: 1 });
  const desk = document.getElementById('desk');
  const wins = [];
  hw.addEventListener('window', (e) => {
    const w = e.window;
    if (w.kind !== 'toplevel') return;
    wins.push(w);
    const x = w.title === 'mac source' ? 0 : 450;
    Object.assign(w.canvas.style, { position: 'absolute', left: `${x}px`, top: '0px' });
    desk.append(w.canvas);
    w.place(x, 0);
  });
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
  const src = sys.spawn(['/usr/local/bin/dnd', 'source', 'mac source', TEXT, '300', '200'], { env });
  const dst = sys.spawn(['/usr/local/bin/dnd-linux', 'target', 'linux target', '300', '200'], { env });
  const reading = Promise.all([read(src.stdout), read(src.stderr), read(dst.stdout), read(dst.stderr)]);
  await until(() => wins.some((w) => w.title === 'mac source') && wins.some((w) => w.title === 'linux target') && /source: shown/.test(text) && /target: shown/.test(text), 'both windows', 120000)
    .catch((e) => { throw new Error(`${e.message}: ${text.slice(-600)}`); });
  // The browser's pointer: pressed in the Mac program's window, carried in
  // steps into the Linux program's, released there.
  const box = desk.getBoundingClientRect();
  const at = (x, y) => [box.left + x, box.top + y];
  const mouse = (type, [x, y], buttons) => t.input({ method: 'Input.dispatchMouseEvent', type, x, y, button: 'left', buttons, clickCount: 1 });
  await mouse('mouseMoved', at(50, 50), 0);
  await mouse('mousePressed', at(50, 50), 1);
  await until(() => /source: pressed: start_drag/.test(text), 'the drag starts', 30000).catch(() => {});
  await new Promise((r) => setTimeout(r, 500));
  for (const x of [80, 150, 300, 470, 500, 540]) {
    await mouse('mouseMoved', at(x, 60 + x / 10), 1);
    await new Promise((r) => setTimeout(r, 200));
  }
  await until(() => /target: enter at/.test(text) && /source: told action 1/.test(text), 'the target takes it', 20000).catch(() => {});
  await t.screenshot?.('dragging');
  const icon = [...document.body.children].some((c) => c.tagName === 'CANVAS' && c.style.position === 'fixed' && c.style.pointerEvents === 'none');
  check('its icon is drawn at the pointer', icon);
  await mouse('mouseReleased', at(540, 114), 0);
  await until(() => /target: dropped: /.test(text) && /source: told (finished|cancelled)/.test(text), 'the drop and its finish', 30000).catch(() => {});
  say(text.trim().split('\n').map((l) => `  | ${l}`).join('\n'));
  checkPair(t, 'Mac to Linux on the page', text);
  src.kill('SIGTERM');
  dst.kill('SIGTERM');
  await sys.shutdown();
  await reading.catch(() => {});
}

export default async function (t) {
  if (t.host === 'node') return nativeLeg(t);
  return pageLeg(t);
}
