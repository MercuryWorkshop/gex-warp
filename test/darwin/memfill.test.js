// Filling and copying memory as a Mac program does (fixtures/darwin/memfill.c):
// libSystem's memset, bzero, memmove and memcpy, which gex serves with the
// host's builtins where libplatform defines them (gex_xlate::intrinsic::
// recognise_macho), and `dc zva` (arm64) or `rep stosb`/`rep movsb` (x86-64),
// at the edges -- unaligned, empty, across a page boundary (and into a page
// that is not there or not writable, said and not checked). Built once against the SDK, run on this Mac
// and under gexc in the Mac root pyrus builds (GEX_TEST_DARWIN_ROOT, _X86),
// whose output must be the Mac's. Then what each costs, said and not checked.
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { exec, native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built and checked on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  const n = native(t);
  const prog = join(n.dir('memfill'), 'memfill');
  try {
    execFileSync('xcrun', ['clang', '-arch', x86 ? 'x86_64' : 'arm64', '-O1', '-Wall', '-Wextra', '-Werror', '-o', prog, join(ROOT, 'test/fixtures/darwin/memfill.c')], { stdio: 'pipe' });
  } catch (e) {
    t.skip(`xcrun clang: ${String(e.stderr || e).trim().split('\n').pop()}`);
  }
  const want = await exec([prog], { timeout: 60000 });
  t.check('nothing failed on the Mac', want.code === 0, `exit ${want.code}: ${want.stderr.trim()}`);
  const got = await n.gexc(['run', '--sysroot', mac, prog], { timeout: 240000 });
  const a = want.stdout.split('\n');
  const b = got.stdout.split('\n');
  const i = a.findIndex((l, k) => l !== b[k]);
  t.check(`under gexc as on the Mac (${a.length - 1} lines)`, i < 0 && a.length === b.length && got.code === 0,
    i < 0 ? `exit ${got.code}: ${got.stderr.trim().split('\n').slice(-3).join(' / ')}` : `line ${i + 1}: mac ${JSON.stringify(a[i])}, gex ${JSON.stringify(b[i] ?? null)}`);
  // Into pages that are not there or not writable: said, not checked. gex
  // keeps no page's protection against a store (one linear memory: only an
  // address past its end faults), so these run on where a Mac's die.
  const wantF = await exec([prog, 'faults'], { timeout: 60000 });
  const gotF = await n.gexc(['run', '--sysroot', mac, prog, 'faults'], { timeout: 240000 });
  const fa = wantF.stdout.split('\n');
  const fb = gotF.stdout.split('\n');
  const off = fa.filter((l, k) => l !== fb[k]);
  t.say(off.length ? `faults, not checked: ${off.length} of ${fa.length - 1} lines differ (the Mac's: ${off.slice(0, 3).join(' / ')})` : 'faults: as on the Mac');
  const cost = await n.gexc(['run', '--sysroot', mac, prog, 'time'], { timeout: 240000 });
  for (const l of cost.stdout.trim().split('\n')) t.say(`gexc: ${l}`);
}
