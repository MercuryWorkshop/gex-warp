// A Mac program started by the page in a tree that is not a Mac's: the
// Mac root (pyrus's, the `darwin-root` image) is a directory of the
// system's tree, as a Wine prefix is, and the page runs a program of it by
// its name there (runProgram, page/src/run.js). The kernel enters the root
// as the process becomes the program (crates/gex-kernel/src/darwin/root.rs):
// it finds itself at `/`, the system it came from at `/Volumes/Linux`, and
// the working directory it was started in by its name there.
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
  let tar;
  try {
    tar = await bytesOf(t.work(`darwin-root.${t.arch}.tar`));
  } catch {
    t.skip(`no Mac root for ${t.arch} (GEX_TEST_DARWIN_ROOT${t.arch === 'x86' ? '_X86' : ''}: pyrus builds one)`);
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
  const script = [
    'uname -s',
    'echo "at $(pwd)"',
    'ls -1d /usr/lib/dyld /Volumes/Linux/opt/mac/usr/lib/dyld /Volumes/Linux/home/user/note',
    'cat note',
    'echo from the Mac > /tmp/made && cat /Volumes/Linux/tmp/made',
    'exit 5',
  ].join('; ');
  const run = runProgram(term, {
    filesystem: 'memory',
    elf: tree.files[tree.hard['/opt/mac/bin/bash'] ?? '/opt/mac/bin/bash'],
    args: ['/opt/mac/bin/bash', '-c', script],
    files: { ...tree.files, '/home/user/note': 'a note in the system\'s tree\n' },
    dirs: [...(tree.dirs ?? []), '/tmp', '/home/user'],
    modes: tree.modes,
    hard: tree.hard,
    symlinks: tree.links ?? {},
    env: ['PATH=/usr/bin:/bin:/usr/sbin:/sbin', 'HOME=/'],
    cwd: '/home/user',
  });
  run.catch((e) => { out += `\n[runProgram threw: ${e?.stack ?? e}]`; });
  const r = await timeout(run, 120000, 'the end').catch((e) => { throw new Error(`${e.message}: ${out.slice(-600)}`); });
  const lines = out.replace(/\r/g, '');
  say(lines.split('\n').map((l) => `  | ${l}`).join('\n'));
  eq('the program\'s status', r.status, 5);
  check('it is a Mac\'s', /^Darwin$/m.test(lines), lines.slice(0, 300));
  check('in the directory it was started in, by its name in the root', /^at \/Volumes\/Linux\/home\/user$/m.test(lines));
  check('its root is the Mac\'s, and the system a volume of it', /^\/Volumes\/Linux\/home\/user\/note\n\/Volumes\/Linux\/opt\/mac\/usr\/lib\/dyld\n\/usr\/lib\/dyld$/m.test(lines));
  check('a file of the system\'s tree, by a relative name', /^a note in the system's tree$/m.test(lines));
  check('the two share /tmp', /^from the Mac$/m.test(lines));
}
