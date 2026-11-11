// A page's System with no `Metal` has no /dev/gex-metal: a Mac program's
// open of it is told ENOENT, as under gexc without --metal (darwin/metal).
// fixtures/darwin/metalecho.c in a Mac root, as darwin/metal-page runs it
// with a replayer; a test of its own because a page runs one system.
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { bytesOf } from '../../page/runtime/src/internal.js';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 300 };

export default async function (t) {
  const { eq, timeout } = t;
  const arch = t.arch === 'x86' ? 'x86' : 'aarch64';
  let prog;
  try {
    prog = await bytesOf(t.work(`darwin-metalecho.${arch}`));
  } catch {
    t.skip(`no Mac root or no Mac compiler for ${t.arch} (GEX_TEST_DARWIN_ROOT${t.arch === 'x86' ? '_X86' : ''}: pyrus builds one)`);
  }
  const { arch: _, ...debug } = t.debug();
  const sys = new System({ baseUrl: t.baseUrl, arch, debug });
  const fs = new MemoryFilesystem(sys);
  fs.addTar(t.work(`darwin-root.${arch}.tar`));
  fs.addFiles({ '/usr/local/bin/metalecho': prog }, { mode: 0o755 });
  await timeout(sys.boot(), 120000, 'boot');
  const a = await timeout(sys.run(['/usr/local/bin/metalecho', 'absent'], { env: { PATH: '/usr/bin:/bin', HOME: '/' }, cwd: '/' }), 120000, 'the open');
  eq('no node in a system with no replayer', [a.stdout, a.code], ['open without a host: ENOENT\n', 0], a.stderr.trim().split('\n').slice(-3).join(' / '));
  await sys.shutdown();
}
