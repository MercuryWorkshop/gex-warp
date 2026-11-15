// A set-user-ID or set-group-ID program run by an ordinary user, under gexc
// (fixtures/darwin/setuid.c for a Mac program, setuid-linux.c for a Linux
// one; crates/gex-kernel/src/syscalls.rs `exec_ids_of`, `Creds::exec_ids`).
// The Mac cannot be asked: setuid root needs root.
//
// Who owns a file of the host is what gex keeps beside it, the extended
// attribute `user.gex.owner` ("uid:gid"), which gives a file of the user's
// to root without root (the file's own mode, set-user-ID included, is the
// host's). Here: a copy of the program owned by root with mode 4755, one
// owned by root and group 80 with mode 2755, a secret of root's with mode
// 0600, and the program as it came. Run through execve, fork and execve,
// and posix_spawn, the program is told who it is: real user the caller's,
// effective and saved user root (or the group's), issetugid, AT_SECURE for
// a Linux one; it opens what only root may; and as su does, it gives up
// root (setgid, setgroups, setuid to another user): then it may not take it
// back, nor open what is root's, and a program it runs is no set-id one. A
// user the machine's first one is not is held to its permissions.
import { chmodSync, copyFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { join } from 'node:path';
import { darwin } from '../lib/darwin.mjs';
import { ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

export default async function (t) {
  const d = darwin(t);
  const prog = await d.build('setuid');
  const dir = d.n.dir('setuid-files');
  mkdirSync(dir, { recursive: true });
  const U = process.getuid();
  const G = process.getgid();
  // A file of the host that is root's (or a group's) and has set-id bits.
  const make = (name, from, owner, mode) => {
    const f = join(dir, name);
    copyFileSync(from, f);
    chmodSync(f, 0o755);
    if (owner) execFileSync('/usr/bin/xattr', ['-w', 'user.gex.owner', owner, f]);
    chmodSync(f, mode);
    return f;
  };
  const plain = make('plain', prog.gex, null, 0o755);
  const suid = make('suid', prog.gex, '0:0', 0o4755);
  const sgid = make('sgid', prog.gex, `0:80`, 0o2755);
  const secret = join(dir, 'secret');
  writeFileSync(secret, 'root\n');
  execFileSync('/usr/bin/xattr', ['-w', 'user.gex.owner', '0:0', secret]);
  chmodSync(secret, 0o600);

  const run = async (args) => {
    const r = await d.gex({ name: 'setuid', gex: plain }, args);
    return { ...r, lines: r.stdout.split('\n') };
  };
  const has = (r, re, what) => t.check(what, r.lines.some((l) => re.test(l)), `exit ${r.code}: ${JSON.stringify(r.stdout)} / ${r.stderr.trim().split('\n').slice(-2).join(' / ')}`);
  const groupsOf = (r, label) => r.lines.find((l) => l.startsWith(`${label} at the start: getgroups`)) ?? null;

  // The program as it came: the caller's own, whatever the caller is.
  const base = await run(['exec', plain]);
  has(base, new RegExp(`^exec at the start: ruid ${U} euid ${U} svuid ${U} rgid ${G} egid ${G} svgid ${G} issetugid 0$`), 'a program without the bits is its caller\'s');
  const groups = groupsOf(base, 'exec');
  t.check('the host user\'s groups beside its own', groups !== null && groups.includes(': getgroups = '), String(groups));
  const same = (r, label) => t.check(`${label}: the groups are the caller's`, groupsOf(r, label)?.split(': ')[1] === groups?.split(': ')[1], `${groupsOf(r, label)} / ${groups}`);

  for (const [how, label] of [['exec', 'exec'], ['fork', 'fork'], ['spawn', 'spawn']]) {
    const r = await run(how === 'exec' ? ['exec', suid] : [how, suid]);
    has(r, new RegExp(`^${label} at the start: ruid ${U} euid 0 svuid 0 rgid ${G} egid ${G} svgid ${G} issetugid 1$`), `${how}: set-user-ID root: real the caller's, effective and saved root, issetugid`);
    has(r, new RegExp(`^${label}: its file is owned by 0:0, mode 4755 \\(0\\)$`), `${how}: it is root's, with the bit`);
    same(r, label);
    if (how !== 'exec') has(r, /^wait4: its pid, status 0$/, `${how}: and ended well`);
  }
  const g = await run(['exec', sgid]);
  has(g, new RegExp(`^exec at the start: ruid ${U} euid ${U} svuid ${U} rgid ${G} egid 80 svgid 80 issetugid 1$`), 'set-group-ID: the group, effective and saved');
  has(g, /^exec: its file is owned by 0:80, mode 2755 \(0\)$/, 'it is root\'s, group 80, with the bit');

  // Giving up root, as su does (20 and 80 are staff and admin here).
  if (G !== 20) return t.say(`the group ${G} is not staff (20): the drop is not run`);
  const d1 = await run(['exec', suid, 'drop', 'drop', secret, plain]);
  has(d1, new RegExp(`^drop at the start: ruid ${U} euid 0 svuid 0 `), 'root at the start');
  t.check('root opens root\'s secret', d1.lines.find((l) => l.startsWith('open the secret = '))?.match(/^open the secret = (\d+)$/) !== null, JSON.stringify(d1.stdout));
  has(d1, /^setgroups\(2\) = 0$/, 'root sets its groups');
  has(d1, /^drop after setgroups: getgroups = 2: 20 80$/, 'the effective group first, then the other');
  has(d1, /^getauid = 0$/, 'the audit user is asked');
  has(d1, /^audit user 4294967295$/, 'and is none until a login sets it');
  has(d1, /^setauid\(502\) = 0$/, 'root sets it');
  has(d1, /^audit user now 502$/, 'and it stays');
  has(d1, /^__initgroups\(3\) = 0$/, 'root initgroups');
  has(d1, /^drop after initgroups: getgroups = 3: 20 12 61$/, 'the list as given, the effective group first');
  has(d1, /^setauid\(0\) as a user = -1$/, 'a user may not set the audit user');
  has(d1, /^__initgroups as a user = -1$/, 'nor initgroups');
  has(d1, /^setgid\(20\) = 0$/, 'root sets its group');
  has(d1, /^setuid\(502\) = 0$/, 'root becomes another user');
  has(d1, /^drop after giving up root: ruid 502 euid 502 svuid 502 rgid 20 egid 20 svgid 20 issetugid 1$/, 'all of its ids are the user\'s: nothing is left to go back to');
  has(d1, /^drop after giving up root: getgroups = 1: 20$/, 'and its groups are the one');
  has(d1, /^setuid\(0\) = -1$/, 'it may not go back to root');
  has(d1, /^setgid\(0\) = -1$/, 'nor to its group');
  has(d1, /^setgroups\(1\) = -1$/, 'nor set its groups');
  t.check('nor open what is root\'s', d1.lines.filter((l) => l.startsWith('open the secret = ')).at(1) === 'open the secret = -13', JSON.stringify(d1.stdout));
  has(d1, new RegExp(`^after the drop at the start: ruid 502 euid 502 svuid 502 rgid 20 egid 20 svgid 20 issetugid 0$`), 'a program it runs is no tainted one');

  // The Linux personality: the same, and AT_SECURE.
  const lx = (name, from, owner, mode) => make(name, from, owner, mode);
  const src = join(ROOT, 'test/fixtures/darwin/setuid-linux.c');
  const lbin = join(dir, 'linux-bin');
  await d.n.cc(['-static', '-O1', '-o', lbin, src]);
  const lplain = lx('lplain', lbin, null, 0o755);
  const lsuid = lx('lsuid', lbin, '0:0', 0o4755);
  const lrun = async (args) => {
    const r = await d.n.gexc(['run', lplain, ...args], { timeout: 120000 });
    return { ...r, lines: r.stdout.split('\n') };
  };
  const lbase = await lrun(['exec', lplain]);
  has(lbase, new RegExp(`^exec at the start: ruid ${U} euid ${U} suid ${U} rgid ${G} egid ${G} sgid ${G} AT_SECURE 0$`), 'Linux: a program without the bits is its caller\'s');
  const lr = await lrun(['exec', lsuid]);
  has(lr, new RegExp(`^exec at the start: ruid ${U} euid 0 suid 0 rgid ${G} egid ${G} sgid ${G} AT_SECURE 1$`), 'Linux: set-user-ID root, effective and saved root, AT_SECURE');
  has(lr, /^exec: its file is owned by 0:0, mode 4755 \(0\)$/, 'Linux: it is root\'s, with the bit');
  const ld = await lrun(['exec', lsuid, 'drop', 'drop', secret, lplain]);
  t.check('Linux: root opens root\'s secret', ld.lines.find((l) => l.startsWith('open the secret = '))?.match(/^open the secret = (\d+)$/) !== null, JSON.stringify(ld.stdout));
  has(ld, /^drop after setgroups: getgroups = 2: 20 80$/, 'Linux: its groups, sorted, as set');
  has(ld, /^drop after giving up root: ruid 502 euid 502 suid 502 rgid 20 egid 20 sgid 20 AT_SECURE 1$/, 'Linux: all its ids are the user\'s');
  has(ld, /^setuid\(0\) = -1$/, 'Linux: it may not go back to root');
  has(ld, /^setgroups\(1\) = -1$/, 'Linux: nor set its groups');
  t.check('Linux: nor open what is root\'s', ld.lines.filter((l) => l.startsWith('open the secret = ')).at(1) === 'open the secret = -13', JSON.stringify(ld.stdout));
  has(ld, /^after the drop at the start: ruid 502 euid 502 suid 502 rgid 20 egid 20 sgid 20 AT_SECURE 0$/, 'Linux: a program it runs is not a secure one');
}
