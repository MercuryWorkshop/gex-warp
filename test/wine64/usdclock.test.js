// KUSER_SHARED_DATA's clocks under 64-bit Wine (usdclock.c), with the door but on
// the single-thread build: a process reads SystemTime, InterruptTime and
// TickCount advancing at the rate of the elapsed time and never half written,
// from the host's one clock (gex.usd_tick). Natively, on the page, on the page
// with the native PE core (a compact-space process) and on the single-thread
// build's page; on each page sync's too, whose threads end before its
// waitable timer's check reads GetTickCount64 across a 50 ms wait.
import { CONSOLE } from './cases.mjs';
import { wine64 } from '../lib/winerun.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], images: ['win-progs'], timeout: 3600 };

const allOk = /(ok [^\n]*\n)+0 failures/;
const SYNC = CONSOLE.find((c) => c.name === 'sync');

export default async function (t) {
  const legs = [[wine64(t, { door: 1 }), ['', undefined], [' page', {}]], [wine64(t, { door: 0 }), [' page sp', { single: true }]]];
  if (process.env.GEX_TEST_WINE64_NATIVE) legs.push([wine64(t, { door: 1, native: true }), [' page compact', {}]]);
  else t.say('usdclock page compact: not run (GEX_TEST_WINE64_NATIVE)');
  for (const [w, ...runs] of legs) {
    for (const [leg, page] of runs) {
      const r = await w.nt({ name: `usdclock${leg}`, want: allOk, exe: 'usdclock.exe', args: ['3'], timeout: 600000, page });
      // the measured rates, in the log
      for (const l of (r?.stdout ?? '').split('\n').filter((x) => /^(ok|FAIL) usdclock/.test(x))) t.say(`usdclock${leg}: ${l}`);
      if (page) await w.nt({ ...SYNC, name: `sync${leg}`, timeout: 600000, page });
    }
    w.clean();
  }
}
