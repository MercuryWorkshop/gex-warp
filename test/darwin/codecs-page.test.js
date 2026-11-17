// The `codecs` service (page/runtime codecs.js) under pyrus's VideoToolbox:
// pyrus's test/media programs (fixtures/darwin/codecs-cases.mjs), built for
// a Mac and run on this Mac against its own VideoToolbox when the images
// were made, run again in a Mac root on the page, whose decoders and
// encoders are the browser's WebCodecs. What they print must be what they
// printed on the Mac: H.264 and HEVC decoding is exactly specified, so each
// decoded plane's hash is the same; where a frame's pixels are converted
// by each side's own arithmetic (a range the stream has not got, RGB), the
// bytes are measured against the Mac's and must be within a stated
// tolerance; asynchronous decoding may give the frames in another order
// (the browser's in presentation order), which is said, not compared; an
// encoder's output is judged by what the program says of it (its key
// frames, its size against the bit rate, the PSNR of it decoded again),
// not by its bytes. The cases are Google Chrome's: Chromium has no H.264 or
// HEVC, and there a program is only seen to find no decoder for H.264.
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { Codecs } from '../../page/runtime/src/codecs.js';
import { bytesOf } from '../../page/runtime/src/internal.js';
import { CASES, CLIPS } from '../fixtures/darwin/codecs-cases.mjs';

export const meta = { hosts: ['browser'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 3600 };

const text = (b) => new TextDecoder().decode(b);
// A frame line without its planes' hashes (for a measured case).
const unhashed = (s) => s.replace(/:[0-9a-f]{16}/g, ':#').replace(/ [0-9a-f]{16}( |$)/g, ' #$1');
const unordered = (s) => s.split('\n').filter((l) => !l.startsWith('came in order')).join('\n');

export default async function (t) {
  const { eq, check, timeout } = t;
  const arch = t.arch === 'x86' ? 'x86' : 'aarch64';
  const dir = `darwin-codecs.${arch}`;
  let decode, encode;
  try {
    decode = await bytesOf(t.work(`${dir}/vtdecode`));
    encode = await bytesOf(t.work(`${dir}/vtencode`));
  } catch {
    t.skip(`no Mac root, no pyrus test/media, or no Mac compiler for ${t.arch} (GEX_TEST_DARWIN_ROOT${t.arch === 'x86' ? '_X86' : ''}, GEX_TEST_PYRUS_MEDIA)`);
  }
  if (typeof VideoDecoder !== 'function') t.skip('no WebCodecs in this browser');
  // What this browser has, said.
  const can = {};
  for (const [name, codec] of [['H.264', 'avc1.42c00d'], ['HEVC', 'hvc1.1.6.L60.90'], ['VP8', 'vp8'], ['VP9', 'vp09.00.10.08'], ['AV1', 'av01.0.04M.08']]) {
    const d = (await VideoDecoder.isConfigSupported({ codec }).catch(() => ({}))).supported;
    const e = (await VideoEncoder.isConfigSupported({ codec, width: 320, height: 180 }).catch(() => ({}))).supported;
    can[name] = d;
    t.say(`this browser: ${name} decoding ${d ? 'yes' : 'no'}, encoding ${e ? 'yes' : 'no'}`);
  }
  const h264 = can['H.264'], hevc = can.HEVC;

  const { arch: _, ...debug } = t.debug();
  const sys = new System({ baseUrl: t.baseUrl, arch, debug, services: { bulk: 4 } });
  const fs = new MemoryFilesystem(sys);
  fs.addTar(t.work(`darwin-root.${arch}.tar`));
  fs.addFiles({ '/usr/local/bin/vtdecode': decode, '/usr/local/bin/vtencode': encode }, { mode: 0o755 });
  const clips = {};
  for (const c of CLIPS) clips[`/clips/${c}`] = await bytesOf(t.work(`darwin-codecs/${c}`));
  fs.addFiles(clips);
  new Codecs(sys);
  await timeout(sys.boot(), 120000, 'boot');
  check('programs have the service node', sys.services.available);

  if (!h264) {
    // (Chromium's: no H.264 at all. A program finds no decoder for it, as
    // on a Mac without one; the rest of the cases are Chrome's.)
    const r = await timeout(sys.run(['/usr/local/bin/vtdecode', '/clips/base.h264', 'sync', 'none'], { env: { PATH: '/usr/bin:/bin', HOME: '/' }, cwd: '/' }), 600000, 'no H.264');
    eq('without H.264, no decoder for it', r.stdout.split('\n').slice(2).join('\n'), 'hardware decode of avc1: no\nsession: -12906\n');
    await sys.shutdown();
    return;
  }
  for (const c of CASES) {
    if (c.args[0] === 'main.hevc' && !hevc) {
      t.say(`${c.name}: skipped, this browser decodes no HEVC`);
      continue;
    }
    let want;
    try {
      want = text(await bytesOf(t.work(`${dir}/${c.name}.mac`)));
    } catch {
      t.say(`${c.name}: no output of this Mac's to compare with`);
      continue;
    }
    const args = c.prog === 'vtdecode' ? [`/clips/${c.args[0]}`, ...c.args.slice(1)] : c.args;
    const env = { PATH: '/usr/bin:/bin', HOME: '/', ...(c.measure ? { VTDECODE_DUMP: `/tmp/${c.name}.raw` } : {}) };
    const t0 = performance.now();
    const r = await timeout(sys.run([`/usr/local/bin/${c.prog}`, ...args], { env, cwd: '/' }), 600000, c.name);
    t.record(c.name, Math.round(performance.now() - t0));
    const tail = r.stderr.trim().split('\n').slice(-3).join(' / ');
    if (c.order) {
      eq(`${c.name}: as on the Mac, but for the order frames came in`, unordered(r.stdout), unordered(want), tail);
      const got = r.stdout.split('\n').find((l) => l.startsWith('came in order')) ?? '';
      t.say(`${c.name}: ${got} (the Mac's: ${want.split('\n').find((l) => l.startsWith('came in order'))})`);
    } else if (c.measure) {
      eq(`${c.name}: as on the Mac, but for the bytes of converted pixels`, unhashed(r.stdout), unhashed(want), tail);
      const mine = await fs.readFile(`/tmp/${c.name}.raw`).catch(() => new Uint8Array());
      const mac = await bytesOf(t.work(`${dir}/${c.name}.mac.raw`)).catch(() => new Uint8Array());
      check(`${c.name}: the frames' bytes are all there`, mac.length > 0 && mine.length === mac.length, `${mine.length} and the Mac's ${mac.length}`);
      // Each plane of every frame apart: [name, bytes, within, most].
      const frame = c.measure.planes.reduce((a, p) => a + p[1], 0);
      let at = 0;
      for (const [plane, size, within, most] of c.measure.planes) {
        let max = 0, sum = 0, near = 0, n = 0;
        for (let f = 0; f + frame <= Math.min(mine.length, mac.length); f += frame)
          for (let i = f + at; i < f + at + size; i++) {
            const d = Math.abs(mine[i] - mac[i]);
            if (d > max) max = d;
            sum += d;
            if (d <= 1) near++;
            n++;
          }
        at += size;
        t.say(`${c.name} ${plane}: ${n} bytes, by ${max} at most from the Mac's, ${(sum / (n || 1)).toFixed(3)} on average, ${((100 * near) / (n || 1)).toFixed(2)}% within 1`);
        t.record(`${c.name} ${plane} max difference`, max);
        if (within < 255) check(`${c.name} ${plane}: within ${within} of the Mac's, and ${most * 100}% within 1`, n > 0 && max <= within && near >= most * n, `by ${max} at most`);
      }
    } else {
      eq(`${c.name}: as on the Mac`, r.stdout, want, tail);
    }
  }
  await sys.shutdown();
}
