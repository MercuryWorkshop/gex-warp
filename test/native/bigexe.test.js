// An executable bigger than the space gex's fixed layout gives one below the
// interpreter (PIE_BASE 0x0100_0000 to INTERP_BASE 0x04a0_0000, 58 MiB):
// fixtures/native/bigexe, natively and on the page's shim. Each program
// writes and reads back every page of its .bss and its last byte, grows its
// break and writes it, and takes memory from malloc (bare.c from mmap), then
// says what it saw and where its image and interpreter are.
//
// - A position-independent one, dynamic or static (bare.c: the toolchains'
//   libc.a is not PIC), is placed in the arena (`gex_abi::exe_bias`), its
//   break after it, the interpreter where it always is. The dynamic one execs
//   itself too, so the exec path places it again.
// - A fixed-address one that ends below the stack loads where it is linked,
//   its interpreter, if it reaches that, in the arena; one that reaches the
//   stack is refused, saying so, instead of loaded over it.
import { copyFileSync, mkdirSync } from 'node:fs';
import { pageLegs } from '../lib/guest.mjs';
import { native, trim, tail } from '../lib/native.mjs';

export const meta = { hosts: ['node'], timeout: 1200 };

/** big.c's line: its .bss of `mib` MiB, its image and its interpreter where. */
const libc = (mib, image, interp) => `pages ${(mib << 20) / 4096}, last 7, brk ok, malloc ok, image ${image}, interpreter ${interp}`;

export default async function (t) {
  const n = native(t);
  const d = n.dir('bigexe');
  const [[ld, so]] = Object.entries(n.ldso);
  mkdirSync(`${d}/root/lib`, { recursive: true });
  copyFileSync(so, `${d}/root${ld}`);
  const big = n.src('bigexe/big.c');
  const progs = [
    { name: 'pie', flags: ['-fPIE', '-pie'], dynamic: true, exec: true, want: libc(96, 'arena', 'fixed') },
    {
      name: 'static-pie', src: n.src('bigexe/bare.c'), dynamic: false,
      flags: ['-ffreestanding', '-fno-builtin', '-fno-tree-loop-distribute-patterns', '-fno-stack-protector', '-fPIE', '-static-pie', '-nostdlib'],
      want: 'pages 24576, last 7, brk ok, mmap ok, image arena',
    },
    { name: 'fixed-72', flags: ['-no-pie', '-static', '-DBIG_MIB=72'], dynamic: false, want: libc(72, 'fixed', 'none') },
    { name: 'fixed-dynamic-72', flags: ['-no-pie', '-DBIG_MIB=72'], dynamic: true, exec: true, want: libc(72, 'fixed', 'arena') },
    { name: 'fixed-96', flags: ['-no-pie', '-static'], dynamic: false, refused: true },
  ];
  for (const p of progs) {
    p.path = `${d}/root/${p.name}`;
    await n.cc(['-O1', ...p.flags, '-o', p.path, p.src ?? big]);
  }
  const refusal = /this executable's image \(0x400000\.\.0x[0-9a-f]+\) reaches gex's stack at 0x5000000/;
  const legs = [
    // (a dynamic one by its guest path in the root that holds its
    // interpreter, which is also the argv[0] it execs)
    { label: 'native', run: (p, args) => n.gexc(['run', ...(p.dynamic ? ['--sysroot', `${d}/root`, '--guest', `/${p.name}`] : [p.path]), ...args], { timeout: 300000 }) },
    ...pageLegs(n.g).map((leg) => ({
      label: `page ${leg.label}`,
      run: (p, args) => n.g.run(leg.seed, p.path, args, {
        files: { ...(p.dynamic ? n.ldso : {}), [p.path]: p.path }, cwd: d, env: leg.env, single: leg.single, timeout: 300000,
      }),
    })),
  ];
  for (const leg of legs) {
    for (const p of progs) {
      const args = p.exec ? ['exec'] : [];
      const r = await leg.run(p, args);
      const what = `${leg.label}: ${p.name}`;
      if (p.refused) {
        t.check(`${what}: refused, saying it reaches the stack`, r.code !== 0 && refusal.test(r.stderr), `exit ${r.code}: ${tail(r.stderr)}`);
        continue;
      }
      // (the page's guest lines are its stdout without the runtime's own)
      const out = trim(leg.label === 'native' ? r.stdout : r.guest);
      t.eq(`${what}: output`, out, p.exec ? `${p.want}\n${p.want}` : p.want, tail(r.stderr));
      t.check(`${what}: exits 0`, r.code === 0, `exit ${r.code}: ${tail(r.stderr)}`);
    }
  }
  n.clean();
}
