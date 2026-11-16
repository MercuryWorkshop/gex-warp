// The Debian image (GEX_TEST_SLIM's) laid with its owners
// (`addTar(..., { owners: true })`: made as root in a container, so what it
// names is what each file is), and real permissions on it. Root (the page's
// first user) makes a user, bob, as useradd would (the image has no
// useradd: the lines in /etc/passwd, /etc/shadow and /etc/group, and a home
// of his); as bob he may not write /usr nor read /etc/shadow, and his home
// is his. He is become with setpriv (what su does after PAM: setgroups,
// setgid, setuid): the image's su and runuser end at once ("Critical error
// - immediate abort", a known failure below, on a build before owners too).
// And apt still works with every program not root held to the owners:
// `apt-get update` and `apt-get install` of a small package from a
// repository served here (a stand-in for gex's, made by this test: one
// `Architecture: all` package), fetched through the example RepoNetwork (as
// the demo's page does). Once as the image has apt (its 99gex sets
// APT::Sandbox::User "root": the methods run as root), and once as Debian
// has it, the methods running as `_apt` into `partial` directories apt gives
// `_apt`, with no "unsandboxed" warning (which is apt saying `_apt` could
// not use them).
import { createServer } from 'node:http';
import { createReadStream, mkdirSync, statSync, writeFileSync } from 'node:fs';
import { join, normalize } from 'node:path';
import { createHash } from 'node:crypto';
import { gzipSync } from 'node:zlib';
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { RepoNetwork } from '../../page/runtime/examples/repo-network.mjs';

export const meta = { hosts: ['node'], arches: ['a64'], images: ['slim'], timeout: 900 };

const PKG = 'gex-owners-hello';
const enc = new TextEncoder();

/** A ustar archive of `entries` ({ name, body?, mode, dir? }), root's. */
function tar(entries) {
  const out = [];
  const field = (h, at, len, v) => h.set(enc.encode(v).subarray(0, len), at);
  const oct = (n, len) => n.toString(8).padStart(len - 1, '0');
  for (const e of entries) {
    const body = e.body ? enc.encode(e.body) : new Uint8Array(0);
    const h = new Uint8Array(512);
    field(h, 0, 100, e.name);
    field(h, 100, 8, oct(e.mode, 8));
    field(h, 108, 8, oct(0, 8));
    field(h, 116, 8, oct(0, 8));
    field(h, 124, 12, oct(body.length, 12));
    field(h, 136, 12, oct(1700000000, 12));
    h[156] = e.dir ? 0x35 : 0x30;
    field(h, 257, 8, 'ustar\x0000');
    field(h, 265, 32, 'root');
    field(h, 297, 32, 'root');
    h.fill(0x20, 148, 156);
    field(h, 148, 8, `${oct(h.reduce((a, b) => a + b, 0), 7)}\0`);
    out.push(h, body, new Uint8Array((512 - (body.length % 512)) % 512));
  }
  out.push(new Uint8Array(1024));
  return Buffer.concat(out);
}

/** An `ar` archive, as a .deb is. */
function ar(members) {
  const parts = [Buffer.from('!<arch>\n')];
  for (const [name, data] of members) {
    const hdr = `${name}/`.padEnd(16) + '0'.padEnd(12) + '0'.padEnd(6) + '0'.padEnd(6) + '100644'.padEnd(8) + String(data.length).padEnd(10) + '`\n';
    parts.push(Buffer.from(hdr), data);
    if (data.length % 2) parts.push(Buffer.from('\n'));
  }
  return Buffer.concat(parts);
}

/** The stand-in repository, under `root`: dists/local and one package. */
function makeRepo(root) {
  const control = [
    `Package: ${PKG}`, 'Version: 1.0', 'Architecture: all', 'Maintainer: gex tests <tests@gex.invalid>',
    'Installed-Size: 1', 'Section: misc', 'Priority: optional', 'Description: says hello, for a test of apt under real permissions', '',
  ].join('\n');
  const deb = ar([
    ['debian-binary', Buffer.from('2.0\n')],
    ['control.tar.gz', gzipSync(tar([{ name: './', mode: 0o755, dir: true }, { name: './control', mode: 0o644, body: control }]))],
    ['data.tar.gz', gzipSync(tar([
      { name: './', mode: 0o755, dir: true }, { name: './usr/', mode: 0o755, dir: true }, { name: './usr/bin/', mode: 0o755, dir: true },
      { name: `./usr/bin/${PKG}`, mode: 0o755, body: '#!/bin/sh\necho "hello from $(id -un)"\n' },
    ]))],
  ]);
  const pool = `pool/main/${PKG}_1.0_all.deb`;
  mkdirSync(join(root, 'pool/main'), { recursive: true });
  writeFileSync(join(root, pool), deb);
  const sha = (b) => createHash('sha256').update(b).digest('hex');
  const md5 = (b) => createHash('md5').update(b).digest('hex');
  const packages = Buffer.from(`${control.trimEnd()}\nFilename: ${pool}\nSize: ${deb.length}\nSHA256: ${sha(deb)}\nMD5sum: ${md5(deb)}\n\n`);
  const lines = [];
  for (const arch of ['musl-linux-wasm32', 'arm64', 'all']) {
    const dir = `main/binary-${arch}`;
    mkdirSync(join(root, 'dists/local', dir), { recursive: true });
    writeFileSync(join(root, 'dists/local', dir, 'Packages'), packages);
    lines.push(` ${sha(packages)} ${packages.length} ${dir}/Packages`);
  }
  writeFileSync(join(root, 'dists/local/Release'), [
    'Origin: gex-tests', 'Label: gex-tests', 'Suite: local', 'Codename: local', `Date: ${new Date().toUTCString()}`,
    'Architectures: musl-linux-wasm32 arm64 all', 'Components: main', 'SHA256:', ...lines, '',
  ].join('\n'));
}

function serve(root, log) {
  const server = createServer((req, res) => {
    const path = decodeURIComponent(new URL(req.url, 'http://x').pathname);
    const file = join(root, normalize(path).replace(/^(\.\.[/\\])+/, ''));
    let st;
    try { st = statSync(file); } catch { st = null; }
    if (!st?.isFile()) { log.push(`404 ${path}`); res.writeHead(404).end(); return; }
    log.push(`200 ${path}`);
    res.writeHead(200, { 'content-length': st.size, 'last-modified': st.mtime.toUTCString() });
    createReadStream(file).pipe(res);
  });
  return new Promise((r) => server.listen(0, '127.0.0.1', () => r(server)));
}

export default async function (t) {
  const { say, check, eq, timeout } = t;
  const repo = join(t.workDir, `owners-repo-${process.pid}`);
  makeRepo(repo);
  const log = [];
  const server = await serve(repo, log);
  try {
    const sys = new System({ baseUrl: t.baseUrl, debug: t.debug() });
    const fs = new MemoryFilesystem(sys);
    fs.addTar(t.work('slim/debian-slim.tar.zst'), { owners: true });
    sys.network = new RepoNetwork({ base: `http://127.0.0.1:${server.address().port}/`, fallback: null });
    await timeout(sys.boot(), 180000, 'boot');
    const env = { PATH: '/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin', HOME: '/root', DEBIAN_FRONTEND: 'noninteractive' };
    const run = async (script, ms = 300000) => {
      const t0 = performance.now();
      const r = await timeout(sys.run(['/bin/sh', '-c', script], { env, cwd: '/' }), ms, script);
      const out = `${r.stdout}${r.stderr}`.trim();
      say(`  $ ${script.slice(0, 120)}: ${r.code} in ${(performance.now() - t0).toFixed(0)} ms: ${out.split('\n').slice(-4).join(' / ').slice(0, 400)}`);
      return { ...r, out };
    };
    const as = (script) => run(`setpriv --reuid=1001 --regid=1001 --init-groups env HOME=/home/bob USER=bob sh -c 'cd && ${script}'`);
    eq('/etc/gex/boot', await timeout(sys.spawn(['/etc/gex/boot'], { env, detached: true }).exited, 60000, '/etc/gex/boot'), { code: 0, signal: null });

    // The image's owners, believed: its user's home is 1000's, the rest root's.
    eq('the archive\'s owners', (await run('stat -c "%u:%g %U %n" /home/user /home/user/.bashrc /usr/bin /etc/shadow')).stdout,
      '1000:1000 user /home/user\n1000:1000 user /home/user/.bashrc\n0:0 root /usr/bin\n0:0 root /etc/shadow\n');
    eq('the page runs as root', (await run('id -u')).stdout, '0\n');

    // bob, as useradd makes him.
    const add = await run([
      'echo "bob:x:1001:1001:Bob:/home/bob:/bin/bash" >> /etc/passwd',
      'echo "bob:!:20000:0:99999:7:::" >> /etc/shadow',
      'echo "bob:x:1001:" >> /etc/group',
      'mkdir /home/bob && cp -a /etc/skel/. /home/bob/ && chmod 750 /home/bob && chown -R bob:bob /home/bob',
      'stat -c "%U:%G %a" /home/bob',
    ].join(' && '));
    eq('bob is made, his home his', add.stdout, 'bob:bob 750\n');
    eq('bob is bob', (await as('id -un; id -G; pwd')).stdout, 'bob\n1001\n/home/bob\n');
    const su = await run('su bob -c "id -un" 2>&1');
    t.known('su bob', su.code === 0 && su.stdout === 'bob\n', su.out, 'the image\'s su and runuser end at once ("Critical error - immediate abort"), with or without owners');
    const usr = await as('touch /usr/bob-was-here');
    check('bob may not write /usr', usr.code !== 0 && /Permission denied/.test(usr.out), usr.out);
    const bin = await as('cp /usr/bin/id /usr/bin/true');
    check('nor replace a program of root\'s', bin.code !== 0 && /Permission denied/.test(bin.out), bin.out);
    const shadow = await as('cat /etc/shadow');
    check('bob may not read /etc/shadow', shadow.code !== 0 && /Permission denied/.test(shadow.out) && !/root:/.test(shadow.out), shadow.out);
    eq('his home is his to write', (await as('echo mine > ~/note && stat -c "%U:%G" ~/note && cat ~/note')).stdout, 'bob:bob\nmine\n');
    const theirs = await as('echo x > /home/user/bob-was-here');
    check('and another user\'s is not', theirs.code !== 0 && /Permission denied/.test(theirs.out), theirs.out);

    // apt, from the stand-in repository only: as the image has it (its
    // methods as root), then as Debian has it (as _apt).
    await run('rm -f /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources /etc/apt/sources.list && echo "deb [trusted=yes] http://gex-repo/ local main" > /etc/apt/sources.list.d/local.list');
    eq('the image\'s apt: apt-get update', (await run('apt-get update 2>&1')).code, 0);
    check('apt-get update fetched the repository\'s lists', log.some((l) => l === '200 /dists/local/Release') && log.some((l) => /^200 \/dists\/local\/main\/binary-.*\/Packages$/.test(l)), log.join(', '));
    const SB = '-o APT::Sandbox::User=_apt';
    // (Afresh, as _apt: the lists it fetched as root gone.)
    await run('rm -rf /var/lib/apt/lists/*');
    log.length = 0;
    const update = await run(`apt-get ${SB} update 2>&1`);
    eq('apt-get update, as _apt', update.code, 0);
    check('and fetched them again, as _apt', log.some((l) => l === '200 /dists/local/Release') && log.some((l) => /^200 \/dists\/local\/main\/binary-.*\/Packages$/.test(l)), log.join(', '));
    check('as _apt, sandboxed (no "unsandboxed" warning)', !/unsandboxed/i.test(update.out) && !/Permission denied/.test(update.out), update.out);
    eq('its partial directory is _apt\'s, as apt made it', (await run('stat -c "%U %a" /var/lib/apt/lists/partial')).stdout, '_apt 700\n');
    const install = await run(`apt-get ${SB} install -y ${PKG} 2>&1`);
    eq(`apt-get install ${PKG}, as _apt`, install.code, 0);
    check(`${PKG} came from the repository`, log.some((l) => l.startsWith(`200 /pool/main/${PKG}_`)), log.join(', '));
    check('and was downloaded as _apt too', !/unsandboxed/i.test(install.out) && !/Permission denied/.test(install.out), install.out);
    eq('the archives\' partial directory is _apt\'s', (await run('stat -c "%U %a" /var/cache/apt/archives/partial')).stdout, '_apt 700\n');
    eq(`${PKG} is installed`, (await run(`dpkg-query -W -f '\${Status}' ${PKG}`)).stdout, 'install ok installed');
    eq(`${PKG} is root's, as dpkg laid it`, (await run(`stat -c "%U %a" /usr/bin/${PKG}`)).stdout, 'root 755\n');
    eq('bob runs it', (await as(PKG)).stdout, 'hello from bob\n');
    const rm = await as(`rm -f /usr/bin/${PKG}`);
    check('and may not remove it', rm.code !== 0 && /Permission denied/.test(rm.out), rm.out);
    // _apt may not reach what is root's: the sandbox is real.
    const apt = await run('setpriv --reuid=42 --regid=65534 --clear-groups sh -c "touch /var/lib/apt/lists/x; touch /var/lib/apt/lists/partial/x && echo partial-ok" 2>&1');
    check('_apt may write its partial directory and not the lists beside it', /Permission denied/.test(apt.out) && /partial-ok/.test(apt.out), apt.out);
    eq('and the image\'s apt again, after: apt-get install --reinstall', (await run(`apt-get install -y --reinstall ${PKG} 2>&1`)).code, 0);
    await sys.shutdown();
  } finally {
    server.close();
  }
}
