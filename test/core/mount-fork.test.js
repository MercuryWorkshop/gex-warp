// The mounts a process made are its children's, across a fork and an exec
// (test/fixtures/programs/mount-fork.c): the system's proc mounted and /dev
// bound into a directory whose own /dev has a stray regular file for
// /dev/null, a tmpfs, a bind of the program's directory; then a child that
// chroots there reads /dev/urandom and /proc/self/status, as does a child of
// its own and the program it executes. On the page each process has a
// kernel of its own, so this is what the fork and the exec tell the child.
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';

export const meta = { hosts: ['browser', 'node'], images: ['native-progs'], timeout: 180 };

const inside = (who) => `${who}: /dev/null is a char device
${who}: /dev/zero is a char device
${who}: /dev/full is a char device
${who}: /dev/random is a char device
${who}: /dev/urandom is a char device
${who}: /dev/tty is a char device
${who}: /dev/ptmx is a char device
${who}: /dev/fd -> /proc/self/fd
${who}: urandom read 16 bytes, not all zero: yes
${who}: zero read 16 bytes, all zero: yes
${who}: full write: No space left on device
${who}: /dev/pts: opens
${who}: /proc/self/status: Name:\tmount-fork
${who}: mounts: proc 1, devtmpfs 1, tmpfs 1`;

const WANT = `before: /mnt/jail/dev/null is a regular file
mount proc: 0 ok
bind /dev: 0 ok
bind /g: 0 ok
mount tmpfs: 0 ok
${inside('child')}
${inside('grandchild')}
grandchild ended 0
${inside('exec')}
child ended 0`;

export default async function (t) {
  const { eq, timeout } = t;
  const sys = new System({ baseUrl: t.baseUrl, debug: t.debug() });
  const fs = new MemoryFilesystem(sys);
  fs.addFiles({ '/g/mount-fork': new URL(t.work(`mount-fork.${t.arch === 'x86' ? 'x86' : 'a64'}`)) }, { mode: 0o755 });
  await timeout(sys.boot(), 60000, 'boot');
  const r = await timeout(sys.run(['/g/mount-fork'], { env: { PATH: '/bin', HOME: '/' } }), 90000, 'mount-fork');
  eq('a child, its child and its exec have the mounts', r.stdout.trim(), WANT, r.stderr.slice(-2000));
  await sys.shutdown();
}
