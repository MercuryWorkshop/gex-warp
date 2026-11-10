// A Mac's shell on a terminal of the page: Apple's bash from a Mac root
// (pyrus's), started as a page starts a program (runProgram, page/src/run.js)
// on a terminal bridged to an xterm-shaped object, with keys typed into it:
// a prompt, a line edited before it is entered, a pipeline of the root's
// tools, the terminal's size, a program interrupted from the keyboard and one
// stopped from it, a job in the background, and the shell's exit status.
//
// The root is the `darwin-root` image (test/fixtures/images.mjs).
import { runProgram } from '../../page/src/run.js';
import { setGlobals } from '../../page/src/page-globals.js';
import { untar } from '../../page/src/tar.js';
import { bytesOf } from '../../page/runtime/src/internal.js';

export const meta = { hosts: ['browser', 'node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 300 };

export default async function (t) {
  const { say, check, eq, until, timeout } = t;
  const dbg = t.debug();
  const sw = Object.fromEntries(Object.entries(dbg).filter(([k]) => k !== 'arch'));
  globalThis.GEX_PAGE_SEARCH = `?${new URLSearchParams(sw)}`;
  globalThis.GEX_ASSET_BASE = t.baseUrl;
  setGlobals((n) => sw[n] ?? null, { arch: dbg.arch ?? 'aarch64' });
  let tar;
  try {
    tar = await bytesOf(t.work(`darwin-root.${t.arch}.tar`));
  } catch {
    t.skip(`no Mac root for ${t.arch} (GEX_TEST_DARWIN_ROOT${t.arch === 'x86' ? '_X86' : ''}: pyrus builds one)`);
  }
  const tree = untar(tar);

  let out = '';
  let keys = null;
  const term = {
    rows: 30, cols: 100,
    write: (s) => { out += typeof s === 'string' ? s : new TextDecoder().decode(s); },
    writeln: (s) => { out += `${s}\n`; },
    onData: (fn) => { keys = fn; return { dispose() { keys = null; } }; },
    onResize: () => ({ dispose() {} }),
    focus() {},
  };
  const t0 = performance.now();
  const run = runProgram(term, {
    filesystem: 'memory',
    elf: tree.files[tree.hard['/bin/bash'] ?? '/bin/bash'],
    args: ['/bin/bash', '--noprofile', '--norc', '-i'],
    files: tree.files,
    dirs: [...(tree.dirs ?? []), '/dev', '/Volumes'],
    modes: tree.modes,
    hard: tree.hard,
    symlinks: tree.links ?? {},
    xattrs: tree.xattrs,
    env: ['PATH=/usr/bin:/bin:/usr/sbin:/sbin', 'HOME=/', 'TERM=xterm-256color', 'PS1=mac:\\W > '],
    cwd: '/',
  });
  run.catch((e) => { out += `\n[runProgram threw: ${e?.stack ?? e}]`; });
  const shown = () => out.replace(/\r/g, '').replace(/\x1b\[[0-9;?]*[@-~]/g, '');
  // Type a line once the prompt is back, and wait for what it prints.
  // (A prompt at the start of a line: the shell draws its prompt again in
  // the middle of one when a line is edited, which is not a command done.)
  const prompts = () => (shown().match(/^mac:\S+ > /gm) ?? []).length;
  const enter = async (line, what, ms = 60000) => {
    const n = prompts();
    keys?.(line);
    await timeout(until(() => prompts() > n, what, ms), ms + 5000, what).catch((e) => { throw new Error(`${e.message}: ${shown().slice(-600)}`); });
  };
  await timeout(until(() => prompts() > 0, 'the prompt', 120000), 130000, 'start').catch((e) => { throw new Error(`${e.message}: ${shown().slice(-600)}`); });
  say(`a prompt in ${(performance.now() - t0).toFixed(0)} ms`);

  await enter('uname -s; tty; stty size\r', 'the system and its terminal');
  // A line edited before it is entered: two characters rubbed out, the
  // cursor sent home and a word put in front.
  await enter('cho $((6*7))xx\x7f\x7f\x01e\r', 'an edited line');
  await enter('printf "b 2\\na 1\\n" | sort | awk \'{ s += $2; print NR ":" $1 } END { print "sum " s }\'\r', 'a pipeline');
  await enter('cd /usr/lib && pwd && ls -1d dyld libSystem.B.dylib\r', 'the root');
  // A program the keyboard interrupts: its status says so, and the shell
  // (this one is bash 3.2) goes on with the rest of the line.
  {
    const n = prompts();
    keys?.('sleep 100; echo "status $?"\r');
    await new Promise((r) => setTimeout(r, 1500));
    keys?.('\x03');
    await timeout(until(() => prompts() > n, 'an interrupt', 60000), 65000, 'an interrupt').catch((e) => { throw new Error(`${e.message}: ${shown().slice(-600)}`); });
  }
  await enter('echo "after $?"\r', 'the status after');
  // A program the keyboard stops: the shell has its prompt back and the
  // program as a job, which it sets going again behind it and then ends.
  {
    const n = prompts();
    keys?.('sleep 100\r');
    await new Promise((r) => setTimeout(r, 1500));
    keys?.('\x1a');
    await timeout(until(() => prompts() > n, 'a stop', 60000), 65000, 'a stop').catch((e) => { throw new Error(`${e.message}: ${shown().slice(-600)}`); });
  }
  await enter('jobs | sed "s/  */ /g"\r', 'the stopped job');
  await enter('bg %1 > /dev/null; jobs | sed "s/  */ /g"; kill %1; wait %1 2>/dev/null; echo "stopped job ended $?"\r', 'the job set going');
  await enter('sleep 100 &\r', 'a job');
  await enter('jobs | sed "s/  */ /g"; kill %1; wait %1 2>/dev/null; echo "ended $?"\r', 'the job ended');
  keys?.('exit 7\r');
  const r = await timeout(run, 60000, 'the end');
  const lines = shown();
  say(lines.split('\n').map((l) => `  | ${l}`).join('\n'));
  eq('the shell\'s status', r.status, 7);
  check('it is a Mac\'s', /^Darwin$/m.test(lines), lines.slice(0, 300));
  check('on a terminal', /^\/dev\/[\w\/-]+$/m.test(lines));
  check('of the size the page gave it', /^30 100$/m.test(lines));
  check('a line edited before it is entered', /^42$/m.test(lines));
  check('a pipeline of the root\'s tools', /^1:a\n2:b\nsum 3$/m.test(lines));
  check('the root\'s own files', /^\/usr\/lib\ndyld\nlibSystem\.B\.dylib$/m.test(lines));
  check('a program interrupted from the keyboard', /\^C/.test(lines) && /^status 130$/m.test(lines) && /^after 0$/m.test(lines));
  check('a program stopped from the keyboard, set going again, and ended', /\^Z/.test(lines) && /^\[1\]\+ Stopped sleep 100$/m.test(lines) && /^\[1\]\+ Running sleep 100 &$/m.test(lines) && /^stopped job ended 143$/m.test(lines));
  check('a job in the background, and ended', /^\[1\]\+? Running sleep 100 &$/m.test(lines) && /^ended 143$/m.test(lines));
  eq('the system stopped', r.system?.state.phase, 'stopped');
}
