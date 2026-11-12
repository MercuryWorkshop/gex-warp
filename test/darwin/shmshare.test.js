// POSIX shared memory between Mac processes that each open it by name
// (fixtures/darwin/shmshare.c): a forked child that maps the object after
// the fork, and a process posix_spawn starts. Each sees the other's stores
// while both run, as on a Mac (IOSurface's cross-process surfaces depend on
// it): natively a later mapping of a file another process maps shared is
// made that process's pages (MapIo::join_shared_file). The same lines on
// this Mac, under gexc in a Mac root, and for a Linux build of it (musl,
// GEX_TOOLCHAIN) under gexc -- what Linux prints too, both being these
// two lines of yes -- on both processors.
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { exec, native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 300 };

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built and checked on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const n = native(t);
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  const d = n.dir('shmshare');
  const src = join(ROOT, 'test/fixtures/darwin/shmshare.c');
  const lines = ['forked', 'spawned'].map((w) => `${w}: the other's store seen while it runs: yes; it read ours and saw ours seen: yes`);
  const lin = join(d, 'shmshare.linux');
  await n.cc(['-O1', '-static', '-o', lin, src]);
  const gotL = await n.gexc(['run', lin], { timeout: 120000 });
  const l = gotL.stdout.trim().split('\n');
  for (let k = 0; k < lines.length; k++) t.check(`Linux: ${lines[k]}`, l[k] === lines[k], `got ${JSON.stringify(l[k] ?? null)}`);

  const prog = join(d, 'shmshare');
  try {
    execFileSync('xcrun', ['clang', '-arch', x86 ? 'x86_64' : 'arm64', '-O1', '-Wall', '-o', prog, src], { stdio: 'pipe' });
  } catch (e) {
    t.skip(`xcrun clang: ${String(e.stderr || e).trim().split('\n').pop()}`);
  }
  const want = await exec(x86 ? ['arch', '-x86_64', prog] : [prog], { timeout: 60000 });
  t.check('the Mac ran it', want.code === 0, `exit ${want.code}: ${want.stderr.trim()}`);
  // (It spawns itself by its own name: one the Mac root has, through a bind.)
  const got = await n.gexc(['run', '--sysroot', mac, '--bind', `${d}=/work`, '--cwd', '/work', '--argv0', '/work/shmshare', prog], { timeout: 120000 });
  t.say(got.stdout.trim());
  const a = want.stdout.trim().split('\n');
  const b = got.stdout.trim().split('\n');
  for (let k = 0; k < a.length; k++) t.check(`as on the Mac: ${a[k]}`, a[k] === b[k], `got ${JSON.stringify(b[k] ?? null)}`);
}
