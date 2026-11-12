// /dev/gex-metal: the byte stream a guest's Metal framework reaches a host
// replayer through (hosts/native/src/metal.rs).
//
// First the channel alone, with `cat` as the program at the other end: what
// is written comes back, a read waits without stopping a writer, a
// non-blocking read is told EAGAIN, a megabyte crosses each way at once, two
// opens are two connections; and without --metal there is no node.
//
// Then, if a mink checkout is beside this tree and built (its Metal.framework
// for the pyrus root, its mink-host, and one of its test programs with what
// Apple's Metal answered), that program: an unmodified Mach-O linked against
// Metal, run in the Mac root with mink's framework bound over the root's,
// drawing through the device. Its picture must be the one Apple's Metal drew.
// Without mink (GEX_TEST_MINK, default ../mink) that half is skipped.
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built and checked on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  const n = native(t);
  const d = n.dir('metal');
  const prog = join(d, 'metalecho');
  execFileSync('xcrun', ['clang', '-arch', x86 ? 'x86_64' : 'arm64', '-O1', '-Wall', '-Werror', '-o', prog, join(ROOT, 'test/fixtures/darwin/metalecho.c')], { stdio: 'pipe' });

  const want = [
    'stat: a character device',
    'write: 4',
    'read: ping',
    'write while another thread reads: 4',
    'the waiting read: pong',
    'poll with nothing to read: 0',
    'poll after a write: 1 POLLIN',
    'non-blocking read with nothing: EAGAIN',
    'a megabyte: written whole, read back identical',
    'two opens: one! two!',
    'closed',
    '',
  ].join('\n');
  const echo = await n.gexc(['run', '--sysroot', mac, '--metal', '/bin/cat', prog], { timeout: 120000 });
  t.check('the channel, with cat at the other end', echo.code === 0 && echo.stdout === want, (echo.stdout + echo.stderr).trim().split('\n').slice(-6).join(' / '));
  const named = await n.gexc(['run', '--sysroot', mac, prog], { timeout: 120000, env: { GEX_METAL_HOST: '/bin/cat' } });
  t.check('the program named by GEX_METAL_HOST instead', named.code === 0 && named.stdout === want, (named.stdout + named.stderr).trim().split('\n').slice(-3).join(' / '));
  const absent = await n.gexc(['run', '--sysroot', mac, prog, 'absent'], { timeout: 120000, env: { GEX_METAL_HOST: '' } });
  t.check('no node without a host program', absent.stdout === 'open without a host: ENOENT\n', absent.stdout.trim());

  // mink, if it is there: its triangle through the device.
  const mink = process.env.GEX_TEST_MINK || join(ROOT, '..', 'mink');
  const host = ['target/release/mink-host', 'target/debug/mink-host'].map((p) => join(mink, p)).find(existsSync);
  const framework = join(mink, 'build/framework/pyrus-arm64/System/Library/Frameworks/Metal.framework');
  const pieces = [join(mink, 'build/oracle/triangle'), join(mink, 'build/corpus/triangle.metallib'), join(mink, 'build/oracle/triangle.out')];
  if (x86 || !host || !existsSync(join(framework, 'Versions/A/Metal')) || !pieces.every(existsSync)) {
    t.say(x86 ? 'mink builds its framework for arm64 only: its program is run on a64' : `no built mink at ${mink} (GEX_TEST_MINK; its tools/test-gexc.sh builds what this needs): its program is not run`);
    return;
  }
  copyFileSync(pieces[0], join(d, 'triangle'));
  copyFileSync(pieces[1], join(d, 'triangle.metallib'));
  const run = await n.gexc(
    ['run', '--sysroot', mac, '--metal', host, '--bind', `${framework}=/System/Library/Frameworks/Metal.framework`, '--bind', `${d}=/work`, '--cwd', '/work',
      '--argv0', './triangle', join(d, 'triangle'), 'triangle.metallib', 'triangle.out'],
    { timeout: 300000 },
  );
  const drawn = existsSync(join(d, 'triangle.out')) ? readFileSync(join(d, 'triangle.out')) : Buffer.alloc(0);
  t.check("mink's triangle, drawn through the device, is the picture Apple's Metal drew", run.code === 0 && drawn.equals(readFileSync(pieces[2])),
    `${drawn.length} bytes; ${(run.stdout + run.stderr).trim().split('\n').slice(-3).join(' / ')}`);
}
