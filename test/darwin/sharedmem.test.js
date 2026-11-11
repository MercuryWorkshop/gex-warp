// What a forked child shares with its parent (fixtures/darwin/sharedmem.c):
// anonymous MAP_SHARED memory, shm_open's, a file's, a memfd's (Linux),
// against private memory; seen at a pipe, after an exit and with no system
// call at all; atomics from both; a PTHREAD_PROCESS_SHARED mutex and an
// unnamed semaphore (Linux; a Mac has none) in shared memory.
//
// A Mac program, built against the SDK, on this Mac and under gexc in the
// Mac root pyrus builds (GEX_TEST_DARWIN_ROOT, _X86); a Linux one (musl,
// GEX_TOOLCHAIN, _X86) under gexc, against what Linux printed
// (fixtures/darwin/sharedmem.linux.txt: Alpine in Docker, both processors
// alike). Both processors. Natively the parent's pages are the child's
// (hosts/native/src/shmem.rs); what that does not reach is said and not
// checked: a child's own mapping of a shm_open name after the fork (a
// copy, as before), and on a Mac a process-shared mutex (Darwin's psynch
// queues are each process's).
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { exec, native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

/** Lines of `want` and `got` that differ, but for those `known` matches. */
function compare(t, what, want, got, known) {
  const a = want.split('\n');
  const b = got.split('\n');
  const off = [];
  const said = [];
  for (let k = 0; k < Math.max(a.length, b.length); k++) {
    if (a[k] === b[k]) continue;
    (known.test(a[k] ?? '') ? said : off).push(`line ${k + 1}: want ${JSON.stringify(a[k] ?? null)}, got ${JSON.stringify(b[k] ?? null)}`);
  }
  for (const l of said) t.say(`${what}, not checked: ${l}`);
  t.check(`${what}: as it should be (${a.length - 1} lines)`, off.length === 0, off.slice(0, 3).join(' / '));
}

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built and checked on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const n = native(t);
  const d = n.dir('sharedmem');
  const src = join(ROOT, 'test/fixtures/darwin/sharedmem.c');

  const lin = join(d, 'sharedmem.linux');
  await n.cc(['-O1', '-static', '-Wl,-Ttext-segment=0x1000000', '-o', lin, src, '-lpthread']);
  const want = readFileSync(join(ROOT, 'test/fixtures/darwin/sharedmem.linux.txt'), 'utf8');
  const gotL = await n.gexc(['run', lin], { timeout: 200000 });
  compare(t, 'Linux', want, gotL.stdout, /a child's own mapping of the name/);

  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  const prog = join(d, 'sharedmem');
  try {
    execFileSync('xcrun', ['clang', '-arch', x86 ? 'x86_64' : 'arm64', '-O1', '-Wall', '-Wno-deprecated-declarations', '-o', prog, src], { stdio: 'pipe' });
  } catch (e) {
    t.skip(`xcrun clang: ${String(e.stderr || e).trim().split('\n').pop()}`);
  }
  const wantM = await exec(x86 ? ['arch', '-x86_64', prog] : [prog], { timeout: 120000 });
  t.check('the Mac ran it', wantM.code === 0, `exit ${wantM.code}: ${wantM.stderr.trim()}`);
  const gotM = await n.gexc(['run', '--sysroot', mac, prog], { timeout: 200000 });
  compare(t, 'Mac', wantM.stdout, gotM.stdout, /a child's own mapping of the name|^mutex: the child/);
}
