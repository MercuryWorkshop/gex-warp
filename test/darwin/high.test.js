// A Mac's address space on the page runtime: every kind of address a Mac
// program can be given is at or above 4 GiB, as on a Mac, whose whole
// first 4 GiB is __PAGEZERO (and Swift takes every value below it for a
// spare bit pattern of a reference, an Optional's nil). gex gives a Darwin
// process its addresses in a window 4 GiB above its memory's offsets
// (gex_abi::DARWIN_BASE). fixtures/darwin/high.c in a Mac root on the page:
// malloc's, mmap's (and MAP_FIXED at 4 GiB and above), its own image's,
// dyld's and libSystem's, its stack's, a thread's, a fault's; against what
// this Mac printed (fixtures/darwin/high.mjs). Natively: darwin/high-native.
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { bytesOf } from '../../page/runtime/src/internal.js';
import { compare } from '../fixtures/darwin/high.mjs';

export const meta = { hosts: ['browser', 'node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 600 };

export default async function (t) {
  const arch = t.arch === 'x86' ? 'x86' : 'aarch64';
  let prog, mac;
  try {
    prog = await bytesOf(t.work(`darwin-high.${arch}`));
    mac = new TextDecoder().decode(await bytesOf(t.work(`darwin-high.${arch}.mac`)));
  } catch {
    t.skip(`no Mac root or no Mac compiler for ${t.arch} (GEX_TEST_DARWIN_ROOT${t.arch === 'x86' ? '_X86' : ''}: pyrus builds one)`);
  }
  const { arch: _, ...debug } = t.debug();
  const sys = new System({ baseUrl: t.baseUrl, arch, debug });
  const fs = new MemoryFilesystem(sys);
  fs.addTar(t.work(`darwin-root.${arch}.tar`));
  fs.addFiles({ '/usr/local/bin/high': prog }, { mode: 0o755 });
  await t.timeout(sys.boot(), 120000, 'boot');
  const r = await t.timeout(sys.run(['/usr/local/bin/high'], { env: { PATH: '/usr/bin:/bin', HOME: '/' }, cwd: '/' }), 240000, 'the program');
  if (r.code !== 0) t.say(r.stderr.trim().split('\n').slice(-3).join(' / '));
  compare(t, 'the page', mac, r.stdout, r.code);
  await sys.shutdown();
}
