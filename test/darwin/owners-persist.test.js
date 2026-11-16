// Owners kept across loads in an OPFSFilesystem's store
// (gex_kernel::vfs::tmpfs::persist), in a Mac root laid with its owners
// (darwin/owners-page's): a user root made, a file she made, a file root
// gave her (a chown and nothing else, on one of the archive's files and on
// one root made), and after a reload she is still the owner of each and may
// still write them, while the archive's own files are root's as before.
//   1. a fresh store: alice made, her file, the chowns; synced, and the
//      page reloads without a shutdown;
//   2. each is hers, she writes each, and /usr/bin is still not hers.
import { System } from '../../page/runtime/src/index.js';
import { OPFSFilesystem } from '../../page/runtime/src/fs.js';

export const meta = { hosts: ['browser'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 600 };

export default async function (t) {
  const { say, check, eq, timeout } = t;
  const STORE = `gex-test-owners-${t.arch}`;
  if (t.phase === 1) await OPFSFilesystem.erase(STORE);
  const sys = new System({ baseUrl: t.baseUrl, debug: t.debug() });
  const fs = new OPFSFilesystem(sys, { name: STORE, writeback: 600000 });
  try {
    fs.addTar(t.work(`darwin-root.${t.arch}.tar`), { owners: true });
    await timeout(sys.boot(), 180000, 'boot');
  } catch (e) {
    t.skip(`no Mac root for ${t.arch} (GEX_TEST_DARWIN_ROOT: pyrus builds one): ${e?.message ?? e}`);
  }
  const env = { PATH: '/usr/bin:/bin:/usr/sbin:/sbin', HOME: '/var/root', TERM: 'dumb' };
  const run = async (script, what = script) => {
    const r = await timeout(sys.run(['/bin/sh', '-c', script], { env, cwd: '/' }), 240000, what);
    const out = `${r.stdout}${r.stderr}`.trim();
    say(`  $ ${what}: ${r.code}: ${out.split('\n').slice(-3).join(' / ').slice(0, 300)}`);
    return { ...r, out };
  };
  const as = (script) => run(`su - alice -c '${script}'`, `as alice: ${script}`);
  const owners = 'stat -f "%Su:%Sg %N" /Users/alice /Users/alice/note /Users/Shared/given /private/etc/motd-alice /usr/bin';

  if (t.phase === 1) {
    eq('sysadminctl -addUser alice', (await run('sysadminctl -addUser alice -fullName Alice -password secret -admin')).code, 0);
    eq('her file', (await as('echo one > ~/note && echo ok')).stdout, 'ok\n');
    // Root's files given to her: one root makes, one of the archive's.
    eq('root gives her a file', (await run('echo given > /Users/Shared/given && chown alice:staff /Users/Shared/given && chmod 600 /Users/Shared/given && echo ok')).stdout, 'ok\n');
    eq('and the archive\'s, a chown alone', (await run('chown alice /private/etc/shells && echo ok')).stdout, 'ok\n');
    await run('echo motd > /private/etc/motd-alice && chown alice:staff /private/etc/motd-alice && chmod 644 /private/etc/motd-alice');
    const before = (await run(owners)).stdout;
    eq('the owners before the reload', before, 'alice:staff /Users/alice\nalice:staff /Users/alice/note\nalice:staff /Users/Shared/given\nalice:staff /private/etc/motd-alice\nroot:wheel /usr/bin\n');
    eq('/etc/shells, the archive\'s, now hers', (await run('stat -f "%Su:%Sg" /private/etc/shells')).stdout, 'alice:wheel\n');
    await sys.sync();
    // Killed: no shutdown, no last sync.
    return t.reload({ before });
  }

  eq('after a reload: the owners are kept', (await run(owners)).stdout, t.state.before);
  eq('the archive\'s file she was given is still hers', (await run('stat -f "%Su:%Sg" /private/etc/shells')).stdout, 'alice:wheel\n');
  eq('and the runtime says so', (({ uid }) => uid)(await fs.stat('/private/etc/shells')) > 500, true);
  eq('her account is still there', (await run('dscl . -read /Users/alice NFSHomeDirectory')).stdout, 'NFSHomeDirectory: /Users/alice\n');
  eq('she writes her own file', (await as('echo two >> ~/note && cat ~/note')).stdout, 'one\ntwo\n');
  eq('and the one root gave her', (await as('echo more >> /Users/Shared/given && cat /Users/Shared/given')).stdout, 'given\nmore\n');
  eq('and the archive\'s she was given', (await as('cat /private/etc/shells >/dev/null && echo >> /private/etc/shells && echo ok')).stdout, 'ok\n');
  const bin = await as('touch /usr/bin/alice-was-here');
  check('and still may not write /usr/bin', bin.code !== 0 && /Permission denied/.test(bin.out), bin.out);
  const sudo = await as('echo secret | sudo -S -p "" id -u');
  eq('and sudo is still root\'s', sudo.stdout, '0\n');
  await sys.shutdown();
  await fs.erase();
}
