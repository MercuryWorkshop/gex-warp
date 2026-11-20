// A Mac's __PAGEZERO under gexc: fixtures/darwin/pagezero.c in a Mac root,
// against what this Mac prints. A fixed mapping below 4 GiB fails (Mach's
// KERN_INVALID_ADDRESS, mmap's ENOMEM), a hint there is not where memory
// goes, and protecting, syncing, advising or unmapping an address there
// leaves the memory 4 GiB above it as it was: gex's window must not let an
// address below it name the window's memory under it (the dynamic linker
// took a library linked at 8 MiB to be there, at its own address, while it
// ran 4 GiB up, and could not tell its lazy binding which image it was).
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { exec, native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 300 };

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built and checked on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  const n = native(t);
  const d = n.dir('pagezero');
  const prog = join(d, 'pagezero');
  try {
    execFileSync('xcrun', ['clang', '-arch', x86 ? 'x86_64' : 'arm64', '-O1', '-Wall', '-Wextra', '-Werror', '-o', prog, join(ROOT, 'test/fixtures/darwin/pagezero.c')], { stdio: 'pipe' });
  } catch (e) {
    t.skip(`xcrun clang: ${String(e.stderr || e).trim().split('\n').pop()}`);
  }
  const want = await exec(x86 ? ['arch', '-x86_64', prog] : [prog], { timeout: 60000 });
  t.check('the Mac ran it', want.code === 0, `exit ${want.code}: ${want.stderr.trim()}`);
  const got = await n.gexc(['run', '--sysroot', mac, prog], { timeout: 200000 });
  t.check('gexc ran it', got.code === 0, `exit ${got.code}: ${got.stderr.trim().split('\n').slice(-3).join(' / ')}`);
  const w = want.stdout.trimEnd().split('\n');
  const g = got.stdout.trimEnd().split('\n');
  for (let i = 0; i < w.length; i++) {
    // (each answer with the call it answers, which is the line before an indented one)
    const what = w[i].startsWith('  ') && i > 0 ? `${w[i - 1].split(':')[0]}, ${w[i].trim()}` : w[i].split(':')[0];
    t.check(what, g[i] === w[i], `mac "${w[i]}", gex "${g[i] ?? '(nothing)'}"`);
  }
  t.check('nothing more', g.length === w.length, g.slice(w.length).join(' / '));
}
