// The `codecs` service's audio (page/runtime codecs.js) under pyrus's
// AudioToolbox: pyrus's test/media/atcodec (fixtures/darwin/audio-cases.mjs),
// built for a Mac and run on this Mac against its own AudioToolbox when the
// images were made, run again in a Mac root on the page, whose codecs are
// the browser's WebCodecs AudioEncoder and AudioDecoder. What it prints
// must be what it printed on the Mac: the descriptions a converter
// completes, its cookies, bit rates and priming, how many packets an
// encoder makes and how many frames a decoder makes of them, and where in
// what it decodes the sound is. The sound itself is each codec's own, and
// is judged by the program (near what was encoded), not compared. A case
// whose codec this browser has not got is said and skipped, and one that
// asks of a codec what the browser's lacks is said (fixtures' `differs`).
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { Codecs } from '../../page/runtime/src/codecs.js';
import { bytesOf } from '../../page/runtime/src/internal.js';
import { CASES, SOUNDS, argsOf } from '../fixtures/darwin/audio-cases.mjs';

export const meta = { hosts: ['browser'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 1800 };

const text = (b) => new TextDecoder().decode(b);

export default async function (t) {
  const { eq, check, timeout } = t;
  const arch = t.arch === 'x86' ? 'x86' : 'aarch64';
  const dir = `darwin-audio.${arch}`;
  let prog;
  try {
    prog = await bytesOf(t.work(`${dir}/atcodec`));
  } catch {
    t.skip(`no Mac root, no pyrus test/media, or no Mac compiler for ${t.arch} (GEX_TEST_DARWIN_ROOT${t.arch === 'x86' ? '_X86' : ''}, GEX_TEST_PYRUS_MEDIA)`);
  }
  if (typeof AudioDecoder !== 'function') t.skip('no WebCodecs in this browser');
  // What this browser has, said.
  const can = {};
  for (const codec of ['mp4a.40.2', 'mp4a.40.5', 'opus', 'mp3']) {
    const d = (await AudioDecoder.isConfigSupported({ codec, sampleRate: 48000, numberOfChannels: 2 }).catch(() => ({}))).supported;
    const e = (await AudioEncoder.isConfigSupported({ codec, sampleRate: 48000, numberOfChannels: 2 }).catch(() => ({}))).supported;
    can[codec] = { d, e };
    t.say(`this browser: ${codec} decoding ${d ? 'yes' : 'no'}, encoding ${e ? 'yes' : 'no'}`);
  }

  const { arch: _, ...debug } = t.debug();
  const sys = new System({ baseUrl: t.baseUrl, arch, debug });
  const fs = new MemoryFilesystem(sys);
  fs.addTar(t.work(`darwin-root.${arch}.tar`));
  fs.addFiles({ '/usr/local/bin/atcodec': prog }, { mode: 0o755 });
  const sounds = {};
  for (const f of SOUNDS) {
    const b = await bytesOf(t.work(`darwin-audio/${f}`)).catch(() => null);
    if (b) sounds[`/sounds/${f}`] = b;
  }
  for (const c of CASES.filter((c) => c.packets)) {
    const b = await bytesOf(t.work(`${dir}/${c.packets}`)).catch(() => null);
    if (b) sounds[`/packets/${c.packets}`] = b;
  }
  fs.addFiles(sounds);
  new Codecs(sys);
  await timeout(sys.boot(), 120000, 'boot');
  check('programs have the service node', sys.services.available);

  for (const c of CASES) {
    const has = can[c.codec];
    if (!has?.d || (c.encode && !has.e)) {
      t.say(`${c.name}: skipped, this browser has no ${c.codec} ${c.encode ? 'encoder' : 'decoder'}`);
      continue;
    }
    let want;
    try {
      want = text(await bytesOf(t.work(`${dir}/${c.name}.mac`)));
    } catch {
      t.say(`${c.name}: no output of this Mac's to compare with`);
      continue;
    }
    const t0 = performance.now();
    const r = await timeout(sys.run(['/usr/local/bin/atcodec', ...argsOf(c, '/sounds', '/packets')], { env: { PATH: '/usr/bin:/bin', HOME: '/', ATCODEC_SAY: '1' }, cwd: '/' }), 600000, c.name);
    t.record(c.name, Math.round(performance.now() - t0));
    const said = r.stderr.trim().split('\n').slice(-4).join(' / ');
    if (said) t.say(`${c.name}: ${said}`);
    if (c.differs) t.known(`${c.name}: as on the Mac`, r.stdout === want, said, c.differs);
    else eq(`${c.name}: as on the Mac`, r.stdout, want, said);
  }
  await sys.shutdown();
}
