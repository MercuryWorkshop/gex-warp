// Code running across a page boundary while the second page changes between
// calls (pagecross.c): its protection toggled, data beside the code written,
// the page decommitted and committed again, or decommitted so the call's
// fetch there faults to a vectored handler that maps it back. A block that
// runs onto a page whose translation is retired is lifted again or faults
// as Windows would, and never comes back to translation for ever: each case
// ends with every call right; and an immediate rewritten before each call,
// whose page enters the hot-rewrite mode and, quiet, leaves it; and one of
// 16 blocks rewritten before each call while a second thread runs the page
// and, at every eighth rewrite, the rewritten function. The i386
// program and the same program built for x86_64, natively and on the page.
import { wow64 } from '../lib/winerun.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 1500 };

export default async function (t) {
  const w = wow64(t);
  const want = /^pagecross ok/m;
  const timeout = 600000;
  const exe = await w.mingw('pagecross');
  const exe64 = await w.mingw('pagecross', { out: 'pagecross64.exe', x64: true });
  // Natively without the rewritten immediate: the native host has no
  // hot-rewrite mode, and lifts its page's 1000 blocks again at every one of
  // the 2000 rewrites (a debugging host; the mode is the page's).
  const n = await w.native(['C:\\pagecross.exe', '2000', '0', 'noimm'], { files: { 'pagecross.exe': exe }, timeout });
  t.check(`native: ${want.source}`, want.test(n.out), w.detail(n));
  const n64 = await w.native(['C:\\pagecross64.exe', '2000', '0', 'noimm'], { files: { 'pagecross64.exe': exe64 }, timeout, name: 'native64' });
  t.check(`native64: ${want.source}`, want.test(n64.out), w.detail(n64));
  // On the page with the JIT's diagnostics, which say when the rewritten
  // page enters the hot-rewrite mode and when, quiet, it leaves it.
  const env = { GEX_JIT_DIAG: '1' };
  const hot = /page 0x[0-9a-f]+ is rewritten while it runs/, cooled = /page 0x[0-9a-f]+ quiet for \d+ ms after \d+ guard hits/;
  for (const [name, path, file] of [['page', '/t/pagecross.exe', exe], ['page64', '/t/pagecross64.exe', exe64]]) {
    const p = await w.page([path], { files: { [path]: file }, timeout, name, env });
    const all = `${p.out ?? ''}\n${p.err ?? ''}`;
    t.check(`${name}: ${want.source}`, want.test(p.out), w.detail(p));
    t.check(`${name}: the rewritten page enters the hot-rewrite mode and leaves it`, hot.test(all) && cooled.test(all), w.detail(p));
    t.say(`${name}: ${(all.match(/pagecross: imm ok \d+ \(\d+ ms\)/) ?? ['no imm line'])[0]}`);
    t.say(`${name}: ${(all.match(/pagecross: blocks ok .*/) ?? ['no blocks line'])[0]}`);
  }
  w.clean();
}
