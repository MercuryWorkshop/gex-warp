// What /dev/gex-services carries, measured on the page runtime (as
// darwin/services' program is measured natively by hand: see the report of
// the change that made it): fixtures/darwin/servicebench.c against the
// synthetic service `bench` (fixtures/darwin/bench-provider.mjs), served on
// the page's thread and then by a Worker the connection is handed to.
// (a) 8 MB frames from the host, 60 a second and then as fast as they go,
// on a bulk connection; (b) 64-byte round trips, one a millisecond, and
// 64-byte events from the host a millisecond apart. The numbers are said;
// the checks are that every frame came, whole and in order, and that the
// Worker's connections were the Worker's.
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { bytesOf } from '../../page/runtime/src/internal.js';
import { bench } from '../fixtures/darwin/bench-provider.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 900 };

export default async function (t) {
  const { check, timeout } = t;
  const arch = t.arch === 'x86' ? 'x86' : 'aarch64';
  let prog;
  try {
    prog = await bytesOf(t.work(`darwin-servicebench.${arch}`));
  } catch {
    t.skip(`no Mac root or no Mac compiler for ${t.arch}`);
  }
  const { arch: _, ...debug } = t.debug();
  const sys = new System({ baseUrl: t.baseUrl, arch, debug, services: { bulk: 2, bulkCapacity: 1 << 24 } });
  const fs = new MemoryFilesystem(sys);
  fs.addTar(t.work(`darwin-root.${arch}.tar`));
  fs.addFiles({ '/usr/local/bin/servicebench': prog }, { mode: 0o755 });
  let stop = sys.services.provide('bench', bench);
  await timeout(sys.boot(), 120000, 'boot');
  const run = async (...args) => {
    const r = await timeout(sys.run(['/usr/local/bin/servicebench', ...args], { env: { PATH: '/usr/bin:/bin', HOME: '/' }, cwd: '/' }), 300000, args.join(' '));
    for (const line of r.stdout.trim().split('\n')) t.say(line);
    return r;
  };
  const measure = async (where) => {
    t.say(`-- the provider on ${where}`);
    const paced = await run('bulk', String(8 << 20), '120', '60');
    check(`${where}: 120 frames of 8 MB, 60 a second, whole and in order`, paced.code === 0, paced.stdout.trim());
    const flat = await run('bulk', String(8 << 20), '120', '0');
    check(`${where}: 120 frames of 8 MB as fast as they go`, flat.code === 0, flat.stdout.trim());
    const small = await run('small', '1000', '1');
    check(`${where}: 1000 round trips and 1000 events of 64 bytes`, small.code === 0, small.stdout.trim());
  };
  await measure("the page's thread");
  stop();
  let worker;
  if (globalThis.process?.versions?.node) {
    const { Worker } = await import('node:worker_threads');
    worker = new Worker(new URL('../fixtures/darwin/bench-worker.mjs', import.meta.url));
  } else {
    worker = new Worker(new URL('../fixtures/darwin/bench-worker.mjs', import.meta.url), { type: 'module' });
  }
  stop = sys.services.provide('bench', { version: 1, worker });
  await measure('a Worker');
  check("the Worker's connections are not the page thread's", sys.services.connections('bench').length === 0);
  stop();
  await worker.terminate();
  await sys.shutdown();
}
