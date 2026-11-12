// bubblewrap itself, Alpine's (not a stand-in), making a sandbox under gex
// natively: its `clone` with every CLONE_NEW* flag, its user namespace's
// maps, the tmpfs it builds a root in, its binds of descriptors opened
// O_PATH (`/proc/self/fd/N`), read-only remounts, both of its
// `pivot_root`s and the detached unmounts, `devpts`, its loopback set up by
// netlink, and its monitor's signalfd. The sandbox sees only what it was
// given, a read-only bind refuses writes, its tmpfs is its own (nothing it
// writes reaches the tree), and the command's exit status comes back out.
//
// The tree is GEX_TEST_ALPINE_BWRAP: an x86-64 Alpine root with the
// bubblewrap package installed (its /usr/bin/bwrap a real one, and
// busybox in /bin), which the test only reads.
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { native } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 300 };

export default async function (t) {
  const root = process.env.GEX_TEST_ALPINE_BWRAP;
  if (!root || !existsSync(join(root, 'usr/bin/bwrap'))) t.skip('GEX_TEST_ALPINE_BWRAP: an x86-64 Alpine root with bubblewrap installed');
  if (readFileSync(join(root, 'usr/bin/bwrap')).subarray(0, 4).toString('latin1') !== '\x7fELF') t.skip(`${root}/usr/bin/bwrap is not bubblewrap itself`);
  const n = native(t);
  const before = readdirSync(join(root, 'tmp')).sort().join(' ');
  const bwrap = (args, script) => n.gexc(['run', '--sysroot', root, join(root, 'usr/bin/bwrap'), ...args, '--', '/bin/sh', '-c', script], { timeout: 120000 });
  const given = ['--ro-bind', '/usr', '/usr', '--ro-bind', '/bin', '/bin', '--ro-bind', '/lib', '/lib', '--proc', '/proc', '--dev', '/dev', '--tmpfs', '/tmp', '--unshare-all'];
  const r = await bwrap(given, [
    'echo "root: $(ls / | tr "\\n" " ")"',
    'echo "dev: $(ls /dev | tr "\\n" " ")"',
    'echo made > /tmp/inside && echo "tmp: $(cat /tmp/inside)"',
    'touch /usr/written 2>/dev/null && echo "usr: written" || echo "usr: refused"',
    'test -e /etc && echo "etc: there" || echo "etc: not there"',
    'test -r /proc/self/status && echo "proc: there"',
    'exit 7',
  ].join('; '));
  t.say(r.stdout.trim());
  t.check('bubblewrap runs the command and gives back its exit status', r.code === 7, `exit ${r.code}: ${r.stderr.trim().split('\n').filter((l) => !l.startsWith('[gex]')).slice(-4).join(' / ')}`);
  t.check('the sandbox\'s root is what it was given', /^root: bin dev lib proc tmp usr $/m.test(r.stdout), r.stdout);
  t.check('and nothing else of the tree', /^etc: not there$/m.test(r.stdout), r.stdout);
  t.check('its /dev is bubblewrap\'s', /^dev: core fd full null ptmx pts random shm stderr stdin stdout tty urandom zero $/m.test(r.stdout), r.stdout);
  t.check('its /tmp is a tmpfs it can write', /^tmp: made$/m.test(r.stdout), r.stdout);
  t.check('a read-only bind refuses a write', /^usr: refused$/m.test(r.stdout), r.stdout);
  t.check('/proc is there when asked for', /^proc: there$/m.test(r.stdout), r.stdout);
  t.check('nothing it wrote reached the tree', !existsSync(join(root, 'usr/written')) && readdirSync(join(root, 'tmp')).sort().join(' ') === before && !existsSync(join(root, 'newroot')) && !existsSync(join(root, 'oldroot')));
  // As glycin runs its image loaders: no /proc asked for, a file bound, the
  // environment cleared. /bin is not given, so busybox's links are not there.
  const g = await n.gexc(['run', '--sysroot', root, join(root, 'usr/bin/bwrap'), '--unshare-all', '--die-with-parent', '--chdir', '/',
    '--ro-bind', '/usr', '/usr', '--ro-bind', '/lib', '/lib', '--dev', '/dev', '--tmpfs', '/tmp-home', '--clearenv', '--setenv', 'HOME', '/tmp-home',
    '--ro-bind-try', '/etc/passwd', '/etc/passwd', '--', '/lib/ld-musl-x86_64.so.1', '--list', '/usr/bin/bwrap'], { timeout: 120000 });
  t.check('a glycin-like sandbox runs a program from what it was given', g.code === 0 && /libcap\.so\.2/.test(g.stdout), `exit ${g.code}: ${g.stdout.trim().slice(-200)} / ${g.stderr.trim().split('\n').filter((l) => !l.startsWith('[gex]')).slice(-3).join(' / ')}`);
}
