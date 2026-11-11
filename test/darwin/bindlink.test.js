// A bind under a directory the Mac root links elsewhere (`/tmp` is
// `private/tmp`): `--bind DIR=/tmp/bound` is there by both names, as a
// Linux bind mount is, its target resolved when it is made
// (hosts/native resolve_in_root). fixtures/darwin/bindlink.c (pyrus-appkit's
// gex-gaps/bind_symlink.c) opens, stats, creates and renames in it, and asks
// its real path. Both processors.
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 300 };

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built and checked on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  const n = native(t);
  const d = n.dir('bindlink');
  const prog = join(d, 'bindlink');
  execFileSync('xcrun', ['clang', '-arch', x86 ? 'x86_64' : 'arm64', '-O1', '-w', '-o', prog, join(ROOT, 'test/fixtures/darwin/bindlink.c')], { stdio: 'pipe' });
  const want = 'opendir /tmp/bound: ok\nstat: ok\nopen O_CREAT /tmp/bound/a.tmp: ok\nrename to /tmp/bound/b.txt: ok\nrealpath: /private/tmp/bound\naccess W_OK: ok\n';
  for (const at of ['/tmp/bound', '/private/tmp/bound']) {
    const dir = join(d, at.replaceAll('/', '_'));
    mkdirSync(dir, { recursive: true });
    const got = await n.gexc(['run', '--sysroot', mac, '--bind', `${dir}=${at}`, prog, '/tmp/bound'], { timeout: 120000 });
    t.check(`bound at ${at}, there at /tmp/bound`, got.stdout === want && existsSync(join(dir, 'b.txt')), got.stdout.trim().split('\n').join(' / '));
  }
}
