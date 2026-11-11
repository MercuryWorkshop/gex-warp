// The page System's `macos` (page/runtime/src/system.js): which macOS the
// programs a page starts are told they are on, as a Wine prefix's Windows
// version is set for all its programs and one program may have its own.
// fixtures/darwin/osversion.c, static, in a tree in memory that has this
// Mac's SystemVersion.plist and the table of releases a pyrus root has: a
// system made with `macos: 'sonoma'` starts it told sonoma, and a program
// whose own environment names a release is told that one.
//
// The table is the Mac root's (GEX_TEST_DARWIN_ROOT, or _X86; by default
// ../pyrus/out/root beside this repository): a root without one is a skip.
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { darwin } from '../lib/darwin.mjs';
import { ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 300 };

export default async function (t) {
  const { eq, timeout } = t;
  const d = darwin(t);
  const x86 = t.arch === 'x86';
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  const table = join(mac, 'usr/share/gex/macos-releases');
  if (!existsSync(table)) t.skip(`no table of releases at ${table} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus's build.sh puts one in its roots)`);
  const prog = await d.build('osversion');
  const { arch: _, ...debug } = t.debug();
  const sys = new System({ baseUrl: t.baseUrl, arch: x86 ? 'x86' : 'aarch64', macos: 'sonoma', debug });
  const fs = new MemoryFilesystem(sys);
  fs.addFiles({
    '/bin/osversion': readFileSync(prog.gex),
    '/usr/share/gex/macos-releases': readFileSync(table),
    '/System/Library/CoreServices/SystemVersion.plist': readFileSync('/System/Library/CoreServices/SystemVersion.plist'),
  }, { mode: 0o755 });
  await timeout(sys.boot(), 60000, 'boot');
  // What each says of itself, as far as the version goes.
  const said = (out) => out.split('\n').filter((l) => /^first: (told|kern\.os(release|version|productversion(compat)?):)/.test(l)).join('\n');
  const a = await timeout(sys.run(['/bin/osversion'], { env: { T_VAR: 'yes' }, cwd: '/' }), 120000, 'the system\'s release');
  eq('a program the page starts is told the system\'s release', [said(a.stdout), a.code], [[
    'first: told sonoma',
    'first: kern.osrelease: 23.6.0',
    'first: kern.osversion: 23J631',
    'first: kern.osproductversion: 14.8.9',
    'first: kern.osproductversioncompat: 10.16',
  ].join('\n'), 0]);
  const b = await timeout(sys.run(['/bin/osversion'], { env: { T_VAR: 'yes', GEX_MACOS_VERSION: '27.0' }, cwd: '/' }), 120000, 'its own release');
  eq('and one whose environment names a release, that one', [said(b.stdout), b.code], [[
    'first: told 27.0',
    'first: kern.osrelease: 27.0.0',
    'first: kern.osversion: 26A425',
    'first: kern.osproductversion: 27.0',
    'first: kern.osproductversioncompat: ',
  ].join('\n'), 0]);
  await sys.shutdown();
}
