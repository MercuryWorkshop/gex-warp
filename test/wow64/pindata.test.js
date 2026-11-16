// A memory64 page process's data exports from a DLL its prefix pins in the
// band (dll-bases, [0x11000000, 0x20000000)) are within the 32-bit RVA reach
// of the image: msvcr120 pinned, its data exports by GetProcAddress against the
// addresses its functions give (pindata.c). The page's shim, memory64 only.
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { wow64 } from '../lib/winerun.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 900 };

export default async function (t) {
  const w = wow64(t);
  const exe = await w.mingw('pindata', { x64: true, flags: ['-Wall'] });
  const r = await w.page(['Z:\\t\\pindata.exe'], {
    files: { '/t/pindata.exe': exe }, mem64: true, rel: 'lib/wine/x86_64-unix/wine.m64', cwd: '/t',
    setup: (p) => writeFileSync(join(p, 'dll-bases'), '17670000 /wine32/lib/wine/x86_64-windows/msvcr120.dll\n'),
  });
  for (const l of r.out.split('\n').filter((l) => /^pindata /.test(l))) t.say(l);
  t.check('pindata ok', /^pindata ok/m.test(r.out), w.detail(r));
  t.check('pinned msvcr120', /^pindata base 0*17670000$/mi.test(r.out), w.detail(r));
  w.clean();
}
