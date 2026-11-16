// A memory64 page process whose prefix's dll-bases pins the DLLs of gex's
// core PE module (wow64, wow64cpu, wow64win) in the band: that module's data
// is shared, at the top of the space, so ntdll maps those DLLs above 4 GiB
// instead (gex.image_floats), within their data exports' 32-bit RVA reach.
// An x86_64 program loads wow64win and reads its sdwhwin32 (pinwow.c). (An
// i386 program does not run in a 64-bit memory: the i386 builtins have no
// memory64 modules.)
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { wow64 } from '../lib/winerun.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 900 };

const PINS = ['wow64', 'wow64cpu', 'wow64win'].map((d, i) => `${(0x15000000 + i * 0x100000).toString(16)} /wine32/lib/wine/x86_64-windows/${d}.dll`);

export default async function (t) {
  const w = wow64(t);
  const exe = await w.mingw('pinwow', { x64: true, flags: ['-Wall'] });
  const r = await w.page(['Z:\\t\\pinwow.exe'], {
    files: { '/t/pinwow.exe': exe }, mem64: true, rel: 'lib/wine/x86_64-unix/wine.m64', cwd: '/t', timeout: 300000,
    setup: (p) => writeFileSync(join(p, 'dll-bases'), PINS.join('\n') + '\n'),
  });
  for (const l of r.out.split('\n').filter((l) => /^pinwow/.test(l))) t.say(l);
  t.check('pinwow ok', /^pinwow ok/m.test(r.out), w.detail(r));
  const base = /^pinwow base ([0-9a-f]+)/mi.exec(r.out);
  t.check('wow64win above 4 GiB, not at its pin', base && BigInt(`0x${base[1]}`) >= 1n << 32n, base?.[0]);
  t.check('no data export out of reach', !/out of its 32-bit RVA's reach/.test(r.out), r.out.split('\n').filter((l) => /RVA's reach/.test(l)).slice(0, 3).join(' / '));
  w.clean();
}
