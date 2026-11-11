// Descriptors passed over a unix socket (SCM_RIGHTS), as Linux passes them:
// fixtures/darwin/fdpass.c in one process -- with the first byte they were
// sent with, across partial reads, two sends read as one, no room for them,
// read(2) before recvmsg, a memfd's seals, MSG_CMSG_CLOEXEC, msg_controllen
// counted with its padding -- and fixtures/darwin/fdpass-fork.c from a child
// to its parent: a memfd written, one filled through a mapping and sealed
// (an image loader's frame), a pipe's end.
//
// Linux programs (musl, GEX_TOOLCHAIN, _X86), under gexc and on the page
// (run-node: each process a Worker with a memory of its own, the socket and
// what is passed over it in the global kernel), against what Linux printed
// (fixtures/darwin/fdpass*.linux.txt: Alpine in Docker). On the page a memfd
// passed to another process arrived as nothing: by its name alone, which the
// receiver has no file for (GTK's image loaders hand each frame over so, and
// no GTK program got as far as a window).
//
// Said and not checked: a peek hands descriptors over on Linux and not
// here; what a sender stores through its mapping after the send is not the
// receiver's until it is written back (no memory is shared between two
// processes' mappings made apart); on the page, /proc/self/exe passed is
// not the sender's, and descriptors dropped for want of a control buffer
// are not said (MSG_CTRUNC).
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

function compare(t, what, want, got, known) {
  const a = want.split('\n');
  const b = got.split('\n');
  const off = [];
  const said = [];
  for (let k = 0; k < Math.max(a.length, b.length); k++) {
    if (a[k] === b[k]) continue;
    (known.test(a[k] ?? '') ? said : off).push(`line ${k + 1}: want ${JSON.stringify(a[k] ?? null)}, got ${JSON.stringify(b[k] ?? null)}`);
  }
  for (const l of said) t.say(`${what}, not checked: ${l}`);
  t.check(`${what}: as on Linux (${a.length - 1} lines)`, off.length === 0, off.slice(0, 3).join(' / '));
}

export default async function (t) {
  const x86 = t.arch === 'x86';
  const n = native(t);
  const d = n.dir('fdpass');
  const seed = join(ROOT, 'page/public/gex', `seed${x86 ? '.x86' : ''}.shared.wasm`);
  for (const [name, nativeKnown, pageKnown] of [
    ['fdpass', /^peek 4:/, /^peek 4:|^read with no control buffer:/],
    ['fdpass-fork', /^live, after the child wrote again:/, /^live, after the child wrote again:|^file:/],
  ]) {
    const prog = join(d, name);
    await n.cc(['-O1', '-static', '-Wl,-Ttext-segment=0x1000000', '-o', prog, join(ROOT, 'test/fixtures/darwin', `${name}.c`)]);
    const want = readFileSync(join(ROOT, 'test/fixtures/darwin', `${name}.linux.txt`), 'utf8');
    const lines = (s) => s.split('\n').filter((l) => !l.startsWith('[')).join('\n');
    const g = await n.gexc(['run', prog], { timeout: 200000 });
    compare(t, `${name} under gexc`, want, lines(g.stdout), nativeKnown);
    if (!existsSync(seed)) {
      t.say(`${name} on the page: no ${seed} (hosts/browser/build.sh)`);
      continue;
    }
    const p = await n.g.run(seed, prog, [], { env: { GEX_SHARED: '1' }, timeout: 240000 });
    compare(t, `${name} on the page`, want, lines(p.stdout), pageKnown);
  }
}
