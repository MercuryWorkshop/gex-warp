// A protected Mac program (a segment marked SG_PROTECTED_VERSION_1: Apple's
// encrypted __TEXT) on the page runtime: refused, by name, as gexc refuses
// one it has no key for (crates/gex-ld/src/dsmos.rs). The page loads an
// `execve`'s program from its headers and reads each segment from the file
// straight into place (gex_ld::plan_program), where nothing decrypts it: a
// protected program used to be mapped still encrypted and its garbage run.
// The program here is fixtures/darwin/high.c's (darwin-high.ARCH), its
// __TEXT only marked protected, not encrypted (no key is looked for or
// used). Unmarked, the same program runs. On the node host, gexc too: a
// whole load, refused without a key (GEX_DSMOS_KEY).
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { bytesOf } from '../../page/runtime/src/internal.js';

export const meta = { hosts: ['browser', 'node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 600 };

const LC_SEGMENT_64 = 0x19;
const SG_PROTECTED_VERSION_1 = 0x8;

/** A copy of thin Mach-O `bytes` with its __TEXT segment marked protected. */
function markProtected(bytes) {
  const out = new Uint8Array(bytes);
  const dv = new DataView(out.buffer);
  if (dv.getUint32(0, true) !== 0xfeedfacf) throw new Error('not a thin 64-bit Mach-O');
  let at = 32;
  for (let i = dv.getUint32(16, true); i > 0; i--) {
    const cmd = dv.getUint32(at, true);
    const name = new TextDecoder().decode(out.subarray(at + 8, at + 24)).replace(/\0+$/, '');
    if (cmd === LC_SEGMENT_64 && name === '__TEXT') {
      dv.setUint32(at + 68, dv.getUint32(at + 68, true) | SG_PROTECTED_VERSION_1, true);
      return out;
    }
    at += dv.getUint32(at + 4, true);
  }
  throw new Error('no __TEXT segment');
}

export default async function (t) {
  const { say, check, timeout } = t;
  const arch = t.arch === 'x86' ? 'x86' : 'aarch64';
  let prog;
  try {
    prog = await bytesOf(t.work(`darwin-high.${arch}`));
  } catch {
    t.skip(`no Mac root or no Mac compiler for ${t.arch} (GEX_TEST_DARWIN_ROOT${t.arch === 'x86' ? '_X86' : ''}: pyrus builds one)`);
  }
  const marked = markProtected(prog);
  const { arch: _, ...debug } = t.debug();
  const sys = new System({ baseUrl: t.baseUrl, arch, debug });
  const fs = new MemoryFilesystem(sys);
  fs.addTar(t.work(`darwin-root.${arch}.tar`));
  fs.addFiles({ '/usr/local/bin/plain': prog, '/usr/local/bin/protected': marked }, { mode: 0o755 });
  await timeout(sys.boot(), 120000, 'boot');
  const env = { PATH: '/usr/bin:/bin', HOME: '/' };
  const run = async (argv, what) => {
    const r = await timeout(sys.run(argv, { env, cwd: '/' }), 240000, what);
    say(`  ${what}: ${r.code}${r.signal ? ` ${r.signal}` : ''} ${r.stderr.trim().split('\n').slice(-2).join(' / ').slice(0, 300)}`);
    return r;
  };
  const said = async () => `${(await t.logs(/protected program/)).join('\n')}`;

  const plain = await run(['/usr/local/bin/plain'], 'the program, unmarked');
  check('the program, unmarked, runs', plain.code === 0 && plain.signal == null, `${plain.code} ${plain.signal}`);

  // Started from the page, and exec'd by a shell.
  const direct = await run(['/usr/local/bin/protected'], 'the program marked protected');
  check('marked protected, it is not run', direct.code !== 0 && direct.stdout === '', `${direct.code} ${direct.signal} ${JSON.stringify(direct.stdout.slice(0, 80))}`);
  const shell = await run(['/bin/sh', '-c', '/usr/local/bin/protected; echo "status $?"'], 'exec\'d by a shell');
  check('nor exec\'d by a shell, whose exec fails', /status (?!0\b)\d+/.test(shell.stdout) && !/0x[0-9a-f]{9,}/.test(shell.stdout), JSON.stringify(shell.stdout.slice(0, 200)));
  const why = `${direct.stderr}\n${shell.stderr}\n${await said()}`;
  check('the runtime saying which program and why', /\/usr\/local\/bin\/protected: a protected program .*SG_PROTECTED_VERSION_1.*no key/.test(why), why.trim().split('\n').slice(-3).join(' / '));
  await sys.shutdown();

  // gexc's whole load: refused with no key.
  if (t.host === 'node' && !process.env.GEX_DSMOS_KEY) {
    const { native } = await import('../lib/native.mjs');
    const { writeFileSync } = await import('node:fs');
    const { join } = await import('node:path');
    const mac = process.env[t.arch === 'x86' ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'];
    if (!mac) return;
    const n = native(t);
    const file = join(n.dir('protected'), 'protected');
    writeFileSync(file, marked, { mode: 0o755 });
    const r = await n.gexc(['run', '--sysroot', mac, file], { timeout: 120000 });
    check('gexc refuses it too, without a key', r.code !== 0 && /protected binary/.test(r.stderr), `${r.code} ${r.stderr.trim().split('\n').slice(-2).join(' / ')}`);
  }
}
