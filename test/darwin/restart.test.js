// Restartable ranges (crates/gex-kernel/src/darwin/restart.rs), which the
// Objective-C runtime frees its method caches by: fixtures/darwin/restart.c
// registers a range a thread turns in and asks that its threads be
// restarted, under gexc and on the page's runtime under Node.
//
// The Mac's own answers are not to be had (its libobjc has registered the
// task's ranges before a program's code runs, and a task registers once),
// so what the program must print is here, from XNU's source.
import { darwin } from '../lib/darwin.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 300 };

const WANT = `ask before there are any ranges = 0
a recovery address too far from its range = 4
a range with flags = 4
two ranges that overlap = 4
register the range = 0
register again = 46
ok a thread turns in the range
ask = 0
the thread left the range with 7
ok soon
ask with one thread at work and one asleep = 0
ok answered before the sleeper wakes
and again = 0
ask again = 0
the thread left the range with 7
a thread nobody asked about left it with 0
ask with no other thread = 0
`;

export default async function (t) {
  const d = darwin(t);
  const prog = await d.build('restart');
  const leg = (name, got) => t.check(`restart: ${name}`, got.stdout === WANT && got.code === 0,
    `exit ${got.code}: ${got.stdout.trim().split('\n').filter((l, i) => l !== WANT.split('\n')[i]).slice(0, 3).join(' / ') || got.stderr.trim().split('\n').slice(-3).join(' / ')}`);
  leg('under gexc', await d.gex(prog));
  const on = await d.page(prog);
  if (on) leg('on the page runtime', on);
  else t.say('restart: no page leg (page/public/gex has no shared shim for this arch: hosts/browser/build.sh, GEX_ARCH=x86 for x86)');
}
