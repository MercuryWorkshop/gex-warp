// Large functions (fixtures/darwin/large.mjs): thousands of basic blocks
// with 64 values live (at -O0 one function of more than a megabyte, past an
// arm64 conditional branch's reach), frames of 70 KB to 3 MB probed page by
// page on the main thread and a thread's, a 2.5 MB alloca, switches of
// thousands of cases, a call of 48 arguments and a variadic one of 30, a
// computed goto. Built against the SDK at -O0 and -O2, run on this Mac and
// under gexc in the Mac root pyrus builds (GEX_TEST_DARWIN_ROOT, _X86): the
// same lines. Both processors.
//
// (Looked for after pyrus-gui's gex-gaps/large_main_crash.m, which was no
// translation's fault: pyrus's CFNumberCompare reads a compiler-made
// NSNumber as a CFNumber when either is a float, and indexes a table of
// seven with the low three bits of the number's encoding string's address,
// so whether it crashed, answered wrongly or passed went with where the
// linker put the string.)
import { execFileSync } from 'node:child_process';
import { existsSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { exec, native, ROOT } from '../lib/native.mjs';
import { largeProgram } from '../fixtures/darwin/large.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 900 };

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built and checked on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  const n = native(t);
  const d = n.dir('large');
  for (const [opt, blocks] of [['-O0', 30000], ['-O2', 4000]]) {
    const src = join(d, `large${opt}.c`);
    const prog = join(d, `large${opt}`);
    writeFileSync(src, largeProgram({ blocks }));
    try {
      execFileSync('xcrun', ['clang', '-arch', x86 ? 'x86_64' : 'arm64', opt, '-w', '-o', prog, src], { stdio: 'pipe' });
    } catch (e) {
      t.skip(`xcrun clang: ${String(e.stderr || e).trim().split('\n').pop()}`);
    }
    const want = await exec(x86 ? ['arch', '-x86_64', prog] : [prog], { timeout: 120000 });
    t.check(`${opt}: the Mac ran it`, want.code === 0, `exit ${want.code}: ${want.stderr.trim()}`);
    const got = await n.gexc(['run', '--sysroot', mac, prog], { timeout: 300000 });
    const a = want.stdout.split('\n');
    const b = got.stdout.split('\n');
    const i = a.findIndex((l, k) => l !== b[k]);
    t.check(`${opt} (${blocks} blocks): under gexc as on the Mac (${a.length - 1} lines)`, i < 0 && a.length === b.length && got.code === 0,
      i < 0 ? `exit ${got.code}: ${got.stderr.trim().split('\n').slice(-3).join(' / ')}` : `line ${i + 1}: mac ${JSON.stringify(a[i])}, gex ${JSON.stringify(b[i] ?? null)}`);
  }
}
