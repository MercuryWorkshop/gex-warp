// Mojave's own userland under gexc: macOS 10.14's dyld, libSystem,
// libdispatch (on the kernel's work queue, its events and workloops) and
// daemons, from an extracted x86-64 root at GEX_TEST_MOJAVE_ROOT (skipped
// without one: nothing of Apple's is in this tree).
//
// sw_vers, by itself; then, under gexlaunchd (guest/launchd: the bootstrap
// server in place of launchd, as root, which libxpc insists on of a
// bootstrap server that is not process 1), a user's defaults written and
// read back through Mojave's cfprefsd (its agent and daemon started on
// demand, checking in for their Mach services), and a notification posted
// to a waiter through its notifyd. The user is 501 with a home of the
// test's own (the root is not written to: a passwd with that user, and the
// home, are bound over it).
import { mkdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { exec, native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 900 };

export default async function (t) {
  const root = process.env.GEX_TEST_MOJAVE_ROOT;
  if (!root) t.skip('GEX_TEST_MOJAVE_ROOT is not set (an extracted macOS 10.14 root, x86-64)');
  if (!existsSync(join(root, 'usr/lib/dyld'))) t.skip(`${root} has no usr/lib/dyld`);
  const n = native(t);
  const d = n.dir('mojave');

  const sw = await n.gexc(['run', '--sysroot', root, join(root, 'usr/bin/sw_vers')], { timeout: 120000 });
  t.check('sw_vers: completes', sw.code === 0, `exit ${sw.code}: ${sw.stderr.trim().split('\n').slice(-2).join(' / ')}`);
  t.check('sw_vers: says 10.14', /^ProductVersion:\s+10\.14/m.test(sw.stdout), sw.stdout.trim());

  const gl = join(d, 'gl');
  const b = await exec(['sh', join(ROOT, 'guest/launchd/build.sh'), gl, 'x86_64']);
  if (b.code !== 0) throw new Error(`guest/launchd/build.sh: ${b.stderr.trim()}`);
  const home = join(d, 'home');
  mkdirSync(join(home, 'Library/Preferences'), { recursive: true });
  const etc = join(d, 'etc');
  mkdirSync(etc, { recursive: true });
  writeFileSync(join(etc, 'passwd'), readFileSync(join(root, 'etc/passwd'), 'utf8') + 'gex:*:501:20:gex:/Users/gex:/bin/sh\n');
  writeFileSync(join(etc, 'master.passwd'), readFileSync(join(root, 'etc/master.passwd'), 'utf8') + 'gex:*:501:20::0:0:gex:/Users/gex:/bin/sh\n');
  const session = (script) =>
    n.gexc(
      ['run', '--uid', '0', '--sysroot', root, '--bind', `${gl}=/gex`, '--bind', `${home}=/Users/gex`,
        '--bind', `${etc}/passwd=/etc/passwd`, '--bind', `${etc}/master.passwd=/etc/master.passwd`,
        '--env', 'HOME=/Users/gex', '--env', 'USER=gex',
        join(gl, 'gexlaunchd.x86_64'), '-v', '-u', '501', '--', '/bin/sh', '-c', script],
      { timeout: 300000 },
    );

  const prefs = await session('defaults write com.gex.test Greeting hello && defaults write com.gex.test Number -int 42 && defaults read com.gex.test Number && defaults read com.gex.test Greeting');
  t.check('defaults: written and read back', prefs.code === 0 && prefs.stdout.trim() === '42\nhello', `exit ${prefs.code}, said ${JSON.stringify(prefs.stdout.trim())}`);
  t.check('defaults: through cfprefsd', /check-in com\.apple\.cfprefsd\.agent/.test(prefs.stderr), 'cfprefsd\'s agent never checked in');
  const plist = join(home, 'Library/Preferences/com.gex.test.plist');
  t.check('defaults: the plist is in the home', existsSync(plist));

  const notes = await session('notifyutil -1 com.gex.ping & sleep 3; notifyutil -p com.gex.ping; wait');
  t.check('notifyutil: a post reaches a waiter through notifyd', notes.code === 0 && notes.stdout.trim() === 'com.gex.ping', `exit ${notes.code}, said ${JSON.stringify(notes.stdout.trim())}`);
  n.clean();
}
