// A Mac program's root, in a system that is Linux's
// (crates/gex-kernel/src/darwin/root.rs): one tree, Linux's, with a Mac's
// in a directory of it, and one program built for each system that says
// where it finds itself and runs the next. A Linux program runs the Mac
// one, which is in its root (its linker there, the system at
// /Volumes/Linux, the same working directory by its name there, the same
// /tmp); that runs the Linux one, which is back out; and that the Mac one
// again. Then a Mac program outside any root, given one by its environment.
//
// The page's runtime under Node only: a process there has a kernel of its
// own, where gexc runs one kind of program at a time. The Mac's tree is
// one pyrus built: GEX_TEST_DARWIN_ROOT (and GEX_TEST_DARWIN_ROOT_X86), by
// default ../pyrus/out/root (and root-x86_64) beside this repository.

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  const infix = x86 ? '.x86' : '';
  const seed = join(ROOT, `page/public/gex/seed${infix}.shared.wasm`);
  if (!existsSync(seed)) t.skip(`no ${seed} (hosts/browser/build.sh${x86 ? ', GEX_ARCH=x86' : ''})`);

  const n = native(t);
  const tree = n.dir('root');
  for (const d of ['bin', 'work', 'tmp', 'opt']) mkdirSync(join(tree, d), { recursive: true });
  // (A clone where the disk makes them: a root is some hundreds of files.)
  execFileSync('cp', ['-Rc', mac, join(tree, 'opt/mac')]);
  mkdirSync(join(tree, 'opt/mac/bin'), { recursive: true });
  const src = join(ROOT, 'test/fixtures/darwin/root/where.c');
  await n.cc(['-static', '-O1', '-o', join(tree, 'bin/where'), src]);
  const clang = ['clang', '-arch', x86 ? 'x86_64' : 'arm64', '-O1', src, '-o'];
  try {
    execFileSync('xcrun', [...clang, join(tree, 'opt/mac/bin/where')], { stdio: 'pipe' });
    execFileSync('xcrun', [...clang, join(tree, 'bin/macwhere')], { stdio: 'pipe' });
  } catch (e) {
    t.skip(`xcrun clang: ${String(e.stderr || e).trim().split('\n').pop()}`);
  }

  const run = async (args, env = []) => {
    const r = await n.g.run(seed, join(tree, 'bin/where'), args, {
      env: { GEX_SHARED: '1' },
      flags: ['--dir', `/=${tree}`, '--cwd', '/work', ...env.flatMap((e) => ['--env', e])],
      timeout: 240000,
    });
    return { ...r, lines: r.stdout.split('\n').filter((l) => /^(Linux|Darwin): /.test(l)) };
  };
  const at = (system, cwd, inside, tmp, here) =>
    `${system}: in ${cwd}; a Mac's linker ${inside}; /Volumes/Linux ${inside}; /dev/null yes; the note in /tmp: ${tmp}; the note here: ${here}`;

  // Linux, the Mac's, Linux again, the Mac's again: each where its own
  // system's tree is, in the one directory, reading the last one's notes.
  const chain = await run(['/opt/mac/bin/where', '/Volumes/Linux/bin/where', '/opt/mac/bin/where']);
  const want = [
    at('Linux', '/work', 'no', 'none', 'none'),
    at('Darwin', '/Volumes/Linux/work', 'yes', 'Linux', 'Linux'),
    at('Linux', '/work', 'no', 'Darwin', 'Darwin'),
    at('Darwin', '/Volumes/Linux/work', 'yes', 'Linux', 'Linux'),
    'Linux: its child ended 41',
    'Darwin: its child ended 0',
    'Linux: its child ended 0',
  ];
  t.check('a Linux program runs a Mac one in its root, which runs a Linux one out of it', chain.lines.join('\n') === want.join('\n') && chain.code === 0,
    `exit ${chain.code}: ${chain.lines.join(' / ') || chain.stderr.trim().split('\n').slice(-3).join(' / ')}`);

  // A Mac program that is in no root: the one its environment names.
  const named = await run(['/bin/macwhere'], ['GEX_DARWIN_ROOT=/opt/mac']);
  t.check('a Mac program outside any root runs in the one GEX_DARWIN_ROOT names', named.lines[1] === at('Darwin', '/Volumes/Linux/work', 'yes', 'Linux', 'Linux') && named.lines[2] === 'Linux: its child ended 41',
    named.lines.join(' / ') || named.stderr.trim().split('\n').slice(-3).join(' / '));
}
