// Wineserver round trips for an i386 program (fixtures/wow64/srvlat.c), a
// WoW64 process: a bare request, a message to another thread's window,
// hidden and shown windows created and destroyed, a shown window's text set,
// in µs per operation. Natively with a Wayland compositor of gexc's own,
// natively on the null display driver, and on the page's shim on the null
// driver (it has no display here). Each is checked against a ceiling well
// above what it costs on a loaded host, to catch an operation that went
// back to costing a turn of a wait loop or worse. The numbers are said, for
// a run on a quiet host to compare.
import { graphics, wow64 } from '../lib/winerun.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 900 };

const OPS = ['req', 'send', 'hidden', 'shown', 'text'];
// µs per operation
const CEILING = { req: 200, send: 10000, hidden: 20000, shown: 200000, text: 20000 };

export default async function (t) {
  const w = wow64(t);
  const exe = await w.mingw('srvlat', { libs: ['-lntdll'] });
  const files = { 'srvlat.exe': exe };
  const legs = [
    ['native', () => w.native(['C:\\srvlat.exe', '3'], { files, wayland: true })],
    ['native, null driver', () => w.native(['C:\\srvlat.exe', '3'], { files, userReg: graphics('null'), name: 'native-null' })],
    ['page', () => w.page(['/t/srvlat.exe', '3'], { files: { '/t/srvlat.exe': exe }, userReg: graphics('null') })],
  ];
  for (const [leg, run] of legs) {
    const r = await run();
    const m = /^srvlat req ([\d.]+) send ([\d.]+) hidden ([\d.]+) shown ([\d.]+) text ([\d.]+)/m.exec(r.out);
    t.check(`${leg}: every operation ran`, !!m, w.detail(r));
    if (!m) continue;
    const us = Object.fromEntries(OPS.map((k, i) => [k, Number(m[i + 1])]));
    t.say(`${leg}: ${OPS.map((k) => `${k} ${us[k]} µs`).join(', ')}`);
    for (const k of OPS) t.check(`${leg}: ${k} ${us[k]} µs/op under ${CEILING[k]}`, us[k] < CEILING[k]);
  }
  w.clean();
}
