// A page helper's instance of the global kernel says its "process" memory
// has no page write map (`k_no_write_map`, gex-kmem `no_write_map`): after
// every copy into its caller's memory the kernel reads eight bytes at a
// fixed place in it (gex_abi's WRITE_MAP_SLOT, just under a megabyte) and,
// when they are not zero, marks the pages it wrote with a 1 a page at the
// address they make. In a helper's scratch (a megabyte or more: a drop's,
// the open-files ask's) or the X server's own wasm memory, those bytes are
// the helper's, and the 1s land among them. Here a helper instance reads a
// file into its scratch with an "address" at the place: marked through
// without the call, untouched after it.
import { readFileSync, existsSync } from 'node:fs';
import { kernelThread, lazyFetch, processMemories } from '../../hosts/browser/js/lazy-fetch.mjs';
import { pageFs } from '../../hosts/browser/js/page-fs.mjs';

export const meta = { hosts: ['node'], timeout: 60 };

const SLOT = 0x100000 - 32;
const MAP = 0x80000;
const BUF = 0x20000;
const MARK = MAP + (BUF >> 12);

export default async function (t) {
  const { check, eq } = t;
  const file = new URL('../../page/public/gex/gex_kernel.wasm', import.meta.url);
  if (!existsSync(file)) return t.skip('page/public/gex/gex_kernel.wasm is not built (hosts/kernel/build.sh)');
  const module = await WebAssembly.compile(readFileSync(file));
  const memory = new WebAssembly.Memory({ initial: 2048, maximum: 65536, shared: true });
  // A helper's scratch: sixteen pages, as kernel-helper.js makes one.
  const pm = new WebAssembly.Memory({ initial: 16, maximum: 65535, shared: true });
  const k = (await WebAssembly.instantiate(module, {
    env: { memory, ...processMemories(pm), lazy_fetch: lazyFetch(memory), block_io: () => -1, page_fs: pageFs(memory) },
  })).exports;
  if (!k.k_no_write_map) return t.skip('this kernel has no k_no_write_map: rebuild it');
  const P = new Uint8Array(pm.buffer);
  const put = (s, at) => { const b = new TextEncoder().encode(s); P.set(b, at); return b.length; };
  kernelThread(k, (k.k_stack_area() >>> 0) + (k.k_stack_size() >>> 0));
  eq('a directory', k.k_fs_mkdir(0x100n, put('/t', 0x100), 0o755), 0);
  k.k_fs_populate_done?.();
  const name = put('/t/f', 0x100);
  const w = k.k_fs_open(0x100n, name, 0o1101n, 0o644); // O_WRONLY|O_CREAT|O_TRUNC
  P.fill(0x61, 0x10000, 0x11000);
  eq('a file written', k.k_fs_write(w, 0n, 0x10000n, 0x1000), 0x1000);
  k.k_fs_close(w);
  const read = () => {
    // What the helper left at the slot: an "address".
    new DataView(pm.buffer).setBigUint64(SLOT, BigInt(MAP), true);
    P.fill(0, MAP, MAP + 0x100);
    const r = k.k_fs_open(0x100n, name, 0n, 0);
    const n = k.k_fs_read(r, 0n, BigInt(BUF), 0x1000);
    k.k_fs_close(r);
    return { n, mark: P[MARK], bytes: P[BUF] };
  };
  const before = read();
  eq('read before', [before.n, before.bytes], [0x1000, 0x61]);
  check('without the call, the bytes at the slot are taken for a map and marked through', before.mark === 1, JSON.stringify(before));
  eq('the call', k.k_no_write_map(), 0);
  const after = read();
  eq('read after', [after.n, after.bytes], [0x1000, 0x61]);
  eq('after it, nothing is marked', P.subarray(MAP, MAP + 0x100).some((b) => b !== 0), false);
}
