// Window procedure calls nested deep (cbnest.c): each level a user callback
// from native win32u into the i386 program, a call-out on the page, with
// written code run, rewritten, freed and made again partway down and on the
// way back up, so translations are installed and retired while the
// call-outs above are open and each one's watch must still catch its return.
// Natively and on the page's shim: 60 levels, three rounds, must all return
// in order ("cbnest ok 60 3", exit 0).
import { graphics, wow64 } from '../lib/winerun.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 900 };

export default async function (t) {
  const w = wow64(t);
  const exe = await w.mingw('cbnest');
  const line = /^cbnest ok 60 3/m;
  const n = await w.native(['C:\\cbnest.exe', '60', '3'], { files: { 'cbnest.exe': exe }, userReg: graphics('null'), name: 'native' });
  t.check('native: 60 levels, three rounds, all returned', line.test(n.out) && n.code === 0, w.detail(n));
  const p = await w.page(['/t/cbnest.exe', '60', '3'], { files: { '/t/cbnest.exe': exe }, userReg: graphics('null') });
  t.check('page: 60 levels, three rounds, all returned', line.test(p.out) && p.code === 0, w.detail(p));
  w.clean();
}
