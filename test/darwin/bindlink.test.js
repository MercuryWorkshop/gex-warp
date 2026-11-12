// A directory bound under a directory the root has as a symbolic link (a
// Mac root's /tmp -> private/tmp, fixtures/darwin/bindlink.c): there by
// either name, as a Linux bind mount is, which resolves its target when it
// is made -- the kernel places a mount under a link where the link leads
// (vfs `Namespace::mount_point`). Natively, both processors.
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 300 };

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built and checked on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const n = native(t);
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  const d = n.dir('bindlink');
  const prog = join(d, 'bindlink');
  const bound = join(d, 'bound');
  mkdirSync(bound, { recursive: true });
  try {
    execFileSync('xcrun', ['clang', '-arch', x86 ? 'x86_64' : 'arm64', '-O1', '-w', '-o', prog, join(ROOT, 'test/fixtures/darwin/bindlink.c')], { stdio: 'pipe' });
  } catch (e) {
    t.skip(`xcrun clang: ${String(e.stderr || e).trim().split('\n').pop()}`);
  }
  for (const at of ['/tmp/bound', '/private/tmp/bound']) {
    const got = await n.gexc(['run', '--sysroot', mac, '--bind', `${bound}=${at}`, '--bind', `${d}=/work`, prog, '/tmp/bound'], { timeout: 120000 });
    const lines = got.stdout.trim().split('\n');
    t.say(`bound at ${at}: ${lines.join(' / ')}`);
    for (const l of lines) {
      const [what, ...rest] = l.split(': ');
      const said = rest.join(': ');
      const ok = what === 'realpath' ? said === '/private/tmp/bound' : said === 'ok';
      t.check(`bound at ${at}, through /tmp: ${what}`, ok, said);
    }
    t.check(`bound at ${at}: every call said`, lines.length === 6, `${lines.length} lines`);
  }
}
