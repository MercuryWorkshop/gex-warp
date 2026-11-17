// GL's window default on the page's Mesa: a context's viewport is the size
// of the window it is bound to. The stream fixtures/wine/src/gltri.c sends
// through Wine's opengl32 -- a desktop context, the window's surface attached
// and then sized (SURFACE_ATTACH, SURFACE_RESIZE), a grey clear and a yellow
// triangle in immediate mode, never a glViewport -- replayed on WebGPU on a
// drawable that starts at 8x8, and the 64x48 window read back: the triangle
// in its middle, about 384 pixels of it. Left at the drawable's size before
// the window's own, the viewport drew it as a speck in the bottom-left corner.
// The mt build's module unsplit (the `mesa-cold` image), or the st build's
// without SharedArrayBuffer.

export const meta = { hosts: ['browser'], arches: ['x86'], images: ['mesa-cold'], timeout: 600 };

const W = 64, H = 48;
const GREY = [51, 51, 51], YELLOW = [230, 210, 40];

/** One batch: [opcode, words...] each, as the guest writes them. */
function batch(cmds) {
  const words = cmds.flatMap(([op, ...args]) => [op, (args.length + 2) * 4, ...args]);
  const b = new Uint8Array(words.length * 4);
  const v = new DataView(b.buffer);
  words.forEach((w, i) => v.setUint32(i * 4, w >>> 0, true));
  return b;
}
const f = (x) => { const v = new DataView(new ArrayBuffer(4)); v.setFloat32(0, x, true); return v.getUint32(0, true); };

/** A canvas whose WebGPU context draws into a texture (as mesa-cold's). */
function textureCanvas() {
  const canvas = new OffscreenCanvas(W, H);
  let config = null;
  let texture = null;
  const context = {
    canvas,
    configure(c) { config = c; },
    unconfigure() { config = null; },
    getConfiguration() { return config; },
    getCurrentTexture() {
      if (!texture || texture.width !== canvas.width || texture.height !== canvas.height) {
        texture?.destroy();
        texture = config.device.createTexture({
          size: [canvas.width, canvas.height], format: config.format, viewFormats: config.viewFormats ?? [],
          usage: (config.usage ?? GPUTextureUsage.RENDER_ATTACHMENT) | GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.COPY_SRC,
        });
      }
      return texture;
    },
  };
  const own = canvas.getContext.bind(canvas);
  canvas.getContext = (type, o) => (type === 'webgpu' ? context : own(type, o));
  return canvas;
}

export default async function (t) {
  const build = typeof SharedArrayBuffer === 'function' ? 'mt' : 'st';
  const mesa = t.work(`mesa-cold/${build}/whole/gex-wgpu-replay.js`);
  const adapter = await navigator.gpu.requestAdapter();
  if (!t.check('a WebGPU adapter', !!adapter)) return;
  const device = await adapter.requestDevice();
  let validation = 0;
  device.addEventListener('uncapturederror', (e) => { if (validation++ < 5) t.say(`  WebGPU validation: ${e.error?.message}`); });
  const glCanvas = textureCanvas();
  const qs = document.querySelector.bind(document);
  document.querySelector = (s) => (String(s).includes('gex-gl') ? glCanvas : qs(s));
  const { default: create } = await import(/* @vite-ignore */ mesa);
  const m = await create({ preinitializedWebGPUDevice: device, mainScriptUrlOrBlob: mesa, locateFile: (x) => new URL(x, mesa).href, print: () => {}, printErr: () => {} });
  const submit = WebAssembly.promising(m.gexGlSubmitRaw);
  // the host's drawable before the window says its size
  if (!t.check('gex_gl_init', m._gex_gl_init(8, 8) === 0)) return;
  const ptr = m._malloc(1 << 16);
  let rejects = 0;
  const send = async (cmds) => { const b = batch(cmds); m.HEAPU8.set(b, ptr); if ((await submit(ptr, b.length, 0)) !== 0) rejects++; };
  const COLOR3F = 8432, VERTEX2F = 11057;
  await send([
    [1, 1, 2, 1, 1], // CONTEXT_CREATE: desktop 2.1, compatibility
    [5, 0, 7, 1, 1], // SURFACE_ATTACH: a Wayland surface, handle 1
    [2, W, H], // SURFACE_RESIZE: the window's size
    [16, f(51 / 255), f(51 / 255), f(51 / 255), f(1)], [17, 0x4000],
    [173, 4], [COLOR3F, f(230 / 255), f(210 / 255), f(40 / 255)],
    [VERTEX2F, f(-0.5), f(-0.5)], [VERTEX2F, f(0.5), f(-0.5)], [VERTEX2F, f(0), f(0.5)], [174],
  ]);
  // in a batch of its own, so the reply is the pixels alone
  await send([[128, 0, 0, W, H, 0x1908, 0x1401, 0, 0, 0]]); // READ_PIXELS, RGBA bytes
  await device.queue.onSubmittedWorkDone();
  const rlen = m._gex_gl_reply_len();
  const rat = m._gex_gl_reply_data();
  const px = m.HEAPU8.slice(rat + 8, rat + rlen);
  t.check('no rejected batch or validation error', rejects === 0 && validation === 0, `${rejects} rejected, ${validation} validation errors`);
  if (!t.eq('the read-back is the window', px.length, W * H * 4)) return;
  const at = (x, y) => [...px.subarray((y * W + x) * 4, (y * W + x) * 4 + 3)];
  const is = (p, c) => p.every((v, i) => Math.abs(v - c[i]) <= 1);
  let yellow = 0, grey = 0;
  for (let i = 0; i < W * H; i++) { const p = [...px.subarray(i * 4, i * 4 + 3)]; if (is(p, YELLOW)) yellow++; else if (is(p, GREY)) grey++; }
  t.say(`  ${build}: ${yellow} yellow, ${grey} grey of ${W * H}; middle ${at(W / 2, H / 2)}, corner ${at(2, 2)}`);
  t.check('the middle of the window is the triangle', is(at(W / 2, H / 2), YELLOW), String(at(W / 2, H / 2)));
  t.check('the bottom-left corner is the clear colour', is(at(2, 2), GREY), String(at(2, 2)));
  t.check('the triangle is a quarter of the window, not a speck (300 pixels or more)', yellow >= 300, `${yellow} yellow pixels`);
  t.check('and nothing else (one colour besides the clear one)', yellow + grey === W * H, `${W * H - yellow - grey} pixels of other colours`);
}
