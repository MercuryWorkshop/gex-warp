// A fatal fault says where the guest was (hosts/native/src/crashinfo.rs):
// GEX_CRASH=1 puts one block on stderr, with the signal, the fault address,
// and for the pc, the link register and the frame-pointer backtrace, the
// image that holds each (its path and the offset in it) and the nearest
// symbol (fixtures/native/crashinfo: main dereferences a pointer nothing
// maps from a function in a shared object, two calls deep). A signal with a
// handler says nothing, and so does a crash with GEX_CRASH unset (stderr is
// not a terminal here) or 0. Natively.
import { copyFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { native } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 300 };

export default async function (t) {
  const n = native(t);
  const x86 = t.arch === 'x86';
  const d = n.dir('crashinfo');
  const cflags = ['-O1', '-fno-omit-frame-pointer'];
  await n.cc([...cflags, '-fPIC', '-shared', '-Wl,-soname,libcrash.so', '-o', `${d}/libcrash.so`, n.src('crashinfo/libcrash.c')]);
  await n.cc([...cflags, '-o', `${d}/main`, n.src('crashinfo/main.c'), `-L${d}`, '-lcrash', '-Wl,-rpath,/app']);
  // The guest's root: the dynamic loader where the program asks for it, and the program's directory.
  const root = join(d, 'root');
  mkdirSync(join(root, 'lib'), { recursive: true });
  mkdirSync(join(root, 'app'), { recursive: true });
  const [[ld, libc]] = Object.entries(n.ldso);
  copyFileSync(libc, join(root, ld.slice(1)));
  for (const f of ['main', 'libcrash.so']) copyFileSync(join(d, f), join(root, 'app', f));
  const run = (args, env) => n.gexc(['run', '--sysroot', root, join(root, 'app/main'), ...args], { env: { GEX_LOG: 'off', ...env }, timeout: 120000 });

  const bad = await run([], { GEX_CRASH: '1' });
  const err = bad.stderr;
  t.check('the program died of SIGSEGV', bad.code === 139, `exit ${bad.code}: ${err.trim().split('\n').slice(-3).join(' / ')}`);
  t.check('the block names the signal and a fault address', /^gex: fatal SIGSEGV \(pid \d+, thread \d+\) at fault address 0x[0-9a-f]+/m.test(err), err);
  const pc = /^gex:   pc +0x[0-9a-f]+  (\S+)\+0x[0-9a-f]+  (\S+)\+0x[0-9a-f]+$/m.exec(err);
  t.check('pc is in libcrash.so, at one of its functions', !!pc && pc[1] === '/app/libcrash.so' && /^crash_(inner|outer)$/.test(pc[2]), pc ? pc.slice(1).join(' ') : err);
  if (!x86) {
    const lr = /^gex:   lr +0x[0-9a-f]+  (\S+)\+0x[0-9a-f]+  (\S+)\+0x[0-9a-f]+$/m.exec(err);
    t.check('lr is in libcrash.so, in crash_outer', !!lr && lr[1] === '/app/libcrash.so' && lr[2] === 'crash_outer', lr ? lr.slice(1).join(' ') : err);
  }
  t.check('the backtrace reaches main in the program', /^gex:   #\d+ +0x[0-9a-f]+  \/app\/main\+0x[0-9a-f]+  main\+0x[0-9a-f]+$/m.test(err), err);

  const quiet = await run([], {});
  t.check('with GEX_CRASH unset a crash prints no block', !quiet.stderr.includes('gex: fatal'), quiet.stderr);
  const off = await run([], { GEX_CRASH: '0' });
  t.check('with GEX_CRASH=0 neither', !off.stderr.includes('gex: fatal'), off.stderr);
  const handled = await run(['handle'], { GEX_CRASH: '1' });
  t.check('a handled SIGSEGV prints nothing and the handler runs', handled.code === 0 && handled.stdout.includes('handled') && !handled.stderr.includes('gex: fatal'), `exit ${handled.code}: ${handled.stderr}`);
  const abrt = await run(['abort'], { GEX_CRASH: '1' });
  t.check('an abort says SIGABRT and where', abrt.code === 134 && /^gex: fatal SIGABRT /m.test(abrt.stderr) && /^gex:   pc /m.test(abrt.stderr), `exit ${abrt.code}: ${abrt.stderr}`);
  t.say(err.trim());
  n.clean();
}
