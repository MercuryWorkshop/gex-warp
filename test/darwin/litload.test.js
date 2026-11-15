// A load at a pc-relative constant address in a bundle dlopen'd at run
// time reads the global as it is now, not as it was when the code was
// translated (crates/gex-xlate/src/lib.rs `lift_one`: memory is folded on a
// scratch copy, for discovery, and never into the emitted block).
// fixtures/darwin/litload.c dlopens litload-bundle.c's bundle and reads
// back a zero-filled global (__DATA,__common) and an initialised one
// (__DATA,__data) after it writes them. On AArch64 the reads are LDR
// literal, which ld64 makes of an `adrp`+`ldr` near its target and Apple's
// linker today no longer does: the bundle's pairs are rewritten here (a
// `nop`, and the `ldr` at the target), as Ruby's fiddle.bundle has them;
// that bundle read its module back as `false` (`can't modify frozen
// FalseClass` from `require "fiddle"`). On x86-64, rip-relative `mov`s.
// The same lines as on the Mac. Natively, against the Mac root pyrus builds.
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { exec, native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

/**
 * Each `adrp xN, sym@PAGE` followed by `ldr xT, [xN, sym@PAGEOFF]` in the
 * `words` words from offset `at` becomes `nop` and `ldr xT, sym`, as ld64's
 * AdrpLdr optimisation leaves them. How many pairs were rewritten.
 */
function literalise(bytes, at, words) {
  let n = 0;
  for (let k = 0; k + 1 < words; k++) {
    const pc = at + 4 * k;
    const adrp = bytes.readUInt32LE(pc);
    const ldr = bytes.readUInt32LE(pc + 4);
    const rd = adrp & 31;
    // ADRP; LDR (immediate, unsigned offset), 64-bit, from that register.
    if (((adrp & 0x9f000000) >>> 0) !== 0x90000000 || ((ldr & 0xffc00000) >>> 0) !== 0xf9400000 || ((ldr >> 5) & 31) !== rd) continue;
    let imm = (((adrp >>> 5) & 0x7ffff) << 2) | ((adrp >>> 29) & 3);
    if (imm & (1 << 20)) imm -= 1 << 21;
    const target = (pc & ~0xfff) + imm * 4096 + ((ldr >>> 10) & 0xfff) * 8;
    const delta = (target - (pc + 4)) / 4;
    if (!Number.isInteger(delta) || Math.abs(delta) >= 1 << 18) throw new Error(`no literal reach from ${pc.toString(16)} to ${target.toString(16)}`);
    bytes.writeUInt32LE(0xd503201f, pc);
    bytes.writeUInt32LE((0x58000000 | ((delta & 0x7ffff) << 5) | (ldr & 31)) >>> 0, pc + 4);
    n++;
  }
  return n;
}

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built and checked on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const n = native(t);
  const d = n.dir('litload');
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  const fx = join(ROOT, 'test/fixtures/darwin');
  const run = async (argv) => {
    const r = await exec(argv, { cwd: d });
    if (r.code === 127) t.skip(`no ${argv[0]} ${argv[1]}`);
    if (r.code !== 0) throw new Error(`${argv.join(' ')}: ${r.stderr.trim().split('\n').slice(-4).join(' / ')}`);
    return r;
  };
  const cc = (...args) => run(['xcrun', 'clang', '-arch', x86 ? 'x86_64' : 'arm64', '-O1', '-Wall', '-Werror', ...args]);
  const bundle = join(d, 'litload.bundle');
  const prog = join(d, 'litload');
  await cc('-fcommon', '-bundle', '-o', bundle, join(fx, 'litload-bundle.c'));
  await cc('-o', prog, join(fx, 'litload.c'));
  if (!x86) {
    // __TEXT starts the file, at address 0: a symbol's address is its offset.
    const syms = (await run(['xcrun', 'nm', '-U', bundle])).stdout;
    const bytes = readFileSync(bundle);
    const at = (s) => {
      const m = new RegExp(`^([0-9a-f]+) T ${s}$`, 'm').exec(syms);
      if (!m) throw new Error(`no ${s} in ${bundle}`);
      return parseInt(m[1], 16);
    };
    const pairs = literalise(bytes, at('_lit_load_common'), 2) + literalise(bytes, at('_lit_load_data'), 2) + literalise(bytes, at('_lit_swap'), 5);
    t.check('the bundle\'s three adrp+ldr pairs are literal loads', pairs === 3, `${pairs} rewritten`);
    writeFileSync(bundle, bytes);
    // Its signature again, for the Mac to load it.
    await run(['codesign', '-f', '-s', '-', bundle]);
  }
  const want = await exec([prog, bundle], { cwd: d });
  const lines = 'as loaded: common 0, data 0x7\nset once: common 0x1234, data 0x5678\nset again: common 0xabcd, data 0\nswap: 0x1111, then 0x2222\nafter swap: common 0x2222\n';
  t.check('on the Mac: the globals as they are each time', want.code === 0 && want.stdout === lines, `exit ${want.code}: ${JSON.stringify(want.stdout)} ${want.stderr.trim()}`);
  // The program's own directory, at the same path in the guest.
  const got = await n.gexc(['run', '--sysroot', mac, '--bind', `${d}=${d}`, prog, bundle], { env: { GEX_LOG: 'off' }, timeout: 200000 });
  t.check('under gexc: as on the Mac', got.code === want.code && got.stdout === want.stdout,
    `exit ${got.code}: ${JSON.stringify(got.stdout)} ${got.stderr.trim().split('\n').slice(-3).join(' / ')}`);
  n.clean();
}
