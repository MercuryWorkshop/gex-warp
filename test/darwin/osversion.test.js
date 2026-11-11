// Which macOS a Mac program is told it is on, the kernel's side
// (crates/gex-kernel/src/darwin/info.rs): fixtures/darwin/osversion.c, a
// static program that asks with raw system calls (kern.osrelease,
// kern.osversion, kern.osproductversion and the rest) and reads the files
// that say it, then becomes itself again with an environment of its own.
//
// In a root that says it is this Mac's release (its own SystemVersion.plist
// and iOSSystemVersion.plist, and the table of releases a pyrus root has,
// /usr/share/gex/macos-releases), what it prints is what this Mac prints.
// Told another release (GEX_MACOS_VERSION, as `gexc run --macos` and the
// page System's `macos` set it), it is told that one's facts from the
// table, and given that one's files, which for 26.5 are its SDK's own
// SystemVersion.plist byte for byte (when this Mac has the 26.5 SDK); and
// the program it becomes, told nothing, is told the root's again. Under
// gexc and on the page's runtime under Node, both processors (an x86-64
// program on 27 is under Rosetta, as on this Mac; on 14 and 26 on an Intel
// Mac).
//
// The table is the Mac root's: GEX_TEST_DARWIN_ROOT (or _X86), by default
// ../pyrus/out/root beside this repository; a root built before it had one
// is a skip.
import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { darwin } from '../lib/darwin.mjs';
import { ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

const CORE = '/System/Library/CoreServices';
const SDK26 = '/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk/System/Library/CoreServices/SystemVersion.plist';

export default async function (t) {
  const d = darwin(t);
  const x86 = t.arch === 'x86';
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  const table = join(mac, 'usr/share/gex/macos-releases');
  if (!existsSync(table)) t.skip(`no table of releases at ${table} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus's build.sh puts one in its roots)`);
  const prog = await d.build('osversion');
  // The root: this Mac's own files that say which it is, the table, and
  // the program, at /bin/osversion.
  const tree = d.n.dir('osversion-root');
  for (const dir of [CORE, '/usr/share/gex', '/bin']) mkdirSync(join(tree, dir), { recursive: true });
  for (const f of ['SystemVersion.plist', 'iOSSystemVersion.plist']) copyFileSync(join(CORE, f), join(tree, CORE, f));
  copyFileSync(table, join(tree, 'usr/share/gex/macos-releases'));
  copyFileSync(prog.gex, join(tree, 'bin/osversion'));
  chmodSync(join(tree, 'bin/osversion'), 0o755);

  const want = await d.mac(prog, ['exec', prog.mac]);
  if (want.code !== 0) throw new Error(`the Mac's run: exit ${want.code}: ${want.stderr.trim().split('\n').slice(-3).join(' / ')}`);
  // What the program it becomes says: the root's release, whatever its
  // parent was told.
  const second = want.stdout.slice(want.stdout.indexOf('second: '));

  // Each release told, as a Mac of it says it.
  const head = readFileSync(join(CORE, 'SystemVersion.plist'), 'utf8').split('\n').slice(0, 4).join('\n');
  const plist = (keys) => `${head}\n${keys.map(([k, v]) => `\t<key>${k}</key>\n\t<string>${v}</string>\n`).join('')}</dict>\n</plist>\n`;
  const said = (file, text) => (text == null ? `first: ${CORE}/${file}: error 2 (stat 2)\n` : `first: ${CORE}/${file}: ${Buffer.byteLength(text)} bytes; stat and fstat size it, lseek to its end agrees\n${text}`);
  const told = (chosen, { product, build, darwin: release, compat = '', ios, xnu, intel, version, compatFile, iosFile }) =>
    [
      `first: told ${chosen}`,
      'first: kern.ostype: Darwin',
      `first: kern.osrelease: ${release}`,
      `first: kern.osversion: ${build}`,
      `first: kern.osproductversion: ${product}`,
      `first: kern.osproductversioncompat: ${compat}`,
      `first: kern.iossupportversion: ${ios ?? 'error 2'}`,
      `first: kern.version: Darwin Kernel Version ${release}: gex; root:${xnu}/RELEASE_${x86 && intel ? 'X86_64' : 'ARM64_'}`,
      'first: kern.osrevision: 199506',
      // (An Intel Mac has no such name: nothing there is translated.)
      `first: sysctl.proc_translated: ${x86 && intel ? 'error 2' : x86 ? 1 : 0}`,
      '',
    ].join('\n') + said('SystemVersion.plist', version) + said('SystemVersionCompat.plist', compatFile) + said('iOSSystemVersion.plist', iosFile) + second;
  const sdk26 = existsSync(SDK26) ? readFileSync(SDK26, 'utf8') : null;
  const ios26 = sdk26 && sdk26.replace('<string>macOS</string>', '<string>iPhone OS</string>');
  const sonoma = (v) => plist([['ProductBuildVersion', '23J631'], ['ProductName', 'macOS'], ['ProductUserVisibleVersion', v], ['ProductVersion', v]]);
  const runs = [
    { label: 'the root\'s own release', env: {}, want: want.stdout },
    ...(sdk26
      ? [{ label: 'told 26.5', env: { GEX_MACOS_VERSION: '26.5' }, want: told('26.5', { product: '26.5', build: '25F70', darwin: '25.5.0', ios: '26.5', xnu: 'xnu-12377.121.6', intel: true, version: sdk26, iosFile: ios26 }) }]
      : []),
    // (No kernel's name known for 14.8.9: this one's, gex's own.)
    { label: 'told sonoma', env: { GEX_MACOS_VERSION: 'sonoma' }, want: told('sonoma', { product: '14.8.9', build: '23J631', darwin: '23.6.0', compat: '10.16', xnu: 'xnu-12377.1.9', intel: true, version: sonoma('14.8.9'), compatFile: sonoma('10.16') }) },
  ];
  if (!sdk26) t.say(`(no ${SDK26}: not told 26.5)`);

  const differ = (a, b) => {
    const x = a.split('\n');
    const y = b.split('\n');
    const i = x.findIndex((l, n) => l !== y[n]);
    return i < 0 ? `${y.length} lines, not ${x.length}` : `line ${i + 1}: want ${JSON.stringify(x[i])}, got ${JSON.stringify(y[i] ?? null)}`;
  };
  const exe = { name: 'osversion', gex: join(tree, 'bin/osversion') };
  const seed = join(ROOT, `page/public/gex/seed${x86 ? '.x86' : ''}.shared.wasm`);
  for (const r of runs) {
    const got = await d.gex(exe, ['exec', '/bin/osversion'], { env: r.env, gexc: ['--sysroot', tree, '--argv0', '/bin/osversion'] });
    t.check(`${r.label}: gexc`, got.stdout === r.want && got.code === 0, `exit ${got.code}: ${differ(r.want, got.stdout)} / ${got.stderr.trim().split('\n').slice(-3).join(' / ')}`);
    if (!existsSync(seed)) continue;
    const env = Object.entries({ T_VAR: 'yes', ...r.env }).flatMap(([k, v]) => ['--env', `${k}=${v}`]);
    const page = await d.n.g.run(seed, exe.gex, ['exec', '/bin/osversion'], { argv0: '/bin/osversion', env: { GEX_SHARED: '1' }, flags: ['--dir', `/=${tree}`, '--cwd', '/', ...env], timeout: 240000 });
    const out = page.stdout.split('\n').filter((l) => !l.startsWith('[')).join('\n');
    t.check(`${r.label}: on the page runtime`, out === r.want && page.code === 0, `exit ${page.code}: ${differ(r.want, out)} / ${page.stderr.trim().split('\n').slice(-3).join(' / ')}`);
  }
  if (!existsSync(seed)) t.say(`(no ${seed}: no page leg)`);
}
