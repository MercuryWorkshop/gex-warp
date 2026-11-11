// A Mac program's own path (fixtures/darwin/exepath.c): the `apple`
// vector's executable_path (what dyld's _NSGetExecutablePath and so a main
// bundle are found from), proc_pidpath and KERN_PROCARGS2's exec path, with
// argv[0] apart from them. On the Mac, a program exec'd by a path and named
// another is told the path three times and the name once; the program it
// becomes by exec likewise.
//
// gexc given a program by its host path, when that file is inside the
// sysroot or a bound directory, names it by its guest path, whatever
// `--argv0` says (an application bundle bound at /Applications finds
// itself there); `--guest` names it by the guest path directly. The page's
// runtime under Node likewise for a program a `--dir` put in the guest.
// And a Linux program's /proc/self/exe and AT_EXECFN the same way. Both
// processors.
import { chmodSync, copyFileSync, existsSync, mkdirSync, realpathSync } from 'node:fs';
import { join } from 'node:path';
import { darwin } from '../lib/darwin.mjs';
import { ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

const told = (who, argv0, path, argc) =>
  [`argv[0] ${argv0}`, `executable_path ${path}`, `proc_pidpath ${path}`, `procargs2 ${path}, argc ${argc}, argv[0] ${argv0}`].map((l) => `${who}: ${l}\n`).join('');

export default async function (t) {
  const d = darwin(t);
  const x86 = t.arch === 'x86';
  const prog = await d.build('exepath');

  // The Mac: told what it was exec'd as, and its name apart.
  const mac = realpathSync(prog.mac);
  const want = await d.mac({ ...prog, mac }, ['exec', mac, 'other']);
  const macWant = told('first', mac, mac, 4) + told('second', 'other', mac, 2);
  t.check('on the Mac: the exec\'d path three ways, argv[0] apart', want.stdout === macWant && want.code === 0, `exit ${want.code}: ${JSON.stringify(want.stdout)}`);

  // A root with the program at /bin/exepath, and a bundle's MacOS
  // directory elsewhere on the host, bound at /Applications.
  const tree = d.n.dir('exepath-root');
  mkdirSync(join(tree, 'bin'), { recursive: true });
  copyFileSync(prog.gex, join(tree, 'bin/exepath'));
  chmodSync(join(tree, 'bin/exepath'), 0o755);
  const bundle = d.n.dir('exepath-bundle');
  copyFileSync(prog.gex, join(bundle, 'Some'));
  chmodSync(join(bundle, 'Some'), 0o755);
  const APP = '/Applications/Some.app/Contents/MacOS';
  const bind = ['--sysroot', tree, '--bind', `${bundle}=${APP}`];

  const runs = [
    { label: 'in the root, named apart', exe: join(tree, 'bin/exepath'), gexc: ['--sysroot', tree, '--argv0', 'name'], args: [], want: told('first', 'name', '/bin/exepath', 1) },
    {
      label: 'in a bound directory, named apart, exec\'d again',
      exe: join(bundle, 'Some'),
      gexc: [...bind, '--argv0', 'name'],
      args: ['exec', `${APP}/Some`, 'other'],
      want: told('first', 'name', `${APP}/Some`, 4) + told('second', 'other', `${APP}/Some`, 2),
    },
    { label: '--guest, by its guest path', exe: `${APP}/Some`, gexc: [...bind, '--guest'], args: [], want: told('first', `${APP}/Some`, `${APP}/Some`, 1) },
  ];
  const differ = (a, b) => {
    const x = a.split('\n');
    const y = b.split('\n');
    const i = x.findIndex((l, n) => l !== y[n]);
    return i < 0 ? `${y.length} lines, not ${x.length}` : `line ${i + 1}: want ${JSON.stringify(x[i])}, got ${JSON.stringify(y[i] ?? null)}`;
  };
  for (const r of runs) {
    const got = await d.gex({ name: 'exepath', gex: r.exe }, r.args, { gexc: r.gexc });
    t.check(`${r.label}: gexc`, got.stdout === r.want && got.code === 0, `exit ${got.code}: ${differ(r.want, got.stdout)} / ${got.stderr.trim().split('\n').slice(-3).join(' / ')}`);
  }

  // A Linux program, the same way: /proc/self/exe and AT_EXECFN.
  const lx = join(bundle, 'lx');
  await d.n.cc(['-static', '-O1', '-o', lx, join(ROOT, 'test/fixtures/darwin/exepath-linux.c')]);
  const linux = (argv0) => `argv[0] ${argv0}\n/proc/self/exe ${APP}/lx\nAT_EXECFN ${APP}/lx\n`;
  for (const [label, gexc, exe, argv0] of [
    ['a Linux program in a bound directory, named apart', ['--bind', `${bundle}=${APP}`, '--argv0', 'name'], lx, 'name'],
    ['a Linux program by its guest path', ['--bind', `${bundle}=${APP}`, '--guest'], `${APP}/lx`, `${APP}/lx`],
  ]) {
    const got = await d.n.gexc(['run', ...gexc, exe], { timeout: 120000 });
    t.check(`${label}: gexc`, got.stdout === linux(argv0) && got.code === 0, `exit ${got.code}: ${JSON.stringify(got.stdout)} / ${got.stderr.trim().split('\n').slice(-2).join(' / ')}`);
  }

  // The page's runtime: the root and the bundle as `--dir`s.
  const seed = join(ROOT, `page/public/gex/seed${x86 ? '.x86' : ''}.shared.wasm`);
  if (!existsSync(seed)) {
    t.say(`(no ${seed}: no page leg)`);
    return;
  }
  for (const r of runs.slice(0, 2)) {
    const page = await d.n.g.run(seed, r.exe, r.args, { argv0: 'name', env: { GEX_SHARED: '1' }, flags: ['--dir', `/=${tree}`, '--dir', `${APP}=${bundle}`, '--cwd', '/'], timeout: 240000 });
    const out = page.stdout.split('\n').filter((l) => !l.startsWith('[')).join('\n');
    t.check(`${r.label}: on the page runtime`, out === r.want && page.code === 0, `exit ${page.code}: ${differ(r.want, out)} / ${page.stderr.trim().split('\n').slice(-3).join(' / ')}`);
  }
}
