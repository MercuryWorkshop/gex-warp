// An image is relocated where it is mapped, before anything runs in it, as
// Windows maps one (fixtures/wine/src/imgreloc.c): a program's own .exe
// mapped twice more as an image section, which no loader relocates (each
// view for its own address, none through another's pages), and a copy
// of it started suspended, which its parent reads (its PEB's base, then its
// header and data there) before the child's loader has run. Natively, and on
// the page in a 64-bit memory, where a program that can move goes top down,
// away from its base, so the child is one that moved.
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { wow64 } from '../lib/winerun.mjs';

const SRC = fileURLToPath(new URL('../fixtures/wine/src/imgreloc.c', import.meta.url));

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 900 };

const lines = (out) => out.replace(/\r/g, '').split('\n').filter((l) => l.startsWith('imgreloc ')).join('\n');
const WANT = 'imgreloc map ok\nimgreloc map ok\nimgreloc child ok\nimgreloc done';

export default async function (t) {
  const w = wow64(t);
  const exe = await w.mingw('imgreloc', { x64: true, src: SRC, flags: ['-Wl,--dynamicbase,--high-entropy-va'], out: 'imgreloc.exe' });

  const n = await w.native(['C:\\imgreloc.exe'], { files: { 'imgreloc.exe': exe }, name: 'native' });
  t.eq('native: a second mapping and a suspended child are relocated where they are', lines(n.out), WANT, w.detail(n));

  // the memory64 twin (WINE64_TWIN=1); 600 s: its first start compiles its modules
  if (!existsSync(join(process.env.GEX_TEST_WOW64_DIST, 'lib/wine/x86_64-unix/wine.m64'))) t.skip('the WoW64 install has no memory64 twin (WINE64_TWIN=1)');
  const p = await w.page(['/t/imgreloc.exe'], { files: { '/t/imgreloc.exe': exe }, name: 'page-m64', timeout: 600000, wineEnv: { GEX_MEM64: '1' } });
  t.eq('page mem64: a second mapping and a suspended child that moved are relocated where they are', lines(p.out), WANT, w.detail(p));
  w.clean();
}
