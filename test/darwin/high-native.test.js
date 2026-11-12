// darwin/high natively: fixtures/darwin/high.c under gexc in a Mac root,
// every kind of address it can be given at or above 4 GiB, against what
// this Mac prints (fixtures/darwin/high.mjs). (A program linked to run
// below 4 GiB with a small zero page, which a Mac runs where it is linked,
// gets no window: darwin/arches' static programs are such.)
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { exec, native, ROOT } from '../lib/native.mjs';
import { compare } from '../fixtures/darwin/high.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built and checked on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  const n = native(t);
  const d = n.dir('high');
  const prog = join(d, 'high');
  try {
    execFileSync('xcrun', ['clang', '-arch', x86 ? 'x86_64' : 'arm64', '-O1', '-w', '-o', prog, join(ROOT, 'test/fixtures/darwin/high.c')], { stdio: 'pipe' });
  } catch (e) {
    t.skip(`xcrun clang: ${String(e.stderr || e).trim().split('\n').pop()}`);
  }
  const want = await exec(x86 ? ['arch', '-x86_64', prog] : [prog], { timeout: 120000 });
  t.check('the Mac ran it', want.code === 0, `exit ${want.code}: ${want.stderr.trim()}`);
  const got = await n.gexc(['run', '--sysroot', mac, prog], { timeout: 200000 });
  if (got.code !== 0) t.say(got.stderr.trim().split('\n').slice(-3).join(' / '));
  compare(t, 'gexc', want.stdout, got.stdout, got.code);
}
