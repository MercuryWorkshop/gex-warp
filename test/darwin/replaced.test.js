// A program replaced at the same path, on the page runtime: run again, it is
// the new program that runs, not the old one's translations. The page keys
// what it has translated by the code's identity, which was the file's name
// and offset alone, and shares it between processes by that key (the
// dispatch slots a page of code is numbered at, `shared_page_table`; the
// modules stored for an object): the new file's code was handed the old
// file's, and the program ended in SIGSEGV. The identity now carries which
// contents of the file were mapped (gex_kernel::mem::Vma::code_stamp: its
// inode, length, change count and time), so a file written over in place,
// one put at the path after the old was removed, and one renamed onto it
// are each other code; and a file run again unchanged is still the same
// code, its pages numbered where they were (darwin/slots).
//
// In a Mac root (pyrus's, the `darwin-root` image): the root's own small
// programs copied to /tmp/prog one after another, each run from a shell.
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { internals } from '../../page/runtime/src/internal.js';

export const meta = { hosts: ['browser', 'node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 900 };

export default async function (t) {
  const { say, check, eq, timeout } = t;
  const arch = t.arch === 'x86' ? 'x86' : 'aarch64';
  const sys = new System({ baseUrl: t.baseUrl, debug: t.debug() });
  const fs = new MemoryFilesystem(sys);
  try {
    fs.addTar(t.work(`darwin-root.${arch}.tar`));
    await timeout(sys.boot(), 120000, 'boot');
  } catch (e) {
    t.skip(`no Mac root for ${t.arch} (GEX_TEST_DARWIN_ROOT: pyrus builds one): ${e?.message ?? e}`);
  }
  const k = internals(sys, 'replaced').kernel();
  const left = async () => Number(await k.r('k_slot_left'));
  const env = { PATH: '/usr/bin:/bin:/usr/sbin:/sbin', HOME: '/tmp', TERM: 'dumb' };
  const sh = async (script) => {
    const r = await timeout(sys.run(['/bin/bash', '-c', script], { env, cwd: '/tmp' }), 240000, script);
    say(`  $ ${script}: ${r.code}${r.signal ? ` ${r.signal}` : ''} ${JSON.stringify(r.stdout.slice(0, 120))} ${JSON.stringify(r.stderr.slice(-160))}`);
    return r;
  };
  const ran = (what, r, want) => check(what, r.code === 0 && r.stdout === want, `${r.code} ${r.signal ?? ''} ${JSON.stringify(r.stdout.slice(0, 120))} ${r.stderr.trim().split('\n').pop() ?? ''}`);

  // (cp's own code drawn first, apart from the program's)
  eq('two programs copied to /tmp', (await sh('cp /usr/bin/basename /tmp/prog && cp /usr/bin/dirname /tmp/other && echo copied')).stdout, 'copied\n');
  const drawn = async (f) => {
    const before = await left();
    await f();
    return before - (await left());
  };
  const first = await drawn(async () => ran('a program copied to a path runs', await sh('/tmp/prog /a/first'), 'first\n'));
  // Unchanged, run again (and again): the same code, sharing what the first
  // run translated -- its slots drawn the first time only (darwin/slots: a
  // chunk at most).
  const again = await drawn(async () => ran('run again, unchanged', await sh('/tmp/prog /a/again'), 'again\n'));
  const more = await drawn(async () => ran('and again', await sh('/tmp/prog /a/more'), 'more\n'));
  const page = t.arch === 'x86' ? 4096 : 1024;
  say(`  pages of slots drawn: the program's first run ${first / page}, run again ${again / page}, and again ${more / page}`);
  check('an unchanged program run again shares what its first run translated (at most 8 pages of slots drawn, fewer than the first run\'s)',
    again <= 8 * page && more <= 8 * page && again < first && more < first, `${again / page}, ${more / page} against ${first / page}`);

  // Replaced: written over in place (the same inode), removed and made
  // again, and another file renamed onto the path; and exec'd by the shell
  // in its own process, the old program's address space replaced.
  ran('written over in place, the new program runs', await sh('cp /usr/bin/dirname /tmp/prog && /tmp/prog /a/second'), '/a\n');
  ran('removed and made again, the new program runs', await sh('rm /tmp/prog; cp /bin/echo /tmp/prog && /tmp/prog third'), 'third\n');
  ran('another renamed onto the path, it runs', await sh('cp /usr/bin/basename /tmp/prog2 && mv /tmp/prog2 /tmp/prog && /tmp/prog /a/fourth'), 'fourth\n');
  ran('and exec\'d in place of the shell', await sh('cp /usr/bin/dirname /tmp/prog && exec /tmp/prog /x/fifth'), '/x\n');
  ran('and back again', await sh('cp /bin/echo /tmp/prog && exec /tmp/prog sixth'), 'sixth\n');
  // Its first contents again, in a file of their own.
  eq('the unchanged copy beside it still the program it was', (await sh('/tmp/other /d/e')).stdout, '/d\n');
  await sys.shutdown();
}
