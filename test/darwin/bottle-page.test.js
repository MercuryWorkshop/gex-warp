// Bottles through the page runtime's API (page/runtime/src/bottles.js): the
// Mac root (pyrus's, the `darwin-root` image) laid at /opt/mac of the
// system's tree, the system layer; two bottles made over it with `Bottles`,
// listed, configured and deleted, and the root's own tools run in each
// (`run`: the program by its path in the bottle), which say what they are
// told there: the macOS and the Mac chosen for the bottle, by sysctl and
// uname; what one bottle writes the other has not; the system layer is
// read-only to both and the same for both; a bottle told to share nothing
// has no volume of the system outside. What is asked of a Mac is the row of
// the root's own table for it (`machines`).
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { Bottles, parseConf, formatConf } from '../../page/runtime/src/bottles.js';
import { bytesOf } from '../../page/runtime/src/internal.js';

export const meta = { hosts: ['browser', 'node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 600 };

export default async function (t) {
  const { check, eq, timeout } = t;
  const x86 = t.arch === 'x86';
  let tar;
  try {
    tar = await bytesOf(t.work(`darwin-root.${t.arch}.tar`));
  } catch {
    t.skip(`no Mac root for ${t.arch} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  }
  const text = 'name = a\nmacos = 14\n\n[app TextEdit]\nmachine = Macmini9,1\n';
  eq('a configuration is read and written as it was', formatConf(parseConf(`# said\n${text}[other]\nx = 1\n`)), text);

  const sys = new System({ baseUrl: t.baseUrl, arch: x86 ? 'x86' : 'aarch64', debug: t.debug() });
  const fs = new MemoryFilesystem(sys);
  fs.addTar(tar, { at: '/opt/mac' });
  const bottles = new Bottles(sys, { dir: '/bottles', system: '/opt/mac' });
  await timeout(sys.boot(), 60000, 'boot');
  await fs.mkdir('/tmp', { recursive: true });

  const machines = await bottles.machines();
  if (!machines.length) t.skip('the Mac root has no table of Macs (a pyrus root built since it had one has)');
  const releases = await bottles.releases();
  const sonoma = releases.find((r) => r.product.startsWith('14.'));
  // (An x86-64 program on macOS 14 is on an Intel Mac.)
  const mac = machines.find((m) => m.model === (x86 ? 'iMac20,1' : 'Macmini9,1'));
  const appMac = machines.find((m) => m.model === (x86 ? 'MacPro7,1' : 'Mac13,1'));
  check('the root\'s tables are read: its releases and its Macs', !!sonoma && !!mac && !!appMac && machines.some((m) => m.model === 'Mac14,3'), JSON.stringify(machines.map((m) => m.model)));

  const one = await timeout(bottles.create('one'), 120000, 'a bottle made');
  const two = await timeout(bottles.create('two', { macos: '14', machine: mac.model }), 120000, 'another made');
  check('a bottle has a name, a system layer and identifiers of its own', one.settings.name === 'one' && one.settings.system === '/opt/mac' && /^[0-9A-F-]{36}$/.test(one.settings.uuid) && one.settings.uuid !== two.settings.uuid && one.settings.serial !== two.settings.serial, JSON.stringify(one.settings));
  eq('the bottles there are', (await bottles.list()).map((b) => [b.name, b.dir, b.settings.macos ?? null, b.settings.machine ?? null]), [['one', '/bottles/one', null, null], ['two', '/bottles/two', '14', mac.model]]);
  check('its own directories are there, with the first person\'s home', (await fs.stat('/bottles/one/Users/user')).type === 'directory' && (await fs.stat('/bottles/one/private/var')).type === 'directory' && (await fs.stat('/bottles/one/Applications')).type === 'directory');
  check('a second bottle of a name is refused', await bottles.create('one').then(() => false, (e) => e.code === 'EEXIST'));
  check('a macOS the root has no row of is refused', await bottles.set('one', { macos: '9' }).then(() => false, (e) => e instanceof RangeError));
  check('a setting there is not is refused', await bottles.set('one', { colour: 'red' }).then(() => false, (e) => e instanceof TypeError));
  check('no such bottle', await bottles.get('none').then(() => false, (e) => e.code === 'ENOENT'));

  const told = async (name, argv = ['/usr/sbin/sysctl', 'hw.model', 'kern.osproductversion', 'kern.version', 'gex.bottle.name', 'gex.bottle.graphics', 'hw.target']) => {
    const r = await timeout(bottles.run(name, argv), 240000, `${argv[0]} in ${name}`);
    // (sysctl stops at a name that is not there, with a status that says
    // so, and a Mac whose row does not know its board has no hw.target:
    // the name is asked last, and what was printed is what is read.)
    return r.code === 0 || (argv[0].endsWith('/sysctl') && r.stdout) ? r.stdout : `exit ${r.code}: ${r.stdout}${r.stderr}`.trim();
  };
  const said = (out) => Object.fromEntries(out.split('\n').filter((l) => l.includes(': ')).map((l) => [l.slice(0, l.indexOf(': ')), l.slice(l.indexOf(': ') + 2)]));
  const ends = (m) => (m.arch === 'x86_64' ? '/RELEASE_X86_64' : `/RELEASE_ARM64${m.soc ? `_${m.soc}` : ''}`);
  const kernel = (s) => (s['kern.version'] ?? '').slice((s['kern.version'] ?? '').lastIndexOf('/'));

  const plain = said(await told('one'));
  check('a bottle with no settings is told the default Mac', plain['hw.model'] === 'Mac14,3' && plain['gex.bottle.name'] === 'one' && plain['gex.bottle.graphics'] === 'honest', JSON.stringify(plain));
  const chosen = said(await told('two'));
  check(`a bottle told a macOS and a Mac says so (${mac.model} on ${sonoma.product})`, chosen['hw.model'] === mac.model && chosen['kern.osproductversion'] === sonoma.product && kernel(chosen) === ends(mac) && (mac.target ? chosen['hw.target'] === mac.target : true), JSON.stringify(chosen));
  const uname = await told('two', ['/usr/bin/uname', '-v']);
  eq('uname says what sysctl does', uname.trim(), chosen['kern.version']);

  // An application's own settings, and their being forgotten.
  await bottles.set('two', { machine: appMac.model, graphics: 'machine' }, { app: 'sysctl' });
  eq('an application\'s section', (await bottles.get('two')).apps, { sysctl: { machine: appMac.model, graphics: 'machine' } });
  const app = said(await told('two'));
  check(`an application's own Mac is that application's (${appMac.model})`, app['hw.model'] === appMac.model && kernel(app) === ends(appMac) && app['gex.bottle.graphics'] === (appMac.gpufamily ?? 'honest').toLowerCase() && (await told('two', ['/usr/bin/uname', '-v'])).trim() === chosen['kern.version'], JSON.stringify(app));
  await bottles.removeApp('two', 'sysctl');
  await bottles.set('two', { machine: null });
  const dflt = said(await told('two'));
  check('a setting forgotten is the default again: a Mac that ran that macOS', dflt['hw.model'] === (x86 ? 'MacBookPro16,1' : 'Mac14,3') && dflt['kern.osproductversion'] === sonoma.product, JSON.stringify(dflt));

  // What one writes the other has not; the system layer neither writes.
  const sh = (name, script) => told(name, ['/bin/sh', '-c', script]);
  eq('a bottle writes its home and its /Library, and not the system layer',
    await sh('one', 'pwd; echo mine > note && cat ~/note; mkdir -p /usr/lib /Library/Made && echo made; echo x > /usr/lib/new 2>/dev/null || echo refused; rm /bin/sh 2>/dev/null || echo kept'),
    '/Users/user\nmine\nmade\nrefused\nkept\n');
  eq('what it wrote is in its directory', await fs.readFile('/bottles/one/Users/user/note', { encoding: 'utf-8' }), 'mine\n');
  check('and nothing in the system layer', !(await fs.stat('/opt/mac/usr/lib/new').then(() => true, () => false)) && !(await fs.stat('/opt/mac/Library/Made').then(() => true, () => false)));
  eq('the other bottle has none of it, and the same system',
    await sh('two', 'ls ~/note /Library/Made 2>/dev/null | wc -l | tr -d " "; ls /usr/lib/dyld; ls -d /Volumes/Linux/bottles/one'),
    '0\n/usr/lib/dyld\n/Volumes/Linux/bottles/one\n');
  await bottles.set('two', { 'share.system': false });
  eq('a bottle told to share nothing has no volume of the system outside', await sh('two', 'ls -d /Volumes/Linux/bottles 2>/dev/null || echo none'), 'none\n');

  await bottles.delete('two');
  eq('a bottle deleted is gone, the other stays', [(await bottles.list()).map((b) => b.name), await fs.stat('/bottles/two').then(() => true, () => false)], [['one'], false]);
  await sys.shutdown();
}
