// A Mac root on the page whose names are looked up without regard to case,
// as a Mac's default volume's are: the root's own word (bottle.conf's
// `filesystem.case`), taken by each Mac program as it starts. Until it is
// said the root tells case apart, as it always has; said, the other
// spelling of a name is the name (a lookup, a create refused, a directory
// made over it refused), the name keeps the spelling it was made with, a
// rename that changes only case changes it, and a mount point is found
// however it is spelled (/DEV/null); said otherwise, case is told apart
// again.
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';

export const meta = { hosts: ['browser', 'node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 600 };

const CONF = '/Library/Preferences/bottle.conf';

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
  // (The setting, in place of any the root has: read by the next program.)
  const say_case = (v) => run(`mkdir -p /Library/Preferences && { grep -v '^filesystem.case' ${CONF} 2>/dev/null; echo 'filesystem.case = ${v}'; } > /private/tmp/conf && cp /private/tmp/conf ${CONF}`, `filesystem.case = ${v}`);

  eq('case is told apart until the root says', (await run('test -e /USR/BIN/SW_VERS && echo same || echo apart')).stdout, 'apart\n');

  eq('the root says insensitive', (await say_case('insensitive')).code, 0);
  eq('the other spelling of a name is the name', (await run('test -x /USR/BIN/SW_VERS && /usr/bin/STAT -f %N /Usr/Bin/Sw_Vers')).stdout, '/Usr/Bin/Sw_Vers\n');
  eq('a directory made', (await run('mkdir /var/root/Made && echo made')).stdout, 'made\n');
  const again = await run('mkdir /var/root/MADE');
  check('is there by its other spelling (mkdir refused)', again.code !== 0 && /File exists/.test(again.out), again.out);
  const excl = await run('set -C; echo x > /var/root/made/F && echo y > /var/root/MADE/f');
  check('and a create of a file\'s other spelling is refused', excl.code !== 0 && /exist/i.test(excl.out), excl.out);
  eq('a name keeps the spelling it was made with', (await run('ls /var/root/made')).stdout, 'F\n');
  eq('a rename that changes only case changes it', (await run('mv /var/root/made /var/root/MADE && ls /var/root | grep -i "^made$"')).stdout, 'MADE\n');
  // (The mount's own names are its filesystem's to fold or not: a Mac's
  // devfs tells case apart, as the page's does.)
  eq('a mount point is found however it is spelled', (await run('test -c /DEV/null && echo there; test -e /dev/NULL || echo apart')).stdout, 'there\napart\n');

  eq('the root says sensitive', (await say_case('sensitive')).code, 0);
  eq('and case is told apart again', (await run('test -e /USR/BIN/SW_VERS && echo same || echo apart; ls -d /var/root/MADE')).stdout, 'apart\n/var/root/MADE\n');
  await run('rm -rf /var/root/MADE');
  await sys.shutdown();
}
