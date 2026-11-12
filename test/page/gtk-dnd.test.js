// Drag and drop between two real GTK 3 programs on the page: text typed into
// Mousepad (a GtkSourceView) is selected and dragged, with the browser's own
// pointer, into Geany's editor (Scintilla's GTK widget), each program a
// process with its own compositor and its window a canvas of the page's
// (HostWindows), the page carrying the drag between them. GTK is fussier
// than a raw client (darwin/dnd's fixture): its drag icon, version 3's
// action negotiation, the serials of the press and of the enter, a drop
// that comes before the data is read.
//
// What is checked is what each program says and does: their Wayland traffic
// (WAYLAND_DEBUG) -- the source's start_drag, told the target's type and a
// move, dropped, asked for its text and finished; the target's offer
// entered, received and finished -- and Geany's document, which is not
// modified ("untitled - Geany") until the text arrives in it
// ("*untitled - Geany").
export const meta = {
  hosts: ['browser'], arches: ['x86'], images: ['linux-gtk'], timeout: 600,
  html: '<style>html, body { margin: 0; } #desk { position: relative; width: 1200px; height: 560px; }</style><div id="desk"></div>',
};

const GTK = { PATH: '/usr/local/bin:/usr/bin:/bin', HOME: '/root', LANG: 'C.UTF-8', GDK_BACKEND: 'wayland', GSETTINGS_BACKEND: 'memory', NO_AT_BRIDGE: '1', WAYLAND_DEBUG: '1' };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export default async function (t) {
  const { check, timeout, until, say } = t;
  const { System } = await import('../../page/runtime/src/index.js');
  const { MemoryFilesystem } = await import('../../page/runtime/src/fs.js');
  const { HostWindows } = await import('../../page/runtime/src/windows/host.js');
  const sys = new System({ baseUrl: t.baseUrl, debug: t.debug() });
  const fs = new MemoryFilesystem(sys);
  fs.addTar(t.work('linux-gtk.tar'));
  const hw = new HostWindows(sys, { width: 1200, height: 560, scale: 1 });
  const desk = document.getElementById('desk');
  // Mousepad on the left, Geany on the right, side by side.
  const wins = {};
  hw.addEventListener('window', ({ window: w }) => {
    if (w.kind !== 'toplevel') return;
    // (Mousepad is started first, and Geany once it is shown.)
    const name = wins.mousepad ? 'geany' : 'mousepad';
    wins[name] = w;
    const x = name === 'mousepad' ? 0 : 600;
    Object.assign(w.canvas.style, { position: 'absolute', left: `${x}px`, top: '0px' });
    desk.append(w.canvas);
    w.place(x, 0);
    w.resize(580, 540);
  });
  // A page as wide as the two windows (the runner's is narrower).
  await t.input({ method: 'Emulation.setDeviceMetricsOverride', width: 1240, height: 600, deviceScaleFactor: 1, mobile: false });
  await timeout(sys.boot(), 60000, 'boot');
  const out ={ mousepad: '', geany: '' };
  const read = async (s, k) => {
    const dec = new TextDecoder();
    for (const r = s.getReader(); ;) {
      const { value, done } = await r.read();
      if (done) return;
      out[k] += dec.decode(value, { stream: true });
    }
  };
  const run = (k, argv) => {
    const p = sys.spawn(argv, { env: { ...GTK, ...hw.env }, cwd: '/root' });
    return [p, read(p.stdout, k), read(p.stderr, k)];
  };
  const [mp, ...mr] = run('mousepad', ['/usr/bin/mousepad']);
  const title = (k, re) => re.test(wins[k]?.title ?? '');
  await until(() => title('mousepad', /Mousepad/), 'Mousepad\'s window', 300000)
    .catch((e) => { throw new Error(`${e.message}: ${out.mousepad.slice(-600)}`); });
  await sleep(2000);
  const box = desk.getBoundingClientRect();
  const at = (x, y) => [box.left + x, box.top + y];
  const mouse = (type, [x, y], buttons, clickCount = 1) => t.input({ method: 'Input.dispatchMouseEvent', type, x, y, button: 'left', buttons, clickCount });
  const key = (type, key, code, vk, modifiers = 0, text) => t.input({ method: 'Input.dispatchKeyEvent', type, key, code, windowsVirtualKeyCode: vk, modifiers, ...(text ? { text } : {}) });
  // Its text: typed, then all of it selected.
  await mouse('mouseMoved', at(300, 300), 0);
  await mouse('mousePressed', at(300, 300), 1);
  await mouse('mouseReleased', at(300, 300), 0);
  await sleep(500);
  for (const c of 'hello') {
    await key('keyDown', c, `Key${c.toUpperCase()}`, c.toUpperCase().charCodeAt(0), 0, c);
    await key('keyUp', c, `Key${c.toUpperCase()}`, c.toUpperCase().charCodeAt(0));
    await sleep(150);
  }
  await until(() => title('mousepad', /^\*/), 'the text typed', 60000).catch(() => {});
  await key('rawKeyDown', 'Control', 'ControlLeft', 17, 2);
  await key('rawKeyDown', 'a', 'KeyA', 65, 2);
  await key('keyUp', 'a', 'KeyA', 65, 2);
  await key('keyUp', 'Control', 'ControlLeft', 17, 0);
  // Where the selection is: its colour in Mousepad's canvas.
  let sel = null;
  await until(() => {
    const c = wins.mousepad.canvas;
    const { data, width, height } = c.getContext('2d').getImageData(0, 0, c.width, c.height);
    let n = 0, sx = 0, sy = 0;
    for (let y = 0; y < height; y++) {
      for (let x = 0; x < width; x++) {
        const i = (y * width + x) * 4;
        if (data[i] < 90 && data[i + 1] > 100 && data[i + 1] < 170 && data[i + 2] > 200) { n++; sx += x; sy += y; }
      }
    }
    if (n > 100) sel = [Math.round(sx / n), Math.round(sy / n)];
    return sel;
  }, 'the selection drawn', 30000).catch(() => {});
  await t.screenshot?.('selected');
  check('the text is typed and selected in Mousepad', !!sel && title('mousepad', /^\*/), `${wins.mousepad.title}; selection ${sel}`);
  if (!sel) throw new Error('nothing selected to drag');
  const [gp, ...gr] = run('geany', ['/usr/bin/geany']);
  await until(() => title('geany', /^untitled - Geany/), 'Geany\'s window', 300000)
    .catch((e) => { throw new Error(`${e.message}: ${out.geany.slice(-600)}`); });
  await sleep(3000);
  // The drag, as a hand does it: pressed on the selection, held, carried in
  // steps a frame or more apart into Geany's editor, held there, let go.
  const from = at(...sel);
  const to = at(900, 300);
  await mouse('mouseMoved', from, 0);
  await sleep(200);
  await mouse('mousePressed', from, 1);
  await sleep(300);
  for (let i = 1; i <= 20; i++) {
    await mouse('mouseMoved', [from[0] + ((to[0] - from[0]) * i) / 20, from[1] + ((to[1] - from[1]) * i) / 20], 1);
    await sleep(60);
  }
  await until(() => /wl_data_source#\d+\.target\("[^"]+"\)/.test(out.mousepad), 'Geany to take it', 30000).catch(() => {});
  await sleep(500);
  await t.screenshot?.('dragging');
  await mouse('mouseReleased', to, 0);
  await until(() => title('geany', /^\*untitled - Geany/) && /wl_data_source#\d+\.dnd_finished\(\)/.test(out.mousepad), 'the drop', 60000).catch(() => {});
  await sleep(2000);
  await t.screenshot?.('dropped');
  const traffic = (k, re) => out[k].split('\n').filter((l) => re.test(l)).map((l) => `  | ${k}: ${l.replace(/^\[[\d.]+\] \{[^}]*\} */, '').slice(0, 160)}`);
  say([...traffic('mousepad', /start_drag|wl_data_source#\d+\.(target|action|dnd_|send|cancelled)/), ...traffic('geany', /wl_data_device#\d+\.(enter|drop|leave)|wl_data_offer#\d+\.(receive|finish)/)].join('\n'));
  const has = (k, re) => re.test(out[k]);
  check('Mousepad starts the drag', has('mousepad', /-> wl_data_device#\d+\.start_drag\(wl_data_source#\d+, wl_surface#\d+, wl_surface#\d+, \d+\)/), out.mousepad.slice(-400));
  check('it is told Geany takes text, as a move', has('mousepad', /wl_data_source#\d+\.target\("(text\/plain[^"]*|UTF8_STRING|TEXT|STRING)"\)/) && has('mousepad', /wl_data_source#\d+\.action\(2\)/));
  check('Geany is offered it', has('geany', /wl_data_device#\d+\.enter\(\d+, wl_surface#\d+, [\d.]+, [\d.]+, wl_data_offer#\d+\)/) && has('geany', /wl_data_offer#\d+\.source_actions\(3\)/));
  check('it is dropped on Geany', has('geany', /wl_data_device#\d+\.drop\(\)/) && has('mousepad', /wl_data_source#\d+\.dnd_drop_performed\(\)/));
  check('Geany asks for the text and Mousepad is asked to send it', has('geany', /-> wl_data_offer#\d+\.receive\("[^"]+", fd \d+\)/) && has('mousepad', /wl_data_source#\d+\.send\("[^"]+", fd \d+\)/));
  check('Geany finishes, and Mousepad is told so', has('geany', /-> wl_data_offer#\d+\.finish\(\)/) && has('mousepad', /wl_data_source#\d+\.dnd_finished\(\)/));
  check('and not cancelled', !has('mousepad', /wl_data_source#\d+\.cancelled\(\)/));
  check('the text is in Geany\'s document', title('geany', /^\*untitled - Geany/), wins.geany.title);
  mp.kill('SIGKILL');
  gp.kill('SIGKILL');
  await sys.shutdown();
  await Promise.all([...mr, ...gr]).catch(() => {});
}
