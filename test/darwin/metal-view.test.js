// A Mac program's Metal view in a window of the page: mink's MTKView sample
// (its tests/programs/mtkview.m, an unmodified Mach-O linked against AppKit,
// MetalKit, QuartzCore and Metal), in a pyrus root that has QuartzCore, with
// mink's Metal and MetalKit frameworks in it and mink's replayer in a Worker
// behind the page's `Metal`.
//
// Offscreen first (the view in no window): what the program prints and the
// frames it reads back must be what the same binary got from Apple's Metal
// on a Mac. Then in a window, drawing on the view's own timer: its last
// frame, as the page's compositor shows it, must be the Mac's last frame.
// The frames reach the window as pixels (the drawable read back, handed to
// QuartzCore, composited by it into the window's buffer): QuartzCore has no
// way yet to leave a layer to the page's compositor, so no frame crosses as
// a texture.
//
// Everything of mink's and the root come from the `mink` image
// (GEX_TEST_MINK, default ../mink, built by its tools/test-view.sh and
// tools/test-web.sh; GEX_TEST_DARWIN_QC_ROOT, default ../pyrus-qc/out/root):
// without them this is a skip. To the byte with --gpu on a Mac (the
// browser's WebGPU on the GPU Apple's Metal drew on); on a software adapter
// the pictures must agree but for edge pixels.
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { CompositorWindows } from '../../page/runtime/src/windows/compositor.js';
import { Metal } from '../../page/runtime/src/metal.js';
import { bytesOf } from '../../page/runtime/src/internal.js';

export const meta = { hosts: ['browser'], arches: ['a64'], images: ['mink'], timeout: 900, html: '<canvas id="out" width="1280" height="720"></canvas>' };

const SIZE = 256;

/** The records of a run: name -> bytes (mink's tests' format: `record NAME LENGTH TOLERANCE` lines, the bytes in order). */
function records(text, data) {
  const out = new Map();
  let at = 0;
  for (const line of text.split('\n')) {
    const w = line.split(' ');
    if (w.length === 4 && w[0] === 'record') {
      out.set(w[1], data.subarray(at, at + Number(w[2])));
      at += Number(w[2]);
    }
  }
  return out;
}

/** How many bytes of two equal-length arrays differ, and the largest difference. */
function differ(a, b) {
  let n = 0, largest = 0;
  for (let i = 0; i < a.length; i++) {
    const d = Math.abs(a[i] - b[i]);
    if (d) n++;
    if (d > largest) largest = d;
  }
  return [n, largest];
}

export default async function (t) {
  const { check, timeout, until, say } = t;
  const arch = 'aarch64';
  let m;
  try {
    m = {
      metal: await bytesOf(t.work(`mink/view/Metal.${arch}`)),
      metalkit: await bytesOf(t.work(`mink/view/MetalKit.${arch}`)),
      program: await bytesOf(t.work(`mink/view/mtkview.${arch}`)),
      library: await bytesOf(t.work('mink/triangle.metallib')),
      text: new TextDecoder().decode(await bytesOf(t.work('mink/view/oracle.txt'))),
      out: await bytesOf(t.work('mink/view/oracle.out')),
    };
  } catch {
    t.skip('no built mink with its view sample, or no pyrus root with QuartzCore (GEX_TEST_MINK: its tools/test-view.sh and tools/test-web.sh build what this needs; GEX_TEST_DARWIN_QC_ROOT)');
  }
  const info = (await navigator.gpu?.requestAdapter())?.info;
  if (!info) t.skip('this browser has no WebGPU adapter');
  const adapter = `${info.vendor ?? '?'} ${info.architecture ?? ''}`.trim();
  const software = /swiftshader|llvmpipe|software/i.test(`${adapter} ${info.description ?? ''}`) || info.isFallbackAdapter === true;
  const exactly = software ? 'but for edge pixels' : 'to the byte';

  const sys = new System({ baseUrl: t.baseUrl, arch, debug: t.debug() });
  const fs = new MemoryFilesystem(sys);
  fs.addTar(t.work(`mink/view/root.${arch}.tar`));
  fs.addFiles({
    '/System/Library/Frameworks/Metal.framework/Versions/A/Metal': m.metal,
    '/System/Library/Frameworks/MetalKit.framework/Versions/A/MetalKit': m.metalkit,
    '/work/mtkview': m.program,
    '/work/triangle.metallib': m.library,
  }, { mode: 0o755 });
  const canvas = document.getElementById('out');
  const out = new CompositorWindows(sys, { canvas });
  const worker = new Worker(t.work('mink/mink-worker.js'), { type: 'module', name: 'mink' });
  let failed = null;
  worker.onmessage = (e) => { if (e.data.kind === 'failed') failed = e.data.error; };
  worker.postMessage({ kind: 'init', glue: new URL(t.work('mink/mink_host.js'), location.href).href });
  new Metal(sys, { replayer: (c) => worker.postMessage({ kind: 'open', id: c.id, toHost: c.toHost, toGuest: c.toGuest, wake: c.wake }, [c.wake]) });
  await timeout(sys.boot(), 120000, 'boot');
  const env = { PATH: '/usr/bin:/bin', HOME: '/', PYRUS_APPKIT_MENUBAR: '0', ...out.env };

  // Offscreen: the text and every frame read back.
  const r = await timeout(sys.run(['/work/mtkview', 'triangle.metallib', 'view.out'], { env, cwd: '/work' }), 600000, 'the sample, offscreen');
  let got = new Uint8Array(0);
  try {
    got = await fs.readFile('/work/view.out');
  } catch {}
  check('offscreen, it prints what it printed on a Mac', r.code === 0 && r.stdout === m.text,
    `exit ${r.code}; ${failed ?? ''} ${(r.stdout + r.stderr).trim().split('\n').slice(-4).join(' / ')}`.slice(0, 600));
  const [n, largest] = got.length === m.out.length ? differ(got, m.out) : [-1, 0];
  check(`and its frames (${m.out.length} bytes, on ${adapter}) are Apple's Metal's, ${exactly}`,
    got.length === m.out.length && (software ? n <= m.out.length / 50 : n === 0), `${got.length} bytes, ${n} differ (the largest by ${largest})`);

  // In a window: the view's timer draws eight frames and the last stays.
  const want = records(m.text, m.out).get('frame07');   // BGRA
  const p = sys.spawn(['/work/mtkview', 'triangle.metallib', '-', '--window', '8', '60'], { env, cwd: '/work' });
  let text = '';
  const read = async (s) => {
    const dec = new TextDecoder();
    for (const rd = s.getReader(); ;) {
      const { value, done } = await rd.read();
      if (done) return;
      text += dec.decode(value, { stream: true });
    }
  };
  const reading = Promise.all([read(p.stdout), read(p.stderr)]);
  await until(() => /^drew 8 frames$/m.test(text), 'the view drew its frames', 240000).catch(() => {});
  check('in a window, the view\'s own timer draws its eight frames', /^drew 8 frames$/m.test(text), `${failed ?? ''} ${text.trim().split('\n').slice(-4).join(' / ')}`.slice(0, 600));
  // Where the window is on the canvas is the compositor's business: the
  // frame is looked for. Its corner pixel is the clear colour.
  const box = canvas.getBoundingClientRect();
  let found = null, best = null;
  const look = async () => {
    const shot = await t.pixels({ x: box.left, y: box.top, width: box.width, height: box.height });
    const px = shot.data, W = shot.width, H = shot.height;
    const near = (x, y, k) => Math.abs(px[(y * W + x) * 4] - want[k * 4 + 2]) <= 2 && Math.abs(px[(y * W + x) * 4 + 1] - want[k * 4 + 1]) <= 2 && Math.abs(px[(y * W + x) * 4 + 2] - want[k * 4]) <= 2;
    for (let y = 0; y + SIZE <= H; y++) {
      for (let x = 0; x + SIZE <= W; x++) {
        // The four corners first: the clear colour, and not a larger area of it.
        if (!near(x, y, 0) || !near(x + SIZE - 1, y + SIZE - 1, SIZE * SIZE - 1) || !near(x + SIZE - 1, y, SIZE - 1) || !near(x, y + SIZE - 1, SIZE * (SIZE - 1))) continue;
        let differing = 0, most = 0;
        for (let j = 0; j < SIZE; j++) {
          for (let i = 0; i < SIZE; i++) {
            const a = ((y + j) * W + x + i) * 4, b = (j * SIZE + i) * 4;
            const d = Math.max(Math.abs(px[a] - want[b + 2]), Math.abs(px[a + 1] - want[b + 1]), Math.abs(px[a + 2] - want[b]));
            if (d) differing++;
            if (d > most) most = d;
          }
        }
        if (!best || differing < best.differing) best = { x, y, differing, most };
        if (software ? differing <= SIZE * SIZE / 50 : differing === 0) return (found = { x, y, differing, most });
        // (A window of the clear colour matches at every offset inside it: one try a row is enough.)
        x += SIZE - 1;
      }
    }
    return null;
  };
  await until(look, 'the last frame is on the page', 40000).catch(() => {});
  await t.screenshot?.('view');
  check(`and the page shows the Mac's last frame in its window, ${exactly}`, !!found,
    best ? `the nearest at ${best.x},${best.y}: ${best.differing} pixels differ (the largest by ${best.most})` : 'nothing like it on the canvas');
  if (found) say(`  the frame is at ${found.x},${found.y} of the canvas; ${found.differing} pixels differ`);
  p.kill('SIGTERM');
  worker.terminate();
  await sys.shutdown();
  await reading.catch(() => {});
}
