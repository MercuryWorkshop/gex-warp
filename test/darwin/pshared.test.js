// Process-shared locks between Mac processes (fixtures/darwin/pshared.c):
// a PTHREAD_PROCESS_SHARED mutex contended from two processes, a condition
// variable broadcast to another process, a read-write lock held across a
// fork, and a named semaphore (sem_open) between a parent, its forked child
// and a process it spawns that opens the name itself. The program prints
// the same lines on this Mac and under gexc in a Mac root (pyrus's,
// GEX_TEST_DARWIN_ROOT, _X86), natively, both processors: the queues of
// psynch objects in shared memory are the system's
// (crates/gex-kernel/src/darwin/psynch.rs), and named semaphores are a table
// of the system's (darwin/psem.rs).
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { exec, native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built and checked on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const n = native(t);
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  const d = n.dir('pshared');
  const prog = join(d, 'pshared');
  try {
    execFileSync('xcrun', ['clang', '-arch', x86 ? 'x86_64' : 'arm64', '-O1', '-Wall', '-Wno-deprecated-declarations', '-o', prog, join(ROOT, 'test/fixtures/darwin/pshared.c')], { stdio: 'pipe' });
  } catch (e) {
    t.skip(`xcrun clang: ${String(e.stderr || e).trim().split('\n').pop()}`);
  }
  const want = await exec(x86 ? ['arch', '-x86_64', prog] : [prog], { timeout: 120000 });
  t.check('the Mac ran it', want.code === 0, `exit ${want.code}: ${want.stderr.trim()}`);
  // (It spawns itself by its own name: one the Mac root has, through a bind.)
  const got = await n.gexc(['run', '--sysroot', mac, '--bind', `${d}=/work`, '--cwd', '/work', '--argv0', '/work/pshared', prog], { timeout: 300000 });
  t.say(got.stdout.trim());
  const a = want.stdout.trim().split('\n');
  const b = got.stdout.trim().split('\n');
  for (let k = 0; k < a.length; k++) t.check(`as on the Mac: ${a[k]}`, a[k] === b[k], `got ${JSON.stringify(b[k] ?? null)}`);
  t.check('and nothing more', b.length === a.length, b.slice(a.length).join(' / '));
}
