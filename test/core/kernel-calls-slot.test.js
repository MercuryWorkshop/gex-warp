// The page's kernel calls lay their arguments in a scratch memory that the
// kernel takes for its caller's process memory, and the kernel reads a
// process's page write map from a fixed place in it (gex_abi's
// WRITE_MAP_SLOT) after every copy it makes there: what a batch leaves in
// that place must be nothing, or the kernel marks "pages" through it (a
// program's output read by the page had its second byte made 1). Nothing a
// batch lays out covers the place, and it is zero when each call is made.
import { runCalls } from '../../page/runtime/src/kernel-calls.js';

export const meta = { hosts: ['node'], timeout: 30 };

const SLOT = 0x100000 - 32;

export default async function (t) {
  const { check, eq } = t;
  const mem = new WebAssembly.Memory({ initial: 32 });
  // What an earlier batch left: ones all over, the slot among them.
  new Uint8Array(mem.buffer).fill(1);
  const seen = [];
  const slot = () => [...new Uint8Array(mem.buffer, SLOT, 8)];
  const k = {
    // (in: pointer and length; out: pointer and capacity)
    k_in: (at, len) => { seen.push({ f: 'in', at: Number(at), len, slot: slot() }); return 0; },
    k_out: (at, cap) => {
      seen.push({ f: 'out', at: Number(at), len: cap, slot: slot() });
      new Uint8Array(mem.buffer, Number(at), 5).set(new TextEncoder().encode('hello'));
      return 5;
    },
  };
  // A megabyte and a half in, which would lie over the slot from the
  // memory's start; then a read, as a program's output is read.
  const big = new Uint8Array(1536 * 1024).fill(7);
  const { value } = runCalls(k, mem, [{ f: 'k_in', a: [{ in: big }] }, { f: 'k_out', a: [{ out: 1 << 18 }] }, { f: 'k_in', a: [{ in: new Uint8Array(900 * 1024) }] }]);
  eq('each call was made', seen.map((s) => s.f), ['in', 'out', 'in']);
  check('nothing laid out covers the write map\'s place', seen.every((s) => s.at + s.len <= SLOT || s.at >= SLOT + 8), JSON.stringify(seen.map(({ at, len }) => [at, len])));
  check('which is zero at each call', seen.every((s) => s.slot.every((b) => b === 0)), JSON.stringify(seen.map((s) => s.slot)));
  eq('what was read', new TextDecoder().decode(value[1].out[0]), 'hello');
  // And a fresh batch clears what the last one's results left around it.
  new Uint8Array(mem.buffer, SLOT, 8).fill(9);
  seen.length = 0;
  runCalls(k, mem, [{ f: 'k_out', a: [{ out: 16 }] }]);
  eq('zero again in the next batch', seen[0].slot, [0, 0, 0, 0, 0, 0, 0, 0]);
}
