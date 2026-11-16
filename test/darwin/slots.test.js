// What a program costs of the page's dispatch slots, and what the page does
// when they are gone. Every guest code page translated at run time takes
// its slots from one pool the whole page shares (hosts/browser
// `slot_space_top`; half of it is the runtime's), and none is given back: a
// session of a few hundred processes lasts only while a program run a
// second time, and a library another program has already run, are given
// the slots they were given before. The real Homebrew installer in a Mac
// root (git, curl, Ruby, a few hundred processes) spent all of it: each
// process numbered its libraries' pages afresh.
//
// In a Mac root (pyrus's, the `darwin-root` image): the shell, a pipeline
// of the usual tools, curl and git, each run several times from the page,
// and a shell's loop of them (a fork and an exec each), with no network.
// * A program run again draws next to nothing (its pages are numbered by
//   what their code is: `shared_page_table`'s identity).
// * A program run for the first time draws its own code and little more:
//   the libraries it shares with the programs before it are where they
//   were in those, at the bases they were given there (the kernel's
//   `darwin_library_place`).
// * With the pool all but spent (its mark put a few pages short of the top,
//   as a run that drew the rest leaves it), a program whose code has all
//   been run goes on running, and one that needs new code past the top
//   ends, killed, the runtime saying why. (Every program started after
//   that, `sh` and `ls` among them, used to end in a panic of the runtime,
//   hosts/browser dense.rs: "the len is 4 but the index is 4".) No panic,
//   and no page left numbered with its directory entry unwritten (a `heal`
//   line, which the run that spent the pool also showed).
//
// `log: 'slots'` is what says where the slots went: each program's line as
// it ends (worker.mjs, the shim's `slot_report`), shown when a limit fails.
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { internals } from '../../page/runtime/src/internal.js';

export const meta = { hosts: ['browser', 'node'], arches: ['a64'], images: ['darwin-root'], timeout: 900 };

// Slots a page of guest code takes (gex_abi::SLOTS_PER_PAGE, AArch64), and
// the first slot no table holds (`slot_space_top`: eight tables of V8's ten
// million, in whole pages).
const PER_PAGE = 1024;
const TOP = 8 * Math.floor(10_000_000 / PER_PAGE) * PER_PAGE;

export default async function (t) {
  const { say, check, eq, timeout } = t;
  const sys = new System({ baseUrl: t.baseUrl, debug: t.debug({ log: 'slots' }) });
  const fs = new MemoryFilesystem(sys);
  try {
    fs.addTar(t.work(`darwin-root.${t.arch}.tar`));
    await timeout(sys.boot(), 120000, 'boot');
  } catch (e) {
    t.skip(`no Mac root for ${t.arch} (GEX_TEST_DARWIN_ROOT: pyrus builds one): ${e?.message ?? e}`);
  }
  const k = internals(sys, 'slots').kernel();
  const left = async () => Number(await k.r('k_slot_left'));
  const env = { PATH: '/usr/bin:/bin:/usr/sbin:/sbin', HOME: '/tmp', TERM: 'dumb' };
  const run = async (argv, what) => {
    const r = await timeout(sys.run(argv, { env, cwd: '/tmp' }), 240000, what);
    if (r.code !== 0) say(`  ${what}: status ${r.code} ${r.signal ?? ''} ${r.stderr.trim().split('\n').slice(-2).join(' / ')}`);
    return r;
  };
  const start = await left();
  check('the pool is there to count', start > 0 && start <= TOP / 2, `${start}`);

  // Each program from the page, RUNS times: the pages of slots each run drew.
  const RUNS = 5;
  const programs = [
    ['sh', ['/bin/sh', '-c', 'true']],
    ['bash', ['/bin/bash', '-c', 'echo $((6*7))']],
    ['ls', ['/bin/ls', '/usr/lib']],
    ['pipeline', ['/bin/sh', '-c', 'ls /usr/bin | grep -c . | cat']],
    ['curl', ['/usr/bin/curl', '--version']],
    ['git init', ['/bin/sh', '-c', 'rm -rf /tmp/r && git init -q /tmp/r']],
    ['git status', ['/usr/bin/git', '-C', '/tmp/r', 'status', '--short']],
  ];
  const first = {}, again = {};
  for (const [name, argv] of programs) {
    const drew = [];
    let ok = true;
    for (let i = 0; i < RUNS; i++) {
      const before = await left();
      const r = await run(argv, `${name}, run ${i + 1}`);
      ok &&= r.code === 0;
      drew.push((before - await left()) / PER_PAGE);
    }
    check(`${name} runs`, ok);
    first[name] = drew[0];
    // The most any run after the first drew.
    again[name] = Math.max(...drew.slice(1));
    say(`  ${name}: pages of slots per run ${drew.join(', ')}`);
    t.record(`first ${name}`, first[name]);
    t.record(`again ${name}`, again[name]);
  }

  // And from a shell's loop, as a script runs its commands: a fork and an
  // exec each. The difference between a long loop and a short one, a round.
  const loop = async (n) => {
    const before = await left();
    const r = await run(['/bin/bash', '-c', `for i in $(seq ${n}); do /bin/ls /usr/lib >/dev/null; git -C /tmp/r status --short; x=$(/usr/bin/basename /a/b); done; echo "$x"`], `a loop of ${n}`);
    check(`a loop of ${n} runs`, r.code === 0 && r.stdout.trim() === 'b', `${r.code} ${JSON.stringify(r.stdout.slice(-80))} ${r.stderr.slice(-200)}`);
    return (before - await left()) / PER_PAGE;
  };
  const warm = await loop(2);
  const short = await loop(2);
  const long = await loop(12);
  // Per round: three programs exec'd, and the forks they came of.
  const round = (long - short) / 10;
  say(`  a loop: ${warm} pages the first two rounds, ${short} the next two, ${long} for twelve: ${round.toFixed(1)} a round`);
  t.record('a round of the loop', round);
  const spent = (start - await left()) / PER_PAGE;
  say(`  all of it drew ${spent} pages of slots, of ${Math.round(start / PER_PAGE)}`);
  t.record('pages drawn', spent);

  // A run after the first: a chunk of the pool at most (a program draws
  // eight pages at a time and leaves what it does not use). It was 80 to
  // 970 pages a run, every run, and 990 a round of the loop.
  const AGAIN = 8;
  for (const [name] of programs) check(`${name} run again draws at most ${AGAIN} pages of slots`, again[name] <= AGAIN, `${again[name]}`);
  check(`a round of a shell's loop draws at most ${AGAIN} pages`, round <= AGAIN, `${round}`);
  // A first run: the program's own code, and what of the libraries no
  // program before it ran. `ls` after the shell has run is the plain case
  // (a few pages of its own; it was 232, libSystem's again at another
  // address), git the largest program here (about 230; it was 1080).
  check('ls, first run after the shell, draws its own code and not the libraries again (at most 48 pages)', first.ls <= 48, `${first.ls}`);
  check('git, first run, draws at most 400 pages', first['git init'] <= 400, `${first['git init']}`);
  // All of it: under a twentieth of the pool (it was 32,300 pages of 39,060).
  const ALL = 2000;
  const fits = spent <= ALL;
  check(`all of it draws at most ${ALL} pages of slots`, fits, `${spent}`);
  if (!fits || Object.values(again).some((n) => n > AGAIN) || round > AGAIN) {
    say((await t.logs(/\[gex\] slots /)).map((l) => `  | ${l.slice(0, 1200)}`).join('\n'));
  }

  // The pool all but spent: its mark put a few pages short of its top, as a
  // run that drew the rest leaves it. Then programs nothing here has run,
  // one after another: each needs slots for its own code, so one may still
  // fit, and from the one whose code crosses the top on, each ends, killed.
  const LEFT = 12;
  await k.r('k_slot_floor', BigInt(TOP - LEFT * PER_PAGE));
  eq('the pool is all but spent', await left(), LEFT * PER_PAGE);
  const known = await run(['/bin/ls', '/usr/lib'], 'a program run before, the pool all but spent');
  check('a program whose code has been run still runs', known.code === 0 && /libSystem/.test(known.stdout), `${known.code} ${known.signal}`);
  eq('and draws nothing', await left(), LEFT * PER_PAGE);
  const novel = [
    ['sort', ['/usr/bin/sort', '/etc/passwd'], /root/],
    ['sed', ['/usr/bin/sed', '-n', '1p', '/etc/passwd'], /./],
    ['awk', ['/usr/bin/awk', 'BEGIN { print 6 * 7 }'], /^42$/m],
    ['od', ['/usr/bin/od', '-c', '/etc/passwd'], /^0000000/m],
    ['wc', ['/usr/bin/wc', '-l', '/etc/passwd'], /\d/],
    // (Again: what the first try numbered past the top is nobody's base.)
    ['sort again', ['/usr/bin/sort', '/etc/passwd'], /root/],
  ];
  const ends = [];
  for (const [name, argv, want] of novel) {
    const r = await timeout(sys.run(argv, { env, cwd: '/tmp' }), 240000, `${name}, new to the page, the pool all but spent`);
    ends.push(r.signal === 'SIGKILL' ? 'killed' : r.code === 0 && want.test(r.stdout) ? 'ran' : `${name}: status ${r.code} ${r.signal ?? ''} ${r.stderr.slice(-200)}`);
  }
  say(`  programs new to the page, ${LEFT} pages of slots left: ${novel.map(([name], i) => `${name} ${ends[i]}`).join(', ')}`);
  const killed = ends.filter((e) => e === 'killed').length;
  check('a program that needs new code runs whole or ends, killed', ends.every((e) => e === 'ran' || e === 'killed'), ends.join(', '));
  check('and from the first that does not fit on, each is killed', killed >= 1 && ends.indexOf('killed') === ends.length - killed, ends.join(', '));
  eq('the pool is spent', await left(), 0);
  const said = await t.logs(/dispatch slot|panic|gex: heal/);
  const why = said.filter((l) => /pid \d+: no dispatch slots are left to translate the code at 0x[0-9a-f]+ \(\S+ \+0x[0-9a-f]+\) into/.test(l));
  // (A line may be relayed twice: the console's, and the process's own.)
  check('the runtime saying which process, where and why, each time', new Set(why.map((l) => /pid (\d+)/.exec(l)[1])).size === killed, `${why.length} lines for ${killed} killed: ${said.slice(-3).join(' / ')}`);
  // The program tried twice ends where it did: the pages its first try
  // numbered past the top are nobody's (taken as its bases, the second try
  // ran on with entries naming a table there is none of, and ended
  // elsewhere).
  const place = new Map(why.map((l) => [/pid (\d+)/.exec(l)[1], /\((\S+ \+0x[0-9a-f]+)\)/.exec(l)[1]]));
  const sorts = [...place.values()].filter((at) => at.startsWith('sort '));
  if (ends[0] === 'killed') check('and a program tried again ends where it did the first time', sorts.length === 2 && sorts[0] === sorts[1], [...place.values()].join(', '));
  check('and that the pool is spent', said.some((l) => /the runtime dispatch slot space is spent/.test(l)), said.slice(-3).join(' / '));
  check('with no panic in the runtime', !said.some((l) => /panic/.test(l)), said.filter((l) => /panic/.test(l)).slice(0, 2).join(' / '));
  check('and no page left numbered and unwritten', !said.some((l) => /gex: heal/.test(l)), said.filter((l) => /gex: heal/.test(l)).slice(0, 2).join(' / '));
  const after = await run(['/bin/sh', '-c', 'ls /usr/bin | grep -c . | cat'], 'the programs run before, after that');
  check('and the programs run before go on running', after.code === 0 && Number(after.stdout) > 100, `${after.code} ${after.signal} ${after.stdout.slice(0, 40)}`);
  const git = await run(['/usr/bin/git', '-C', '/tmp/r', 'status', '--short'], 'git, after that');
  check('git among them', git.code === 0, `${git.code} ${git.signal}`);
  await sys.shutdown();
}
