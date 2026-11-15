// A fatal fault in a Mac program says where it was (hosts/native/src/
// crashinfo.rs), in the guest's own addresses (the Darwin window, at 4 GiB
// and up) and the images found by offset: fixtures/darwin/crashinfo.c
// dereferences a pointer nothing maps from a function in a dylib
// (crashinfo-lib.c), under GEX_CRASH=1. The block names the dylib, the
// offset in it and the nearest symbol (from its symbol table), and a
// backtrace by frame pointer reaches the program; a handled SIGSEGV prints
// nothing. Natively, against the Mac root pyrus builds.
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { exec, native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built and checked on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const n = native(t);
  const d = n.dir('crashinfo');
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  const fx = join(ROOT, 'test/fixtures/darwin');
  const cc = async (...args) => {
    const r = await exec(['xcrun', 'clang', '-arch', x86 ? 'x86_64' : 'arm64', '-O1', '-fno-omit-frame-pointer', '-w', ...args], { cwd: d });
    if (r.code !== 0) t.skip(`xcrun clang: ${r.stderr.trim().split('\n').pop()}`);
  };
  await cc('-dynamiclib', '-install_name', '@rpath/libcrashinfo.dylib', '-o', join(d, 'libcrashinfo.dylib'), join(fx, 'crashinfo-lib.c'));
  await cc('-o', join(d, 'crashinfo'), join(fx, 'crashinfo.c'), `-L${d}`, '-lcrashinfo', '-Wl,-rpath,@executable_path');
  // The program's own directory, at the same path in the guest: its dylib is found beside it.
  const run = (args, env) => n.gexc(['run', '--sysroot', mac, '--bind', `${d}=${d}`, join(d, 'crashinfo'), ...args], { env: { GEX_LOG: 'off', ...env }, timeout: 200000 });

  const bad = await run([], { GEX_CRASH: '1' });
  const err = bad.stderr;
  t.check('the program died of SIGSEGV', bad.code === 139, `exit ${bad.code}: ${err.trim().split('\n').slice(-3).join(' / ')}`);
  const fault = /^gex: fatal SIGSEGV \(pid \d+, thread \d+\) at fault address (0x[0-9a-f]+)/m.exec(err);
  t.check('the block names the signal and a fault address, in the window', !!fault && BigInt(fault[1]) >= 0x1_0000_0000n, err);
  const pc = /^gex:   pc +(0x[0-9a-f]+)  (\S+)\+0x[0-9a-f]+  (\S+)\+0x[0-9a-f]+$/m.exec(err);
  t.check('pc is a window address in libcrashinfo.dylib, at one of its functions',
    !!pc && BigInt(pc[1]) >= 0x1_0000_0000n && pc[2] === join(d, 'libcrashinfo.dylib') && /^_crash_(inner|outer)$/.test(pc[3]), pc ? pc.slice(1).join(' ') : err);
  if (!x86) {
    const lr = /^gex:   lr +0x[0-9a-f]+  (\S+)\+0x[0-9a-f]+  (\S+)\+0x[0-9a-f]+$/m.exec(err);
    t.check('lr is in libcrashinfo.dylib, in _crash_outer', !!lr && lr[1] === join(d, 'libcrashinfo.dylib') && lr[2] === '_crash_outer', lr ? lr.slice(1).join(' ') : err);
  }
  t.check('the backtrace reaches main in the program', new RegExp(`^gex:   #\\d+ +0x[0-9a-f]+  ${join(d, 'crashinfo')}\\+0x[0-9a-f]+  _main\\+0x[0-9a-f]+$`, 'm').test(err), err);
  const handled = await run(['handle'], { GEX_CRASH: '1' });
  t.check('a handled SIGSEGV prints nothing and the handler runs', handled.code === 0 && handled.stdout.includes('handled') && !handled.stderr.includes('gex: fatal'), `exit ${handled.code}: ${handled.stderr}`);
  // An instruction that ends the process, as a Mac's libraries end one that
  // misuses them (libxpc's _xpc_api_misuse is a ud2): the processor's own
  // undefined instruction is SIGILL (EXC_BAD_INSTRUCTION), the compiler's
  // trap SIGILL on x86-64 (a ud2) and SIGTRAP on arm64 (a brk,
  // EXC_BREAKPOINT), each a crash of the program's with where it was, and
  // each a handler's to take.
  for (const [how, signal, number] of [['undefined', 'SIGILL', 4], ['trap', x86 ? 'SIGILL' : 'SIGTRAP', x86 ? 4 : 5]]) {
    const died = await run([how], { GEX_CRASH: '1', GEX_LOG: '' });
    t.check(`${how}: the program died of ${signal}`, died.code === 128 + number, `exit ${died.code}: ${died.stderr.trim().split('\n').slice(-3).join(' / ')}`);
    t.check(`${how}: the block names ${signal}`, new RegExp(`^gex: fatal ${signal} \\(pid \\d+, thread \\d+\\)`, 'm').test(died.stderr), died.stderr);
    t.check(`${how}: pc is in the dylib's _crash_trap`, /^gex:   pc +0x[0-9a-f]+  \S+libcrashinfo\.dylib\+0x[0-9a-f]+  _crash_trap\+0x[0-9a-f]+$/m.test(died.stderr), died.stderr);
    // (On arm64 the function that traps is a leaf with no frame of its
    // own: who called it is the link register's to say.)
    t.check(`${how}: its caller is main`, (x86 ? /^gex:   #\d+ +0x[0-9a-f]+  \S+\+0x[0-9a-f]+  _main\+0x[0-9a-f]+$/m : /^gex:   lr +0x[0-9a-f]+  \S+\+0x[0-9a-f]+  _main\+0x[0-9a-f]+$/m).test(died.stderr), died.stderr);
    t.check(`${how}: it is the program's crash, not one gex could not run`, !/could not run/.test(died.stderr), died.stderr);
    const taken = await run([how, 'handle'], { GEX_CRASH: '1' });
    t.check(`${how}: a handler takes ${signal}`, taken.code === 0 && taken.stdout.includes(`handled ${signal}`) && !taken.stderr.includes('gex: fatal'), `exit ${taken.code}: ${taken.stdout} ${taken.stderr}`);
  }
  const quiet = await run([], {});
  t.check('with GEX_CRASH unset (stderr is no terminal) a crash prints no block', !quiet.stderr.includes('gex: fatal'), quiet.stderr);
  t.say(err.trim());
  n.clean();
}
