// Bottles (crates/gex-kernel/src/darwin/bottle.rs): Mac environments that
// share one Mac root, as Wine prefixes share one Wine. `gexc bottle` makes
// two over the root pyrus builds, and fixtures/darwin/bottle/told.m, a
// Foundation program put in each bottle's /Applications, says what it is
// told there by sysctl, uname, Foundation and gethostuuid:
//
//   - a bottle with no settings is told the root's macOS and the default
//     Mac of it; one with a macOS and a Mac chosen is told those, every
//     answer together (the model, its board, its chip in kern.version and
//     uname, its processor family and counts, the release's Darwin), with
//     the row of the root's tables (/usr/share/gex/mac-models,
//     macos-releases) as what must be said; an application's own settings
//     are that application's alone; the environment's word is over both;
//   - each has identifiers of its own;
//   - what one writes in its home and its /Library the other has not, and
//     is in the bottle's directory;
//   - the system layer cannot be written, by either, and is not.
//
// Under gexc (the bottle's directories mounted over a read-only root), and
// on the page's runtime under Node, where the root is a directory of
// another tree and the kernel enters the bottle as the process becomes the
// program (GEX_BOTTLE; a Linux program, fixtures/darwin/bottle/run.c,
// becomes it), a shell there running the program as its child.
// The root: GEX_TEST_DARWIN_ROOT (or _X86), by default ../pyrus/out/root.
import { execFileSync } from 'node:child_process';
import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 900 };

const rows = (file, fields) => readFileSync(file, 'utf8').split('\n').filter((l) => l.trim() && !l.startsWith('#')).map((l) => {
  const f = l.split('\t');
  return Object.fromEntries(fields.map((name, i) => [name, !f[i] || f[i] === '-' ? null : f[i]]));
});
const said = (text) => Object.fromEntries(text.split('\n').filter((l) => l.includes(': ')).map((l) => [l.slice(0, l.indexOf(': ')), l.slice(l.indexOf(': ') + 2)]));

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  if (!existsSync(join(mac, 'usr/share/gex/mac-models'))) t.skip(`no table of Macs in ${mac} (a pyrus root built since it had one has)`);
  const models = rows(join(mac, 'usr/share/gex/mac-models'), ['model', 'arch', 'target', 'targettype', 'soc', 'cpufamily', 'cpusubfamily', 'pcores', 'ecores', 'threads', 'brand', 'first', 'last', 'gpu', 'name']);
  const releases = rows(join(mac, 'usr/share/gex/macos-releases'), ['product', 'build', 'darwin']);
  const n = native(t);
  const work = n.dir('bottle');
  const prog = join(work, 'told');
  try {
    execFileSync('xcrun', ['clang', '-arch', x86 ? 'x86_64' : 'arm64', '-fobjc-arc', '-O1', '-Wall', '-Werror', '-o', prog, join(ROOT, 'test/fixtures/darwin/bottle/told.m'), '-framework', 'Foundation'], { stdio: 'pipe' });
  } catch (e) {
    t.skip(`xcrun clang: ${String(e.stderr || e).trim().split('\n').pop()}`);
  }

  // The Macs chosen: the bottle's, one application's, the environment's.
  // An x86-64 program on macOS 14 is on an Intel Mac; on the root's own 27
  // it is translated, on the Mac an arm64 program is.
  const model = (id) => models.find((m) => m.model === id);
  const [bottleMac, appMac, envMac] = x86 ? ['iMac20,1', 'MacPro7,1', 'MacBookPro16,1'] : ['Macmini9,1', 'Mac16,7', 'Mac13,1'];
  for (const id of [bottleMac, appMac, envMac, 'Mac14,3']) if (!model(id)) t.skip(`the root's table has no ${id}`);
  const sonoma = releases.find((r) => r.product.startsWith('14.'));
  const tahoe = releases.find((r) => r.product.startsWith('26.') && r.darwin) ?? releases.find((r) => r.product.startsWith('26.'));
  const own = readFileSync(join(mac, 'System/Library/CoreServices/SystemVersion.plist'), 'utf8').match(/<key>ProductVersion<\/key>\s*<string>([^<]+)</)[1];

  // What a program must be told of a Mac it was told to be on.
  const wants = (m, product) => {
    const intel = m.arch === 'x86_64';
    const cpus = m.threads ?? (m.pcores ? String(Number(m.pcores) + Number(m.ecores ?? 0)) : null);
    return {
      'hw.model': m.model,
      'hw.target': m.target ?? 'error 2',
      'hw.targettype': intel ? 'error 2' : m.targettype ?? 'error 2',
      'machdep.cpu.brand_string': m.brand ?? 'error 2',
      'hw.cpufamily': `0x${(Number(m.cpufamily ?? 0) >>> 0).toString(16).padStart(8, '0')}`,
      'hw.cpusubfamily': String(Number(m.cpusubfamily ?? 0)),
      ...(cpus ? { 'hw.ncpu': cpus, 'foundation: processors': cpus } : {}),
      ...(m.pcores && Number(m.ecores) > 0 ? { 'hw.nperflevels': '2', 'hw.perflevel0.physicalcpu': m.pcores, 'hw.perflevel1.physicalcpu': m.ecores } : { 'hw.nperflevels': '1', 'hw.perflevel1.physicalcpu': 'error 2' }),
      'kern.osproductversion': product,
      'kern.version ends': intel ? '/RELEASE_X86_64' : `/RELEASE_ARM64${m.soc ? `_${m.soc}` : ''}`,
    };
  };
  const differs = (got, want) => {
    const g = said(got);
    g['kern.version ends'] = (g['kern.version'] ?? '').slice((g['kern.version'] ?? '').lastIndexOf('/'));
    g['foundation: processors'] = (/^foundation: (\d+) processors$/m.exec(got) ?? [])[1];
    const uname = /^uname: Darwin (\S+) (.*) (\S+)$/m.exec(got);
    const bad = Object.entries(want).filter(([k, v]) => g[k] !== v).map(([k, v]) => `${k}: want ${v}, got ${g[k]}`);
    if (!uname || uname[1] !== g['kern.osrelease'] || uname[2] !== g['kern.version']) bad.push(`uname does not say what sysctl does: ${uname?.[0]}`);
    if (!new RegExp(`^foundation: ${want['kern.osproductversion'].split('.').concat('0', '0').slice(0, 3).join('\\.')}$`, 'm').test(got)) bad.push(`Foundation's version is not ${want['kern.osproductversion']}: ${/^foundation: [\d.]+$/m.exec(got)?.[0]}`);
    return bad.join('; ');
  };

  // One host's bottles: made by gexc in `dir`, over the root at `system`.
  const made = async (dir, system) => {
    const env = { GEX_BOTTLES: dir };
    const gexc = (args, o = {}) => n.gexc(['bottle', ...args], { env, timeout: 300000, ...o });
    const one = await gexc(['create', 'one', '--system', system]);
    const two = await gexc(['create', 'two', '--system', system, `macos=${sonoma.product.split('.')[0]}`, `machine=${bottleMac}`]);
    t.check('two bottles made', one.code === 0 && two.code === 0 && existsSync(join(dir, 'two/Library/Preferences/bottle.conf')), `${one.stderr}${two.stderr}`.trim().split('\n').slice(-2).join(' / '));
    const refused = await gexc(['create', 'three', '--system', system, 'macos=9']);
    t.check('a macOS the root has no row of is refused, and no bottle made', refused.code !== 0 && /no such release/.test(refused.stderr) && !existsSync(join(dir, 'three')), refused.stderr.trim());
    const set = await gexc(['set', 'two', '--app', 'told', `macos=${tahoe.product}`, `machine=${appMac}`, 'graphics=machine']);
    t.check('an application given settings of its own', set.code === 0, set.stderr.trim());
    for (const b of ['one', 'two']) {
      for (const name of ['told', 'other']) {
        copyFileSync(prog, join(dir, b, 'Applications', name));
        chmodSync(join(dir, b, 'Applications', name), 0o755);
      }
    }
    const list = await gexc(['list']);
    t.check('both are listed, with what each was told', list.stdout === `one\t-\t-\t${system}\ntwo\t${sonoma.product.split('.')[0]}\t${bottleMac}\t${system}\n`, JSON.stringify(list.stdout));
    return gexc;
  };

  // What each program of each bottle must be told, by `run(bottle, program,
  // args, env)` -> its output.
  // `disk`: what a run writes is on this machine's disk afterwards (gexc's
  // is; the page's tree is its own memory, each run's anew).
  const checks = async (leg, dir, run, sealed, disk) => {
    const conf = (b) => said(readFileSync(join(dir, b, 'Library/Preferences/bottle.conf'), 'utf8').replace(/ = /g, ': '));
    // (Under Rosetta an x86-64 program is told the processor family
    // Rosetta says, not the Mac's: CPUFAMILY_INTEL_WESTMERE, on this Mac.)
    const rosetta = (w) => (x86 ? { ...w, 'hw.cpufamily': '0x573b5eec', 'hw.cpusubfamily': '0', 'sysctl.proc_translated': '1' } : w);
    const plain = await run('one', 'other');
    const { 'hw.ncpu': _n, 'foundation: processors': _p, 'hw.nperflevels': _l, 'hw.perflevel0.physicalcpu': _f, 'hw.perflevel1.physicalcpu': _e, ...dflt } = wants(model('Mac14,3'), own);
    t.check(`${leg}: a bottle with no settings is told the root's macOS and its default Mac`, !differs(plain, rosetta(dflt)) && said(plain)['hw.nperflevels'] === '1', differs(plain, rosetta(dflt)) || plain.slice(-300));
    const chosen = await run('two', 'other');
    t.check(`${leg}: a bottle told a macOS and a Mac says so by every answer (${bottleMac} on ${sonoma.product})`, !differs(chosen, wants(model(bottleMac), sonoma.product)), differs(chosen, wants(model(bottleMac), sonoma.product)) || chosen.slice(-300));
    const app = await run('two', 'told');
    const appWant = wants(model(appMac), tahoe.product);
    t.check(`${leg}: an application's own settings are that application's (${appMac} on ${tahoe.product})`, !differs(app, appWant), differs(app, appWant) || app.slice(-300));
    t.eq(`${leg}: the graphics profile is the one its settings come to`, [said(plain)['gex.bottle.graphics'], said(app)['gex.bottle.graphics']], ['honest', (model(appMac).gpu ?? 'honest').toLowerCase()]);
    const env = await run('two', 'told', [], { GEX_MAC_MODEL: envMac, GEX_MACOS_VERSION: sonoma.product });
    t.check(`${leg}: the environment's word is over both (${envMac} on ${sonoma.product})`, !differs(env, wants(model(envMac), sonoma.product)), differs(env, wants(model(envMac), sonoma.product)) || env.slice(-300));
    const ids = ['one', 'two'].map((b) => [said(b === 'one' ? plain : chosen).hostuuid, said(b === 'one' ? plain : chosen)['gex.bottle.serial'], said(b === 'one' ? plain : chosen)['gex.bottle.name']]);
    t.check(`${leg}: each bottle has its own identifiers, the ones it was made with`, ids[0][0] !== ids[1][0] && ['one', 'two'].every((b, i) => ids[i][0] === conf(b).uuid && ids[i][1] === conf(b).serial && ids[i][2] === b), JSON.stringify(ids));

    // What one writes the other has not; the system layer neither writes.
    const wrote = await run('one', 'told', ['write', '/Users/user/note', `${leg} one`, 'write', '/Library/Preferences/note', 'kept', 'write', '/usr/lib/note', 'no', 'write', '/System/note', 'no', 'write', '/Applications/note', 'app']);
    t.check(`${leg}: a bottle writes its home, its /Library and its /Applications`, ['/Users/user/note', '/Library/Preferences/note', '/Applications/note'].every((p) => wrote.includes(`write ${p}: done\n`)), wrote.slice(0, 300));
    t.check(`${leg}: and not the system layer (EROFS)`, wrote.includes('write /usr/lib/note: error 30\n') && wrote.includes('write /System/note: error 30\n'), wrote.slice(0, 300));
    t.check(`${leg}: nothing was written in the system layer`, !existsSync(join(sealed, 'usr/lib/note')) && !existsSync(join(sealed, 'System/note')) && !existsSync(join(sealed, 'Users')));
    const other = await run('two', 'told', ['read', '/Users/user/note', 'read', '/Library/Preferences/note', 'read', '/usr/share/gex/mac-models']);
    t.check(`${leg}: the other bottle has none of it, and the same system`, other.includes('read /Users/user/note: error 2\n') && other.includes('read /Library/Preferences/note: error 2\n') && /^read \/usr\/share\/gex\/mac-models: #/m.test(other), other.slice(0, 300));
    if (!disk) return;
    t.check(`${leg}: what it wrote is in the bottle's directory`, existsSync(join(dir, 'one/Users/user/note')) && readFileSync(join(dir, 'one/Users/user/note'), 'utf8') === `${leg} one\n` && existsSync(join(dir, 'one/Library/Preferences/note')));
    const again = await run('one', 'told', ['read', '/Users/user/note']);
    t.check(`${leg}: and the bottle that wrote it has it still`, again.includes(`read /Users/user/note: ${leg} one\n`), again.slice(0, 200));
  };

  // Under gexc: the bottle's directories over the root, read-only.
  {
    const dir = n.dir('bottles');
    const gexc = await made(dir, mac);
    await checks('gexc', dir, async (b, name, args = [], env = {}) => {
      const r = await gexc(['run', b, ...Object.entries(env).flatMap(([k, v]) => ['--env', `${k}=${v}`]), `/Applications/${name}`, ...args]);
      return r.code === 0 ? r.stdout : `exit ${r.code}: ${r.stderr.trim().split('\n').slice(-3).join(' / ')}`;
    }, mac, true);
    const sh = await gexc(['run', 'two', '/bin/sh', '-c', 'mkdir -p /usr/lib /Library/Made && echo made; echo x > /bin/new || echo refused; rm /bin/sh || echo kept; cd && pwd']);
    t.eq('gexc: a shell there: a directory the layer has is found made, the layer unchanged, the home the bottle\'s', sh.stdout, 'made\nrefused\nkept\n/Users/user\n');
    const gone = await gexc(['delete', 'two']);
    const left = await gexc(['list']);
    t.check('a bottle deleted is gone, the other stays', gone.code === 0 && !existsSync(join(dir, 'two')) && left.stdout.startsWith('one\t') && left.stdout.split('\n').length === 2, left.stdout);
  }

  // On the page's runtime: the root a directory of another tree, entered.
  const seed = join(ROOT, `page/public/gex/seed${x86 ? '.x86' : ''}.shared.wasm`);
  if (!existsSync(seed)) {
    t.say(`(no ${seed}: no page leg; hosts/browser/build.sh${x86 ? ', GEX_ARCH=x86' : ''})`);
    return;
  }
  const tree = n.dir('tree');
  for (const d of ['tmp', 'opt', 'bottles', 'bin']) mkdirSync(join(tree, d), { recursive: true });
  // (The first program of a run there is loaded as it is: a Linux one,
  // which becomes the Mac program, and the kernel enters the bottle then.)
  await n.cc(['-static', '-O1', '-o', join(tree, 'bin/run'), join(ROOT, 'test/fixtures/darwin/bottle/run.c')]);
  execFileSync('cp', ['-Rc', mac, join(tree, 'opt/mac')]);
  const dir = join(tree, 'bottles');
  await made(dir, join(tree, 'opt/mac'));
  // (The system layer by its name in the tree the page has, not the disk's.)
  for (const b of ['one', 'two']) {
    const conf = join(dir, b, 'Library/Preferences/bottle.conf');
    writeFileSync(conf, readFileSync(conf, 'utf8').replace(/^system = .*$/m, 'system = /opt/mac'));
  }
  const page = async (b, argv, env = {}) => {
    const flags = ['--dir', `/=${tree}`, '--cwd', '/', ...Object.entries({ GEX_BOTTLE: `/bottles/${b}`, HOME: '/Users/user', PATH: '/usr/bin:/bin', ...env }).flatMap(([k, v]) => ['--env', `${k}=${v}`])];
    const r = await n.g.run(seed, join(tree, 'bin/run'), [`/${argv[0]}`, ...argv.slice(1)], { env: { GEX_SHARED: '1' }, flags, timeout: 300000 });
    const out = r.stdout.split('\n').filter((l) => !l.startsWith('[')).join('\n');
    return r.code === 0 ? out : `exit ${r.code}: ${out.slice(-200)} / ${r.stderr.trim().split('\n').slice(-3).join(' / ')}`;
  };
  await checks('the page runtime', dir, (b, name, args = [], env = {}) => page(b, [`bottles/${b}/Applications/${name}`, ...args], env), join(tree, 'opt/mac'), false);
  // A shell of the system layer, entered into the bottle, and its
  // children: a fork keeps the bottle (its binds, the layer sealed), and
  // what is written there is, by the system outside's names (the volume),
  // in the bottle's directory and neither the layer's nor the other's.
  const script = [
    'mkdir -p /usr/lib /Library/Made && echo made',
    'echo x > /bin/new || echo refused',
    '/Applications/told write /Users/user/note mine | head -1',
    'cat /Volumes/Linux/bottles/two/Users/user/note',
    'ls -d /Volumes/Linux/bottles/two/Library/Made',
    'ls -d /Volumes/Linux/opt/mac/Library/Made /Volumes/Linux/opt/mac/bin/new /Volumes/Linux/opt/mac/Users /Volumes/Linux/bottles/one/Users/user/note /Volumes/Linux/bottles/one/Library/Made 2>/dev/null | wc -l | tr -d " "',
    'cd && pwd',
  ].join('; ');
  const sh = await page('two', ['opt/mac/bin/sh', '-c', script]);
  t.eq('the page runtime: a shell there and its children: in the bottle, the layer sealed, what they write in the bottle\'s directory alone', sh.replace(/^.*(cannot create|Read-only).*\n/m, ''),
    'made\nrefused\nwrite /Users/user/note: done\nmine\n/Volumes/Linux/bottles/two/Library/Made\n0\n/Users/user\n');
  // A bottle that shares nothing of the system outside.
  const conf = join(dir, 'two/Library/Preferences/bottle.conf');
  writeFileSync(conf, readFileSync(conf, 'utf8').replace(/^system = .*$/m, 'system = /opt/mac\nshare.system = no'));
  const alone = await page('two', ['opt/mac/bin/sh', '-c', 'ls -d /Volumes/Linux/bottles 2>/dev/null || echo none; ls -d /Users/user']);
  t.eq('the page runtime: a bottle told to share nothing has no volume of the system outside', alone, 'none\n/Users/user\n');
}
