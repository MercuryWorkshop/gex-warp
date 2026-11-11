// Stores (and a load) that should fault (fixtures/darwin/storefault.c):
// into a page unmapped, PROT_NONE, read-only; near null; a write barrier
// that unprotects in its handler; a stack overflow caught on an alternate
// stack. Said, not checked: gex keeps no page's protection against an
// access (one linear memory; only an address past its end traps), so none
// of them faults today, on either host or processor. Against this Mac and
// against what Linux printed (fixtures/darwin/storefault.linux-*.txt,
// Alpine in Docker). And what a check before every store would cost
// (fixtures/darwin/storecheck.c: the check written into the program, a
// byte per page looked at before each store), said. The harness for
// whoever makes them fault; see the note in the commit that added it.
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { exec, native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

function say(t, what, want, got) {
  const a = want.trim().split('\n');
  const b = got.trim().split('\n');
  const off = a.filter((l, k) => l !== b[k]);
  t.say(off.length ? `${what}: ${off.length} of ${a.length} differ (want ${off.slice(0, 2).join(' / ')})` : `${what}: as it should be`);
}

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built and checked on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const n = native(t);
  const d = n.dir('storefault');
  const lin = join(d, 'storefault.linux');
  await n.cc(['-O1', '-static', '-Wl,-Ttext-segment=0x1000000', '-o', lin, join(ROOT, 'test/fixtures/darwin/storefault.c')]);
  const want = readFileSync(join(ROOT, `test/fixtures/darwin/storefault.linux-${x86 ? 'x86' : 'a64'}.txt`), 'utf8');
  const got = await n.gexc(['run', lin], { timeout: 200000 });
  t.check('Linux: it ran', got.code === 0, `exit ${got.code}`);
  say(t, 'Linux', want, got.stdout);
  const bench = join(d, 'storecheck.linux');
  await n.cc(['-O2', '-static', '-Wl,-Ttext-segment=0x1000000', '-o', bench, join(ROOT, 'test/fixtures/darwin/storecheck.c')]);
  const cost = await n.gexc(['run', bench, '3'], { timeout: 200000 });
  for (const l of cost.stdout.trim().split('\n')) t.say(`a check before each store: ${l}`);

  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  const prog = join(d, 'storefault');
  try {
    execFileSync('xcrun', ['clang', '-arch', x86 ? 'x86_64' : 'arm64', '-O1', '-w', '-o', prog, join(ROOT, 'test/fixtures/darwin/storefault.c')], { stdio: 'pipe' });
  } catch (e) {
    t.skip(`xcrun clang: ${String(e.stderr || e).trim().split('\n').pop()}`);
  }
  const wantM = await exec(x86 ? ['arch', '-x86_64', prog] : [prog], { timeout: 120000 });
  t.check('the Mac ran it', wantM.code === 0, `exit ${wantM.code}: ${wantM.stderr.trim()}`);
  const gotM = await n.gexc(['run', '--sysroot', mac, prog], { timeout: 200000 });
  say(t, 'Mac', wantM.stdout, gotM.stdout);
}
