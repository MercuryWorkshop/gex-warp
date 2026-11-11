// A system of one processor with a program of the other in it, as a page
// makes one (System's `bothArches`): fixtures/darwin/arches.c, a Mac
// program built for arm64 and for x86-64 (the `darwin-root` image builds
// both, static: they need no root), the system's own started by the page
// with the other as its child, through a pipe, and there and back with no
// fork between. And the other started by the page itself. What they say is
// what darwin/arches has from a Mac, which runs both.
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { bytesOf } from '../../page/runtime/src/internal.js';

export const meta = { hosts: ['browser', 'node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 300 };

const NAME = { x86: 'x86-64', aarch64: 'arm64' };
// What `sysctl hw.machine` tells a process of each (a Mac's names).
const MACHINE = { x86: 'x86_64', aarch64: 'arm64' };

export default async function (t) {
  const { eq, timeout } = t;
  const arch = t.arch === 'x86' ? 'x86' : 'aarch64';
  const other = arch === 'x86' ? 'aarch64' : 'x86';
  let own, others;
  try {
    own = await bytesOf(t.work(`darwin-arches.${arch}`));
    others = await bytesOf(t.work(`darwin-arches.${other}`));
  } catch {
    t.skip('no Mac compiler for the two programs (the darwin-root image builds them with xcrun clang)');
  }
  const { arch: _, ...debug } = t.debug();
  const sys = new System({ baseUrl: t.baseUrl, arch, bothArches: true, debug });
  const fs = new MemoryFilesystem(sys);
  fs.addFiles({ '/tmp/arches': own, '/tmp/arches-other': others }, { mode: 0o755 });
  await timeout(sys.boot(), 60000, 'boot');
  const env = { T_VAR: 'yes' };
  const [me, it] = [NAME[arch], NAME[other]];
  // Each process is told its own processor, whichever the system's is.
  const said = (who, words) => `${NAME[who]}: ${words} (T_VAR yes, hw.machine ${MACHINE[who]})`;
  const through = `${said(other, 'through a pipe')}\n`;
  const r = await timeout(sys.run(['/tmp/arches', '/tmp/arches-other', '/tmp/arches'], { env, cwd: '/tmp' }), 120000, 'the two');
  eq('the system\'s own, the other its child', [r.stdout, r.code], [[
    `${me}: the first`,
    said(other, 'a child'),
    'the other as a child: waited for, status 300',
    `read ${through.length} bytes: ${through.trimEnd()}`,
    'the writer: waited for, status 300',
    `${it}: becoming another`,
    said(arch, 'back again'),
    'there and back: waited for, status 300',
    `${me}: the first, still`,
    '',
  ].join('\n'), 0]);
  // The other processor's program, started by the page itself.
  const s = await timeout(sys.run(['/tmp/arches-other', 'say', 'from', 'the', 'page'], { env, cwd: '/tmp' }), 120000, 'the other alone');
  eq('the other, started by the page', [s.stdout, s.code], [`${said(other, 'from the page')}\n`, 3]);
  await sys.shutdown();
}
