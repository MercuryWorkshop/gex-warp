// /dev/gex-metal on the page runtime (hosts/browser/src/metal.rs, the page
// runtime's `Metal`): the same node as the native runner's, each open a
// byte stream to a replayer, here one the page's embedder supplies.
//
// First the channel alone, with a replayer of the test's own that answers
// whatever it is sent (as darwin/metal uses `cat`): the same Mac program,
// fixtures/darwin/metalecho.c, in a Mac root in the page's System. What is
// written comes back, a read waits (parked in the kernel, woken from the
// replayer's realm) without stopping a writer, poll says when there is
// something, a non-blocking read is told EAGAIN, a megabyte crosses each
// way, two opens are two connections. (That a system with no `Metal` has
// no node is darwin/metal-page-absent: a page runs one system.)
//
// Then, in a browser, if a mink checkout is beside this tree and built
// (GEX_TEST_MINK, default ../mink: the `mink` image copies what is needed):
// mink's replayer in a Worker of its own on the browser's WebGPU, mink's
// Metal.framework in the root, and one of mink's test programs, an
// unmodified Mach-O linked against Metal. Its picture must be the one
// Apple's Metal drew. Without mink, or under Node (no WebGPU), that half
// says so and is left out.
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { Metal } from '../../page/runtime/src/metal.js';
import { bytesOf } from '../../page/runtime/src/internal.js';
import { HEAD, TAIL, CLOSED, RING_HEADER_BYTES } from '../../hosts/browser/js/metal-bridge.mjs';

export const meta = { hosts: ['node', 'browser'], arches: ['a64', 'x86'], images: ['darwin-root', 'mink'], timeout: 600 };

/** A replayer that sends back what it is sent, on this thread. */
function echo({ toHost, toGuest, wake }) {
  const iw = new Int32Array(toHost, 0, 8), ib = new Uint8Array(toHost, RING_HEADER_BYTES);
  const ow = new Int32Array(toGuest, 0, 8), ob = new Uint8Array(toGuest, RING_HEADER_BYTES);
  (async () => {
    for (;;) {
      const tail = Atomics.load(iw, TAIL) >>> 0, head = Atomics.load(iw, HEAD);
      const have = (head - tail) >>> 0;
      const out = Atomics.load(ow, HEAD) >>> 0, read = Atomics.load(ow, TAIL);
      const n = Math.min(have, ob.length - ((out - read) >>> 0));
      if (n) {
        for (let k = 0; k < n; k++) ob[(out + k) & (ob.length - 1)] = ib[(tail + k) & (ib.length - 1)];
        Atomics.store(ow, HEAD, (out + n) | 0);
        Atomics.store(iw, TAIL, (tail + n) | 0);
        Atomics.notify(iw, TAIL);
        wake.postMessage(0);
        continue;
      }
      if (!have && Atomics.load(iw, CLOSED) === 1) {
        Atomics.store(ow, CLOSED, 1);
        wake.postMessage(0);
        return;
      }
      // Nothing to move: until the guest writes, or (the ring full) reads.
      const w = have ? Atomics.waitAsync(ow, TAIL, read, 200) : Atomics.waitAsync(iw, HEAD, head, 200);
      if (w.async) await w.value;
    }
  })();
}

export default async function (t) {
  const { check, eq, timeout, say } = t;
  const arch = t.arch === 'x86' ? 'x86' : 'aarch64';
  let prog;
  try {
    prog = await bytesOf(t.work(`darwin-metalecho.${arch}`));
  } catch {
    t.skip(`no Mac root or no Mac compiler for ${t.arch} (GEX_TEST_DARWIN_ROOT${t.arch === 'x86' ? '_X86' : ''}: pyrus builds one)`);
  }
  // mink's pieces, if the image found a built mink (and this is a browser).
  let mink = null;
  if (typeof Worker === 'function' && typeof navigator === 'object' && navigator.gpu) {
    try {
      mink = {
        framework: await bytesOf(t.work(`mink/Metal.${arch}`)),
        program: await bytesOf(t.work(`mink/triangle.${arch}`)),
        library: await bytesOf(t.work('mink/triangle.metallib')),
        want: await bytesOf(t.work('mink/triangle.out')),
      };
    } catch {
      mink = null;
    }
  }

  const { arch: _, ...debug } = t.debug();
  const env = { PATH: '/usr/bin:/bin', HOME: '/' };
  const sys = new System({ baseUrl: t.baseUrl, arch, debug });
  const fs = new MemoryFilesystem(sys);
  fs.addTar(t.work(`darwin-root.${arch}.tar`));
  fs.addFiles({ '/usr/local/bin/metalecho': prog }, { mode: 0o755 });
  if (mink) {
    fs.addFiles({
      '/System/Library/Frameworks/Metal.framework/Versions/A/Metal': mink.framework,
      '/work/triangle': mink.program,
      '/work/triangle.metallib': mink.library,
    }, { mode: 0o755 });
  }
  // The replayer: the echo, until mink's Worker is asked for.
  let replayer = echo;
  const opens = [];
  const metal = new Metal(sys, { replayer: (c) => replayer(c) });
  metal.addEventListener('open', (e) => opens.push(e.pid));
  await timeout(sys.boot(), 120000, 'boot');
  check('the system has the node', metal.available);

  const r = await timeout(sys.run(['/usr/local/bin/metalecho'], { env, cwd: '/' }), 240000, 'the echo');
  eq('the channel, with an echo at the other end', [r.stdout, r.code], [[
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
  ].join('\n'), 0], r.stderr.trim().split('\n').slice(-4).join(' / '));
  check(`the replayer was handed each open (${opens.length})`, opens.length === 2 && opens.every((p) => p === opens[0] && p > 0), JSON.stringify(opens));

  if (!mink) {
    say(typeof navigator === 'object' && navigator.gpu
      ? 'no built mink beside this tree with a program for this processor (GEX_TEST_MINK; its tools/test-web.sh and tools/test-gexc.sh build what this needs): its program is not run'
      : 'no WebGPU here (Node): mink\'s program is run in a browser');
    await sys.shutdown();
    return;
  }
  // mink's replayer: its Worker, told of each connection.
  const worker = new Worker(t.work('mink/mink-worker.js'), { type: 'module', name: 'mink' });
  let failed = null;
  const ready = new Promise((resolve, reject) => {
    worker.onmessage = (e) => {
      if (e.data.kind === 'ready') resolve();
      else if (e.data.kind === 'failed') {
        failed = e.data.error;
        reject(new Error(failed));
      }
    };
    worker.onerror = (e) => reject(new Error(e.message));
  });
  worker.postMessage({ kind: 'init', glue: new URL(t.work('mink/mink_host.js'), location.href).href });
  await timeout(ready, 60000, 'mink\'s Worker');
  replayer = (c) => worker.postMessage({ kind: 'open', id: c.id, toHost: c.toHost, toGuest: c.toGuest, wake: c.wake }, [c.wake]);
  const d = await timeout(sys.run(['/work/triangle', 'triangle.metallib', 'triangle.out'], { env, cwd: '/work' }), 240000, 'mink\'s triangle');
  let drawn = new Uint8Array(0);
  try {
    drawn = await fs.readFile('/work/triangle.out');
  } catch {}
  // To the byte where the browser's WebGPU is this machine's GPU, as Apple's
  // Metal was; a software adapter (SwiftShader, without --gpu elsewhere)
  // may place an edge's pixels otherwise, so there the picture must be the
  // same but for a few of them.
  let differing = 0, largest = 0;
  for (let i = 0; i < Math.min(drawn.length, mink.want.length); i++) {
    const delta = Math.abs(drawn[i] - mink.want[i]);
    if (delta) differing++;
    if (delta > largest) largest = delta;
  }
  const info = (await navigator.gpu.requestAdapter())?.info;
  const adapter = `${info?.vendor ?? '?'} ${info?.architecture ?? ''} ${info?.description ?? ''}`.trim();
  const software = /swiftshader|llvmpipe|software/i.test(adapter) || info?.isFallbackAdapter === true;
  const close = drawn.length === mink.want.length && (software ? differing <= mink.want.length / 50 : differing === 0);
  check(`mink's triangle, drawn through the node on this browser's WebGPU (${adapter}), is the picture Apple's Metal drew${software ? ' but for edge pixels' : ', to the byte'}`, d.code === 0 && close,
    `exit ${d.code}, ${drawn.length} bytes (want ${mink.want.length}), ${differing} differ (the largest by ${largest}); ${failed ?? ''} ${(d.stdout + d.stderr).trim().split('\n').slice(-4).join(' / ')}`);
  worker.terminate();
  await sys.shutdown();
}
