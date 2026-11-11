// What a program for Apple silicon uses without asking, since every such Mac
// has it: AES, the polynomial multiplies, SHA-1, SHA-256, SHA-512, SHA-3's
// four, fjcvtzs and the flag manipulations (crates/gex-isa: the decoder, and
// slow.rs for what the host carries out). fixtures/darwin/crypto.c, built
// against the SDK, on this Mac and under gexc in the Mac root pyrus builds
// (GEX_TEST_DARWIN_ROOT): the same lines. arm64's alone.
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { exec, native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64'], timeout: 600 };

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built and checked on an arm64 Mac');
  const mac = process.env.GEX_TEST_DARWIN_ROOT || join(ROOT, '..', 'pyrus', 'out', 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT: pyrus builds one)`);
  const n = native(t);
  const prog = join(n.dir('crypto'), 'crypto');
  try {
    execFileSync('xcrun', ['clang', '-arch', 'arm64', '-O1', '-Wall', '-Werror', '-o', prog, join(ROOT, 'test/fixtures/darwin/crypto.c')], { stdio: 'pipe' });
  } catch (e) {
    t.skip(`xcrun clang: ${String(e.stderr || e).trim().split('\n').pop()}`);
  }
  const want = await exec([prog], { timeout: 60000 });
  t.check('the Mac ran it', want.code === 0 && /all of it:/.test(want.stdout), `exit ${want.code}: ${want.stderr.trim()}`);
  const got = await n.gexc(['run', '--sysroot', mac, prog], { timeout: 200000 });
  const a = want.stdout.split('\n');
  const b = got.stdout.split('\n');
  const i = a.findIndex((l, k) => l !== b[k]);
  t.check(`under gexc as on the Mac (${a.length - 1} lines)`, i < 0 && a.length === b.length && got.code === 0,
    i < 0 ? `exit ${got.code}: ${got.stderr.trim().split('\n').slice(-3).join(' / ')}` : `line ${i + 1}: mac ${JSON.stringify(a[i])}, gex ${JSON.stringify(b[i] ?? null)}`);
}
