// A Mac program on a pseudo-terminal of the runtime, which is how a web
// desktop gives a program a terminal (`sys.spawn(argv, { pty })`,
// page/runtime/README.md): Apple's bash from a Mac root (pyrus's, the
// `darwin-root` image) started that way, typed at through the pty's master.
// The terminal answers what a shell asks of it (its settings, its size, its
// name), a line is edited, a program is interrupted and one stopped from
// the keyboard, and a resize is heard.
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';

export const meta = { hosts: ['browser', 'node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 300 };

export default async function (t) {
  const { say, check, eq, until, timeout } = t;
  const sys = new System({ baseUrl: t.baseUrl, debug: t.debug() });
  const fs = new MemoryFilesystem(sys);
  try {
    fs.addTar(t.work(`darwin-root.${t.arch}.tar`));
    await timeout(sys.boot(), 60000, 'boot');
  } catch (e) {
    t.skip(`no Mac root for ${t.arch} (GEX_TEST_DARWIN_ROOT${t.arch === 'x86' ? '_X86' : ''}: pyrus builds one): ${e?.message ?? e}`);
  }
  const env = { PATH: '/usr/bin:/bin:/usr/sbin:/sbin', HOME: '/', TERM: 'xterm-256color', PS1: 'mac:\\W > ' };
  const sh = sys.spawn(['/bin/bash', '--noprofile', '--norc', '-i'], { pty: { cols: 100, rows: 30, encoding: 'utf-8' }, env, cwd: '/' });
  let out = '';
  sh.pty.addEventListener('data', (e) => { out += e.data; });
  const shown = () => out.replace(/\r/g, '').replace(/\x1b\[[0-9;?]*[@-~]/g, '');
  const prompts = () => (shown().match(/^mac:\S+ > /gm) ?? []).length;
  const enter = async (line, what, ms = 60000) => {
    const n = prompts();
    sh.pty.write(line);
    await timeout(until(() => prompts() > n, what, ms), ms + 5000, what).catch((e) => { throw new Error(`${e.message}: ${shown().slice(-600)}`); });
  };
  await timeout(until(() => prompts() > 0, 'the prompt', 120000), 130000, 'start').catch((e) => { throw new Error(`${e.message}: ${shown().slice(-600)}`); });
  await enter('uname -s; tty; stty size\r', 'the system and its terminal');
  await enter('stty -a | head -1 | cut -d";" -f1,2,3\r', 'its settings');
  // The terminal by its name, as a shell opens its own (zsh does, by what
  // ttyname says), and by `/dev/tty`.
  await enter('(stty size < "$(tty)") && (exec 3<>/dev/tty && stty size <&3)\r', 'the terminal by name');
  await enter('cho $((6*7))xx\x7f\x7f\x01e\r', 'an edited line');
  {
    const n = prompts();
    sh.pty.write('sleep 100; echo "status $?"\r');
    await new Promise((r) => setTimeout(r, 1500));
    sh.pty.write('\x03');
    await timeout(until(() => prompts() > n, 'an interrupt', 60000), 65000, 'an interrupt').catch((e) => { throw new Error(`${e.message}: ${shown().slice(-600)}`); });
  }
  {
    const n = prompts();
    sh.pty.write('sleep 100\r');
    await new Promise((r) => setTimeout(r, 1500));
    sh.pty.write('\x1a');
    await timeout(until(() => prompts() > n, 'a stop', 60000), 65000, 'a stop').catch((e) => { throw new Error(`${e.message}: ${shown().slice(-600)}`); });
  }
  await enter('jobs | sed "s/  */ /g"; kill %1; wait %1 2>/dev/null; echo "ended $?"\r', 'the stopped job');
  sh.pty.resize(132, 40);
  await enter('stty size\r', 'the new size');
  sh.pty.write('exit 7\r');
  const r = await timeout(sh.exited, 60000, 'the end');
  const lines = shown();
  say(lines.split('\n').map((l) => `  | ${l}`).join('\n'));
  eq('the shell\'s status', r.code, 7);
  check('it is a Mac\'s', /^Darwin$/m.test(lines), lines.slice(0, 300));
  check('on a terminal with a Mac\'s name for it', /^\/dev\/ttys\d{3}$/m.test(lines));
  check('of the size the page gave it', /^30 100$/m.test(lines));
  check('opened by its name and by /dev/tty, the same terminal', (lines.match(/^30 100$/gm) ?? []).length >= 3);
  check('set as a terminal is', /^speed \d+ baud; 30 rows; 100 columns$/m.test(lines));
  check('a line edited before it is entered', /^42$/m.test(lines));
  check('a program interrupted from the keyboard', /\^C/.test(lines) && /^status 130$/m.test(lines));
  check('a program stopped from the keyboard, and ended', /\^Z/.test(lines) && /^\[1\]\+ Stopped sleep 100$/m.test(lines) && /^ended 143$/m.test(lines));
  check('and of the size it was made', /^40 132$/m.test(lines));
  await sys.shutdown?.();
}
