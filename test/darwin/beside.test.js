// A Linux program and a Mac one side by side in one system on the page:
// fixtures/darwin/page/beside.c, a Linux program (static, musl), runs
// Apple's bash from the Mac root (pyrus's, the `darwin-root` image) at
// /opt/mac as its child. The two talk through the one kernel under them: a
// FIFO each way in the /tmp they share, POSIX shared memory by name (the
// Mac program reads what the Linux one wrote), the child's exit status, and
// the signals that stop a job and set it going.
import { runProgram } from '../../page/src/run.js';
import { setGlobals } from '../../page/src/page-globals.js';
import { untar } from '../../page/src/tar.js';
import { bytesOf } from '../../page/runtime/src/internal.js';

export const meta = { hosts: ['browser', 'node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 300 };

export default async function (t) {
  const { say, check, eq, timeout } = t;
  const dbg = t.debug();
  const sw = Object.fromEntries(Object.entries(dbg).filter(([k]) => k !== 'arch'));
  globalThis.GEX_PAGE_SEARCH = `?${new URLSearchParams(sw)}`;
  globalThis.GEX_ASSET_BASE = t.baseUrl;
  setGlobals((n) => sw[n] ?? null, { arch: dbg.arch ?? 'aarch64' });
  let tar, prog;
  try {
    tar = await bytesOf(t.work(`darwin-root.${t.arch}.tar`));
    prog = await bytesOf(t.work(`darwin-beside.${t.arch}`));
  } catch {
    t.skip(`no Mac root for ${t.arch} (GEX_TEST_DARWIN_ROOT${t.arch === 'x86' ? '_X86' : ''}: pyrus builds one), or no compiler for the Linux program (zig, or GEX_TOOLCHAIN's)`);
  }
  const tree = untar(tar, '/opt/mac');

  let out = '';
  const term = {
    rows: 24, cols: 80,
    write: (s) => { out += typeof s === 'string' ? s : new TextDecoder().decode(s); },
    writeln: (s) => { out += `${s}\n`; },
    onData: () => ({ dispose() {} }),
    onResize: () => ({ dispose() {} }),
    focus() {},
  };
  const run = runProgram(term, {
    filesystem: 'memory',
    elf: prog,
    args: ['/bin/beside'],
    files: { ...tree.files, '/bin/beside': prog },
    dirs: [...(tree.dirs ?? []), '/tmp', '/bin'],
    modes: { ...tree.modes, '/bin/beside': 0o755 },
    hard: tree.hard,
    symlinks: tree.links ?? {},
    env: ['PATH=/usr/bin:/bin:/usr/sbin:/sbin', 'HOME=/'],
    cwd: '/tmp',
  });
  run.catch((e) => { out += `\n[runProgram threw: ${e?.stack ?? e}]`; });
  const r = await timeout(run, 120000, 'the end').catch((e) => { throw new Error(`${e.message}: ${out.slice(-600)}`); });
  const lines = out.replace(/\r/g, '');
  say(lines.split('\n').map((l) => `  | ${l}`).join('\n'));
  eq('the Linux program\'s status', r.status, 3);
  check('it is Linux\'s', /^beside: this is a Linux program$/m.test(lines), lines.slice(0, 300));
  check('a FIFO each way: the Mac program heard it, and answered', /^beside: a Darwin program heard: hello from Linux$/m.test(lines));
  check('the Mac program stopped as its job, and went on', /^beside: the Mac program stopped$/m.test(lines));
  check('and ended with its own status', /^beside: the Mac program ended, status 7$/m.test(lines));
  check('shared memory by name, which the Mac program reads', /^beside: it reads the shared memory: written by Linux$/m.test(lines));
}
