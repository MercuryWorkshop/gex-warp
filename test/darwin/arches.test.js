// Programs of both processors in one system on the page: fixtures/darwin/
// arches.c built for arm64 and for x86-64, each run as the system's own
// with the other its child (host.mjs `#otherArch`, run-node's
// GEX_BOTH_ARCHES: the other processor's shim beside the page's, one kernel
// under both). A process becomes a program of the other processor in a
// Worker of that runtime (the shim's `exec_away`), with its descriptors,
// its place among the processes and its status. A Mac runs both too, the
// x86-64 one under Rosetta: what the two say there is what is asked here.
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { darwin, PROGRAMS } from '../lib/darwin.mjs';
import { exec, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

export default async function (t) {
  const d = darwin(t);
  const x86 = t.arch === 'x86';
  const pub = (f) => join(ROOT, 'page/public/gex', f);
  for (const f of ['seed.shared.wasm', 'gex_browser.shared.wasm', 'seed.x86.shared.wasm', 'gex_browser.x86.shared.wasm']) {
    if (!existsSync(pub(f))) t.skip(`no ${f} in page/public/gex: both processors' shims are wanted (hosts/browser/build.sh, and again with GEX_ARCH=x86)`);
  }
  // The system's own processor's build as the helper makes it, and the
  // other's the same way.
  const own = await d.build('arches');
  const other = { gex: join(d.dir, 'arches-other.gex'), mac: join(d.dir, 'arches-other.mac') };
  const srcs = [join(PROGRAMS, 'arches.c'), join(PROGRAMS, 't.c')];
  const cflags = ['-arch', x86 ? 'arm64' : 'x86_64', '-O1', '-Wall', '-Wextra', '-Werror', '-ffreestanding', '-fno-stack-protector', '-fno-builtin', '-nostdlib', `-I${PROGRAMS}`];
  for (const args of [
    ['-DT_STATIC', '-static', '-e', 'start', '-Wl,-pagezero_size,0x4000', '-Wl,-image_base,0x1000000', '-o', other.gex, ...srcs],
    ['-o', other.mac, ...srcs, '-lSystem'],
  ]) {
    const r = await exec(['xcrun', 'clang', ...cflags, ...args], { cwd: d.dir, env: d.n.env });
    if (r.code !== 0) throw new Error(`clang: ${r.stderr.trim().split('\n').slice(-6).join(' / ')}`);
  }
  const mac = await exec([own.mac, other.mac, own.mac], { cwd: d.dir, env: { T_VAR: 'yes' }, timeout: 120000 });
  if (mac.code !== 0 && /Bad CPU type|rosetta/i.test(mac.stderr)) t.skip('this Mac does not run x86-64 programs (no Rosetta)');
  t.check('the Mac runs the two, each the other\'s child', mac.code === 0 && /there and back: waited for, status 300/.test(mac.stdout), `${mac.stdout} ${mac.stderr}`.slice(-400));
  const r = await d.n.g.run(pub(`seed${x86 ? '.x86' : ''}.shared.wasm`), own.gex, ['/tmp/arches-other', '/tmp/arches'], {
    files: { '/tmp/arches': own.gex, '/tmp/arches-other': other.gex },
    argv0: '/tmp/arches',
    env: { GEX_SHARED: '1', GEX_BOTH_ARCHES: '1' },
    flags: ['--env', 'T_VAR=yes', '--cwd', '/tmp'],
    timeout: 240000,
  });
  const said = r.stdout.split('\n').filter((l) => !l.startsWith('[')).join('\n');
  t.check('and so does the page: as on the Mac', said === mac.stdout && r.code === mac.code,
    `mac ${JSON.stringify(mac.stdout.slice(0, 600))} / page ${JSON.stringify(said.slice(0, 600))} exit ${r.code} / ${r.stderr.trim().split('\n').slice(-4).join(' / ')}`);
}
