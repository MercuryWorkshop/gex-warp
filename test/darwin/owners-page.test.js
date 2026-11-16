// Who owns what in a Mac root on the page, and a user held to it. The root
// (the `darwin-root` image) is packed with the owners it means its files to
// have (scripts/pack-root.py: pyrus's `user.gex.owner`, root:wheel
// elsewhere), and laid with `addTar(..., { owners: true })`, so the page's
// tree has them (hosts/kernel pagetar.rs) and a process reads them through
// the remote filesystem. The page's programs start as root, the first
// user, whose permissions are not gex's to check; root makes a user with
// sysadminctl, and `su - alice` then is held to every file's owner and
// mode: /usr/bin is root's and not hers to write, a 0600 file of root's not
// hers to read, her home hers. And sudo is a set-user-ID program of root's
// by the archive's owner (not by the rule that a file nobody owns is the
// first user's), which gives her root.
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';

export const meta = { hosts: ['browser', 'node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 600 };

export default async function (t) {
  const { say, check, eq, timeout } = t;
  const sys = new System({ baseUrl: t.baseUrl, debug: t.debug() });
  const fs = new MemoryFilesystem(sys);
  try {
    fs.addTar(t.work(`darwin-root.${t.arch}.tar`), { owners: true });
    await timeout(sys.boot(), 120000, 'boot');
  } catch (e) {
    t.skip(`no Mac root for ${t.arch} (GEX_TEST_DARWIN_ROOT: pyrus builds one): ${e?.message ?? e}`);
  }
  const env = { PATH: '/usr/bin:/bin:/usr/sbin:/sbin', HOME: '/var/root', TERM: 'dumb' };
  const run = async (script, what = script) => {
    const r = await timeout(sys.run(['/bin/sh', '-c', script], { env, cwd: '/' }), 240000, what);
    const out = `${r.stdout}${r.stderr}`.trim();
    say(`  $ ${what}: ${r.code}${r.signal ? ` ${r.signal}` : ''}: ${out.split('\n').slice(-3).join(' / ').slice(0, 300)}`);
    return { ...r, out };
  };
  const as = (script) => run(`su - alice -c '${script}'`, `as alice: ${script}`);

  // The archive's owners, as the page's runtime and a program see them.
  eq('the runtime sees /Users as the archive gives it (root:admin)', (({ uid, gid }) => [uid, gid])(await fs.stat('/Users')), [0, 80]);
  eq('and a program', (await run('stat -f "%Su:%Sg %Sp" /Users /usr/bin/sudo /usr/bin')).stdout,
    'root:admin drwxr-xr-x\nroot:wheel -r-s--x--x\nroot:wheel drwxr-xr-x\n');
  eq('the page runs as root', (await run('id -u')).stdout, '0\n');

  // A user, made as an administrator makes one.
  const made = await run('sysadminctl -addUser alice -fullName Alice -password secret -admin');
  eq('sysadminctl -addUser alice', made.code, 0);
  eq('her home is hers', (await run('stat -f "%Su:%Sg" /Users/alice')).stdout, 'alice:staff\n');
  eq('root\'s secret, 0600', (await run('echo root-only > /private/tmp/secret && chmod 600 /private/tmp/secret && stat -f "%Su %Sp" /private/tmp/secret')).stdout, 'root -rw-------\n');

  const who = await as('id -un; id -u');
  check('su - alice is alice, with a uid of her own', /^alice\n5\d\d\n$/.test(who.stdout), who.out);

  const bin = await as('touch /usr/bin/alice-was-here');
  check('alice may not write /usr/bin', bin.code !== 0 && /Permission denied/.test(bin.out), bin.out);
  eq('and nothing is made there', (await run('test -e /usr/bin/alice-was-here && echo made || echo none')).stdout, 'none\n');
  const repl = await as('cp /bin/echo /usr/bin/sudo');
  check('nor replace root\'s sudo', repl.code !== 0 && /Permission denied/.test(repl.out), repl.out);
  const secret = await as('cat /private/tmp/secret');
  check('alice may not read root\'s 0600 file', secret.code !== 0 && /Permission denied/.test(secret.out) && !/root-only/.test(secret.out), secret.out);
  const sudoers = await as('cat /etc/sudoers');
  check('nor /etc/sudoers (root:wheel 0440)', sudoers.code !== 0 && /Permission denied/.test(sudoers.out), sudoers.out);
  const home = await as('echo mine > ~/note && cat ~/note && stat -f "%Su:%Sg" ~/note && mkdir -p ~/Documents/deep && echo ok');
  eq('alice writes her own home', home.stdout, 'mine\nalice:staff\nok\n');

  // sudo: set-user-ID, root's by the archive.
  const sudo = await as('echo secret | sudo -S -p "" id -u');
  eq('sudo id -u gives 0', sudo.stdout, '0\n');
  const wrong = await as('echo wrong | sudo -S -p "" id -u');
  check('and not for a wrong password', wrong.code !== 0 && !/^0$/m.test(wrong.stdout), wrong.out);
  const through = await as('echo secret | sudo -S -p "" sh -c "echo made-by-root > /usr/bin/alice-via-sudo" && stat -f "%Su" /usr/bin/alice-via-sudo');
  eq('what sudo writes in /usr/bin is root\'s', through.stdout, 'root\n');
  await run('rm -f /usr/bin/alice-via-sudo');
  await sys.shutdown();
}
