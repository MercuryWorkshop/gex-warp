// A directory's size in /tmp as entries come and go
// (fixtures/darwin/dirsize.c): APFS's 64 and 32 an entry, which gex's
// tmpfs gives a Mac program (crates/gex-kernel/src/vfs/tmpfs.rs,
// TmpFs::dir_size); a Linux one still sees 4096. Built against the SDK,
// run on this Mac and under gexc in the Mac root pyrus builds
// (GEX_TEST_DARWIN_ROOT, _X86): the same lines. Both processors.
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { exec, native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 900 };

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built and checked on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  const n = native(t);
  const prog = join(n.dir('dirsize'), 'dirsize');
  try {
    execFileSync('xcrun', ['clang', '-arch', x86 ? 'x86_64' : 'arm64', '-O1', '-Wall', '-Werror', '-o', prog, join(ROOT, 'test/fixtures/darwin/dirsize.c')], { stdio: 'pipe' });
  } catch (e) {
    t.skip(`xcrun clang: ${String(e.stderr || e).trim().split('\n').pop()}`);
  }
  const want = await exec(x86 ? ['arch', '-x86_64', prog] : [prog], { timeout: 200000 });
  t.check('the Mac ran it', want.code === 0, `exit ${want.code}: ${want.stderr.trim()}`);
  const got = await n.gexc(['run', '--sysroot', mac, prog], { timeout: 400000 });
  const a = want.stdout.split('\n');
  const b = got.stdout.split('\n');
  const i = a.findIndex((l, k) => l !== b[k]);
  t.check(`under gexc as on the Mac (${a.length - 1} lines)`, i < 0 && a.length === b.length && got.code === 0,
    i < 0 ? `exit ${got.code}: ${got.stderr.trim().split('\n').slice(-3).join(' / ')}` : `line ${i + 1}: mac ${JSON.stringify(a[i])}, gex ${JSON.stringify(b[i] ?? null)}`);
}
