// `Bottles`: Mac environments that share one Mac root, as Wine prefixes
// share one Wine (crates/gex-kernel/src/darwin/bottle.rs says what a bottle
// is). The root (pyrus's) is a directory of the system's tree, the system
// layer, which no bottle writes; a bottle is a directory with its own
// `Users`, `Applications`, `Library` and `private/var`, which the kernel
// binds over the layer's when a Mac program is started with `GEX_BOTTLE`
// naming the bottle, and its configuration,
// `Library/Preferences/bottle.conf`, which the kernel reads when a program
// starts: which macOS and which Mac the bottle's programs (or one
// application's) are told they are on, the graphics profile, and what of
// the system outside is shared in.
//
// This is the files, over the system's Filesystem, as `gexc bottle` is over
// a disk's: the two write the same bottles. A bottle is kept as the
// filesystem keeps anything: across loads with a store (OPFSFilesystem),
// not at all in a MemoryFilesystem.
//
//   const bottles = new Bottles(sys, { dir: '/bottles', system: '/opt/mac' });
//   await bottles.create('work', { macos: '14', machine: 'Macmini9,1' });
//   await bottles.set('work', { macos: '11' }, { app: 'TextEdit' });
//   const { stdout } = await bottles.run('work', ['/usr/bin/uname', '-v']);

import { internals } from './internal.js';

/** Where a bottle's configuration is, in the bottle and in its root. */
export const CONF = '/Library/Preferences/bottle.conf';
/** What of a root is a bottle's own. */
export const PARTS = ['/Users', '/Applications', '/Library', '/private/var'];
/** The settings a bottle has, and the ones an application's section may. */
export const KEYS = ['name', 'system', 'uuid', 'serial', 'macos', 'machine', 'graphics', 'share.system', 'share.home'];
export const APP_KEYS = ['macos', 'machine', 'graphics'];

const RELEASES = '/usr/share/gex/macos-releases';
const MODELS = '/usr/share/gex/mac-models';
const MODEL_FIELDS = ['model', 'arch', 'target', 'targettype', 'soc', 'cpufamily', 'cpusubfamily', 'pcores', 'ecores', 'threads', 'brand', 'first', 'last', 'gpufamily', 'name'];
const RELEASE_FIELDS = ['product', 'build', 'darwin', 'xnu', 'kdate', 'compat', 'ios', 'year'];

/** A configuration's text as `{ settings: {key: value}, apps: {name: {key: value}} }`. */
export function parseConf(text) {
  const conf = { settings: {}, apps: {} };
  let into = conf.settings;
  for (const raw of String(text).split('\n')) {
    const line = raw.trim();
    if (!line || line.startsWith('#')) continue;
    if (line.startsWith('[') && line.endsWith(']')) {
      const head = line.slice(1, -1).trim();
      const name = head.startsWith('app') ? head.slice(3).trim() : '';
      // (A section nobody knows: its lines are nobody's.)
      into = name ? (conf.apps[name] ??= {}) : null;
      continue;
    }
    const eq = line.indexOf('=');
    if (eq < 0 || !into) continue;
    const [k, v] = [line.slice(0, eq).trim(), line.slice(eq + 1).trim()];
    if (v) into[k] = v;
    else delete into[k];
  }
  return conf;
}

/** The text of a configuration. */
export function formatConf(conf) {
  const lines = (o) => Object.entries(o).map(([k, v]) => `${k} = ${v}\n`).join('');
  return lines(conf.settings) + Object.entries(conf.apps ?? {}).map(([name, o]) => `\n[app ${name}]\n${lines(o)}`).join('');
}

function rows(text, fields) {
  return String(text).split('\n').filter((l) => l.trim() && !l.startsWith('#')).map((l) => {
    const f = l.split('\t');
    return Object.fromEntries(fields.map((name, i) => [name, f[i] === undefined || f[i] === '-' || f[i] === '' ? null : f[i]]));
  });
}

function checkName(what, name) {
  if (typeof name !== 'string' || !name || /[/\n\r[\]]/.test(name) || name !== name.trim() || name === '.' || name === '..') throw new TypeError(`${what}: ${JSON.stringify(name)} is not a name`);
  return name;
}

function checked(settings, app) {
  const known = app ? APP_KEYS : KEYS;
  const out = {};
  for (const [k, v] of Object.entries(settings ?? {})) {
    if (!known.includes(k)) throw new TypeError(`no setting ${k} (there are: ${known.join(', ')})`);
    const s = v === null || v === undefined || v === false ? (k === 'share.system' && v === false ? 'no' : '') : v === true ? 'yes' : String(v).trim();
    if (/[\n\r]/.test(s) || s.startsWith('[')) throw new TypeError(`${k}: not a value that fits a line`);
    if (k === 'share.system' && !['', 'yes', 'no'].includes(s)) throw new TypeError('share.system is yes or no');
    out[k] = s;
  }
  return out;
}

export class Bottles {
  #sys;
  #internal;
  #dir;
  #system;

  /** `dir`: the directory the bottles are in (made when the first is);
   *  `system`: the Mac root new bottles are over, a directory of the
   *  system's tree with pyrus's root in it. */
  constructor(sys, { dir = '/bottles', system = '/opt/mac' } = {}) {
    this.#internal = internals(sys, 'Bottles');
    this.#sys = sys;
    for (const [k, v] of [['dir', dir], ['system', system]]) {
      if (typeof v !== 'string' || !v.startsWith('/')) throw new TypeError(`Bottles: ${k} is an absolute path`);
    }
    this.#dir = dir.replace(/\/+$/, '') || '/';
    this.#system = system.replace(/\/+$/, '') || '/';
    this.#internal.attach('bottles', this);
  }

  get dir() { return this.#dir; }
  get system() { return this.#system; }

  #fs() {
    const fs = this.#internal.part('filesystem');
    if (!fs) throw new Error('Bottles: the system has no Filesystem');
    return fs;
  }

  /** The directory of the bottle `name`. */
  path(name) {
    return `${this.#dir === '/' ? '' : this.#dir}/${checkName('a bottle', name)}`;
  }

  async #read(name) {
    const fs = this.#fs();
    const text = await fs.readFile(this.path(name) + CONF, { encoding: 'utf-8' }).catch(() => null);
    if (text === null) throw Object.assign(new Error(`no bottle ${name}`), { code: 'ENOENT' });
    return parseConf(text);
  }

  async #write(name, conf) {
    const fs = this.#fs();
    const path = this.path(name) + CONF;
    // Whole or not at all: a program starting meanwhile reads one or the other.
    await fs.writeFile(`${path}.new`, formatConf(conf));
    await fs.rename(`${path}.new`, path);
  }

  async #copy(from, to) {
    const fs = this.#fs();
    const st = await fs.stat(from, { follow: false });
    if (st.type === 'symlink') {
      await fs.symlink(await fs.readlink(from), to);
    } else if (st.type === 'directory') {
      await fs.mkdir(to, { recursive: true, mode: st.mode & 0o7777 });
      for (const e of await fs.readdir(from)) await this.#copy(`${from}/${e.name}`, `${to}/${e.name}`);
    } else if (st.type === 'file') {
      await fs.writeFile(to, await fs.readFile(from), { mode: st.mode & 0o7777 });
    }
  }

  async #tables(system) {
    const fs = this.#fs();
    const read = (p) => fs.readFile(system + p, { encoding: 'utf-8' }).catch(() => '');
    return { releases: rows(await read(RELEASES), RELEASE_FIELDS), machines: rows(await read(MODELS), MODEL_FIELDS) };
  }

  /** A release or a Mac the system layer's tables have, or a throw. */
  async #known(system, settings) {
    const { releases, machines } = await this.#tables(system);
    const names = { bigsur: '11', 'big sur': '11', monterey: '12', ventura: '13', sonoma: '14', sequoia: '15', tahoe: '26' };
    if (settings.macos) {
      const spec = settings.macos.toLowerCase();
      const v = (names[spec] ?? spec).replace(/^macos/, '').trim();
      if (!releases.some((r) => r.product === v || r.product.startsWith(`${v}.`))) throw new RangeError(`macos = ${settings.macos}: no such release in the system layer's table (it has ${releases.map((r) => r.product).join(' ')})`);
    }
    if (settings.machine && machines.length && !machines.some((m) => m.model.toLowerCase() === settings.machine.toLowerCase())) {
      throw new RangeError(`machine = ${settings.machine}: no such Mac in the system layer's table (it has ${machines.map((m) => m.model).join(' ')})`);
    }
  }

  /**
   * A new bottle: its own `Users`, `Applications`, `Library` and
   * `private/var`, copies of the system layer's, the first person's home,
   * and a configuration with identifiers of its own and `settings`
   * (`macos`, `machine`, `graphics`, `share.system`, `share.home`).
   * `system`: its system layer, where not the one this was made with.
   * Resolves with the bottle as `get` describes it.
   */
  async create(name, settings = {}, { system = this.#system } = {}) {
    const fs = this.#fs();
    const dir = this.path(name);
    const given = checked(settings, false);
    if (!(await fs.stat(`${system}/usr/lib/dyld`).then(() => true, () => false))) throw new Error(`Bottles: ${system} is not a Mac root (no usr/lib/dyld there)`);
    if (await fs.stat(dir).then(() => true, () => false)) throw Object.assign(new Error(`${dir}: there already`), { code: 'EEXIST' });
    await this.#known(system, given);
    await fs.mkdir(dir, { recursive: true });
    for (const part of PARTS) {
      if (await fs.stat(system + part).then((s) => s.type === 'directory', () => false)) {
        await fs.mkdir(dir + part.slice(0, part.lastIndexOf('/')) || dir, { recursive: true });
        await this.#copy(system + part, dir + part);
      } else {
        await fs.mkdir(dir + part, { recursive: true });
      }
    }
    const home = await this.#home(system);
    if (home.startsWith('/Users/')) await fs.mkdir(dir + home, { recursive: true });
    // The root's own configuration, where it had one, with what is this
    // bottle's alone.
    const conf = await this.#read(name).catch(() => ({ settings: {}, apps: {} }));
    const random = crypto.getRandomValues(new Uint8Array(26));
    random[6] = (random[6] & 0x0f) | 0x40;
    random[8] = (random[8] & 0x3f) | 0x80;
    const hex = [...random.subarray(0, 16)].map((b) => b.toString(16).padStart(2, '0').toUpperCase());
    const uuid = [hex.slice(0, 4), hex.slice(4, 6), hex.slice(6, 8), hex.slice(8, 10), hex.slice(10)].map((p) => p.join('')).join('-');
    const letters = '0123456789ABCDEFGHJKLMNPQRSTUVWXYZ';
    const serial = [...random.subarray(16)].map((r) => letters[r % letters.length]).join('');
    Object.assign(conf.settings, { name, system, uuid, serial });
    for (const [k, v] of Object.entries(given)) {
      if (v) conf.settings[k] = v;
      else delete conf.settings[k];
    }
    await fs.mkdir(dir + CONF.slice(0, CONF.lastIndexOf('/')), { recursive: true });
    await this.#write(name, conf);
    return this.get(name);
  }

  /** The first person's home in the root at `system`: user 501's. */
  async #home(system) {
    const passwd = await this.#fs().readFile(`${system}/private/etc/passwd`, { encoding: 'utf-8' }).catch(() => '');
    const f = passwd.split('\n').filter((l) => !l.startsWith('#')).map((l) => l.split(':')).find((f) => f[2] === '501');
    return f?.[5]?.startsWith('/') ? f[5] : '/Users/user';
  }

  /** The bottles there are, each as `get` describes it. */
  async list() {
    const fs = this.#fs();
    const names = await fs.readdir(this.#dir).then((l) => l.filter((e) => e.type === 'directory').map((e) => e.name).sort(), () => []);
    const out = [];
    for (const name of names) {
      const b = await this.get(name).catch(() => null);
      if (b) out.push(b);
    }
    return out;
  }

  /** The bottle `name`: `{ name, dir, settings: {key: value}, apps: {name:
   *  {key: value}} }`. Rejects (`code` 'ENOENT') when there is none. */
  async get(name) {
    const conf = await this.#read(name);
    return { name, dir: this.path(name), settings: conf.settings, apps: conf.apps };
  }

  /**
   * Change settings: `{ key: value }`, a null or empty value forgetting the
   * key. With `app`, that application's own (`macos`, `machine`,
   * `graphics`), its section made if it has none (`set(name, {}, { app })`
   * makes it alone). Programs started afterwards are told so. Resolves
   * with the bottle.
   */
  async set(name, settings, { app } = {}) {
    const conf = await this.#read(name);
    const given = checked(settings, app !== undefined);
    if ('system' in given && !given.system) throw new TypeError('a bottle has a system layer');
    await this.#known(given.system || conf.settings.system || this.#system, given);
    const into = app === undefined ? conf.settings : (conf.apps[checkName('an application', app)] ??= {});
    for (const [k, v] of Object.entries(given)) {
      if (v) into[k] = v;
      else delete into[k];
    }
    await this.#write(name, conf);
    return this.get(name);
  }

  /** Drop an application's section. */
  async removeApp(name, app) {
    const conf = await this.#read(name);
    delete conf.apps[checkName('an application', app)];
    await this.#write(name, conf);
    return this.get(name);
  }

  /** The bottle and everything in it, gone. */
  async delete(name) {
    await this.#read(name);
    await this.#fs().rm(this.path(name), { recursive: true });
  }

  /** The Macs the bottle's system layer has a row of: `{ model, arch,
   *  target, targettype, soc, cpufamily, cpusubfamily, pcores, ecores,
   *  threads, brand, first, last, gpufamily, name }`, null what is not known. */
  async machines(name) {
    const conf = name === undefined ? null : await this.#read(name);
    return (await this.#tables(conf?.settings.system ?? this.#system)).machines;
  }

  /** The macOS releases it has a row of: `{ product, build, ... }`. */
  async releases(name) {
    const conf = name === undefined ? null : await this.#read(name);
    return (await this.#tables(conf?.settings.system ?? this.#system)).releases;
  }

  /** The name outside of `inner`, a path in the bottle's root: in the
   *  home it shares in or in the bottle for what is its own, else in its
   *  system layer. */
  async outer(name, inner) {
    const conf = await this.#read(name);
    const system = conf.settings.system ?? this.#system;
    const under = (p) => inner === p || inner.startsWith(`${p}/`);
    const shared = conf.settings['share.home']?.replace(/\/+$/, '');
    if (shared) {
      const home = await this.#home(system);
      if (under(home)) return shared + inner.slice(home.length);
    }
    return (PARTS.some(under) ? this.path(name) : system) + inner;
  }

  /** The environment a program of the bottle is started with: which
   *  bottle, its user's home, a Mac's PATH; `env` over them. */
  async env(name, env = {}) {
    const conf = await this.#read(name);
    const given = Array.isArray(env) ? Object.fromEntries(env.map((e) => [e.slice(0, e.indexOf('=')), e.slice(e.indexOf('=') + 1)])) : env;
    return { PATH: '/usr/bin:/bin:/usr/sbin:/sbin', HOME: await this.#home(conf.settings.system ?? this.#system), ...given, GEX_BOTTLE: this.path(name) };
  }

  /** What `sys.spawn` takes for `argv` and `opts` in the bottle: the
   *  program and the directory by their names outside (the kernel enters
   *  the bottle as the process becomes the program, and each is then the
   *  same file by its name inside). */
  async #started(name, argv, opts) {
    const env = await this.env(name, opts.env);
    const [prog, cwd] = await Promise.all([this.outer(name, argv[0]), this.outer(name, opts.cwd ?? env.HOME)]);
    return [[prog, ...argv.slice(1)], { ...opts, env, cwd }];
  }

  /** `sys.spawn` in the bottle: `argv[0]` a path in the bottle's root
   *  (`/bin/zsh`, `/Applications/A.app/Contents/MacOS/A`), `cwd` one too
   *  (by default the user's home). Resolves with the Process. */
  async spawn(name, argv, opts = {}) {
    return this.#sys.spawn(...(await this.#started(name, argv, opts)));
  }

  /** `sys.run` in the bottle. */
  async run(name, argv, opts = {}) {
    return this.#sys.run(...(await this.#started(name, argv, opts)));
  }
}
