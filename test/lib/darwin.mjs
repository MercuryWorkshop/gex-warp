// Darwin programs (fixtures/darwin) under gexc and on the Mac that built
// them, whose stdout and exit status must be byte-identical: the Mac's own
// kernel is the oracle for gex's Darwin personality, as QEMU is for AArch64
// Linux (lib/qemu.mjs). a64 only, and only on an arm64 Mac: the programs are
// built by its toolchain (`xcrun clang`), and it is what runs the oracle's
// half.
//
//   const d = darwin(t);
//   const files = await d.build('files');          // fixtures/darwin/files.c
//   await d.compare('files', files);
//
// Each program is built twice from one source over fixtures/darwin/t.c, a
// freestanding C library of raw system calls: a static Mach-O executable
// for gex (no dyld, no libSystem: what a guest can run before it has any
// Darwin userland), linked low enough for a 32-bit guest memory, and an
// ordinary dynamic one for the Mac, which will not run a static program.
// Neither asks anything of libSystem, so what they print is the kernel's
// answers alone.

import { mkdirSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { exec, native } from './native.mjs';

export const PROGRAMS = join(import.meta.dirname, '..', 'fixtures', 'darwin');

/** Where the two outputs first differ, for a failed check's detail. */
function differ(want, got) {
  const a = want.split('\n');
  const b = got.split('\n');
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    if (a[i] !== b[i]) return `line ${i + 1}: mac ${JSON.stringify(a[i] ?? null)}, gex ${JSON.stringify(b[i] ?? null)}`;
  }
  return '';
}

export function darwin(t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('Darwin programs are built and checked on an arm64 Mac');
  const n = native(t);
  const dir = n.dir('darwin');
  const cflags = ['-arch', 'arm64', '-O1', '-Wall', '-Wextra', '-Werror', '-ffreestanding', '-fno-stack-protector', '-fno-builtin', '-nostdlib', `-I${PROGRAMS}`];
  const cc = async (args) => {
    const r = await exec(['xcrun', 'clang', ...cflags, ...args], { cwd: dir, env: n.env });
    if (r.code === 127) t.skip('no xcrun clang (the Xcode command line tools)');
    if (r.code !== 0) throw new Error(`clang ${args.join(' ')}: ${r.stderr.trim().split('\n').slice(-6).join(' / ')}`);
  };
  let runs = 0;
  /** A fresh directory for one run to work in. */
  const cwd = (name) => {
    const d = join(dir, `run-${name}-${++runs}`);
    rmSync(d, { recursive: true, force: true });
    mkdirSync(d, { recursive: true });
    return d;
  };
  return {
    n,
    dir,
    /** fixtures/darwin/NAME.c, both ways: `{ gex, mac }`, the two programs. */
    async build(name, flags = []) {
      const srcs = [join(PROGRAMS, `${name}.c`), join(PROGRAMS, 't.c')];
      const gex = join(dir, `${name}.gex`);
      const mac = join(dir, `${name}.mac`);
      // `__PAGEZERO` shrunk and the image at 16 MiB, where gex loads
      // programs: a static executable is not position-independent.
      await cc([...flags, '-DT_STATIC', '-static', '-e', 'start', '-Wl,-pagezero_size,0x4000', '-Wl,-image_base,0x1000000', '-o', gex, ...srcs]);
      await cc([...flags, '-o', mac, ...srcs, '-lSystem']);
      return { name, gex, mac };
    },
    /** `prog` under gexc: `{ code, stdout, stderr }`. `env` is the guest's. */
    gex(prog, args = [], { env = {}, gexc = [], timeout = 120000 } = {}) {
      const envs = Object.entries({ T_VAR: 'yes', ...env }).flatMap(([k, v]) => ['--env', `${k}=${v}`]);
      return n.gexc(['run', ...gexc, ...envs, prog.gex, ...args], { cwd: cwd(prog.name), timeout });
    },
    /** `prog` on the Mac itself. */
    mac(prog, args = [], { env = {}, timeout = 120000 } = {}) {
      return exec([prog.mac, ...args], { cwd: cwd(prog.name), env: { T_VAR: 'yes', ...env }, timeout });
    },
    /** Both, one check: the same stdout and the same exit status. */
    async compare(label, prog, args = [], opts = {}) {
      const want = await this.mac(prog, args, opts);
      const got = await this.gex(prog, args, opts);
      const same = want.stdout === got.stdout && want.code === got.code;
      const why = want.code !== got.code ? `exit status: mac ${want.code}, gex ${got.code}` : differ(want.stdout, got.stdout);
      t.check(`${label}: as on the Mac (${want.stdout.split('\n').length - 1} lines, exit ${want.code})`, same,
        same ? '' : `${why}${got.stderr.trim() ? ` / gex said: ${got.stderr.trim().split('\n').slice(-4).join(' / ')}` : ''}`);
      t.check(`${label}: nothing failed on the Mac`, !/^FAIL /m.test(want.stdout), want.stdout.split('\n').filter((l) => l.startsWith('FAIL ')).join(' / '));
      return { want, got };
    },
  };
}
