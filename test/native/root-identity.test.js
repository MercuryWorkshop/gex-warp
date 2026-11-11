// Two roots, each with a library of the same name and layout but other code
// (fixtures/native/root-identity: /lib/libv.so adds 1 in /r1 and 2 in /r2),
// and a process `chroot`ed into each: the first still up while the second
// starts. A code identity is a file's name as its process has it, and both
// name their library /lib/libv.so, at the same address: the second took the
// first's work for its own (slot numbering, a translation finished). Each
// root's name is in the identity now (lib.rs `root_salt`), and each process
// gets its own library's sum. A process at the system's root keys as before.
import { join } from 'node:path';
import { native } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 300 };

const N = 200000;

export default async function (t) {
  const n = native(t);
  const d = n.dir('root-identity');
  const src = (f) => n.src(`root-identity/${f}`);
  await n.cc(['-O1', '-shared', '-fPIC', '-DV=1', '-o', join(d, 'libv1.so'), src('libv.c')]);
  await n.cc(['-O1', '-shared', '-fPIC', '-DV=2', '-o', join(d, 'libv2.so'), src('libv.c')]);
  await n.cc(['-O1', '-c', '-fPIC', '-o', join(d, 'prog.o'), src('prog.c')]);
  // Linked against a libv.so (either: the same symbols), found at /lib.
  await n.cc(['-O1', '-shared', '-fPIC', '-DV=1', '-o', join(d, 'libv.so'), src('libv.c')]);
  await n.cc(['-o', join(d, 'prog'), join(d, 'prog.o'), `-L${d}`, '-lv']);
  await n.cc(['-O1', '-static', '-Wl,-Ttext-segment=0x1000000', '-o', join(d, 'runner'), src('runner.c')]);
  const [ld, ldHost] = Object.entries(n.ldso)[0];
  const files = { '/runner': join(d, 'runner') };
  for (const [root, lib] of [['/r1', 'libv1.so'], ['/r2', 'libv2.so']]) {
    files[`${root}${ld}`] = ldHost;
    files[`${root}/lib/libv.so`] = join(d, lib);
    files[`${root}/prog`] = join(d, 'prog');
  }
  const r = await n.g.run(n.seed(), join(d, 'runner'), [], { files, argv0: '/runner', env: { GEX_SHARED: '1', GEX_UID: '0' }, timeout: 240000 });
  const said = r.stdout.split('\n').filter((l) => !l.startsWith('[')).join('\n');
  t.eq('each root\'s process sums with its own library', [said, r.code], [[
    `/r1: sum ${N}`,
    `/r2: sum ${2 * N}`,
    '/r2 ended 0',
    '/r1 ended 0',
    '',
  ].join('\n'), 0]);
  n.clean();
}
