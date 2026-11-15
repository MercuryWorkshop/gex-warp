// IOSurface through its public API (fixtures/darwin/iosurface.c), against
// the Mac's: a surface made, its shape, locked, written and read back, its
// seed, a value set and copied, its use count, looked up by id and by a
// Mach port; and looked up from another process, which writes into it for
// the first to see.
//
// The program is linked against the Mac's SDK for macOS 10.14, and run on
// the Mac (Rosetta: it is x86-64) for the reference; under gexc it runs in
// the extracted Mojave root at GEX_TEST_MOJAVE_ROOT on Mojave's own
// IOSurface.framework, whose kernel half is gex's IOSurfaceRoot
// (crates/gex-kernel/src/darwin/iokit/surface.rs): each process maps the
// surface's memory, the kernel's shared memory, and sees the other's
// stores. Skipped without that root (nothing of Apple's is in this tree).
import { existsSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { exec, native } from '../lib/native.mjs';
import { PROGRAMS } from '../lib/darwin.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 600 };

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('Darwin programs are built and checked on an arm64 Mac');
  const root = process.env.GEX_TEST_MOJAVE_ROOT;
  if (!root) t.skip('GEX_TEST_MOJAVE_ROOT is not set (an extracted macOS 10.14 root, x86-64)');
  if (!existsSync(join(root, 'System/Library/Frameworks/IOSurface.framework'))) t.skip(`${root} has no IOSurface.framework`);
  const n = native(t);
  const dir = n.dir('iosurface');
  const bin = join(dir, 'bin');
  mkdirSync(bin, { recursive: true });
  const prog = join(bin, 'iosurface');
  const cc = await exec(['xcrun', 'clang', '-arch', 'x86_64', '-mmacosx-version-min=10.14', '-O1', '-Wall', '-Wextra', '-Werror', '-o', prog, join(PROGRAMS, 'iosurface.c'), '-framework', 'IOSurface', '-framework', 'CoreFoundation']);
  if (cc.code === 127) t.skip('no xcrun clang (the Xcode command line tools)');
  if (cc.code !== 0) throw new Error(`clang: ${cc.stderr.trim().split('\n').slice(-6).join(' / ')}`);

  const want = await exec([prog], { cwd: dir, timeout: 60000 });
  t.check('iosurface: nothing failed on the Mac', want.code === 0 && !/^FAIL /m.test(want.stdout), want.stdout.split('\n').filter((l) => l.startsWith('FAIL ')).join(' / '));
  // (The program is in the root as /gex-iosurface, where it starts itself
  // again for the other process.)
  const got = await n.gexc(['run', '--sysroot', root, '--bind', `${bin}=/gex-iosurface`, join(root, 'bin/sh'), '-c', '/gex-iosurface/iosurface'], { cwd: dir, timeout: 300000 });
  const a = want.stdout.split('\n');
  const b = got.stdout.split('\n');
  const at = a.findIndex((l, i) => l !== b[i]);
  const why = at < 0 && a.length === b.length ? '' : `line ${at + 1}: mac ${JSON.stringify(a[at] ?? null)}, gex ${JSON.stringify(b[at] ?? null)}`;
  t.check(`iosurface: as on the Mac (${a.length - 1} lines, exit ${want.code})`, want.stdout === got.stdout && want.code === got.code,
    `${want.code !== got.code ? `exit status: mac ${want.code}, gex ${got.code}; ` : ''}${why}${got.stderr.trim() ? ` / gex said: ${got.stderr.trim().split('\n').slice(-4).join(' / ')}` : ''}`);
  n.clean();
}
