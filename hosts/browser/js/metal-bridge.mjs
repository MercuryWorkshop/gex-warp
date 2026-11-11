// /dev/gex-metal on a page: the channel between a guest that has the node
// open and the page's Metal replayer.
//
// Natively the node is a pipe to a program gexc starts (hosts/native
// metal.rs). On a page it is the same thing made of shared memory: each open
// is a byte stream each way, and the other end is a replayer the page's
// embedder supplies (mink's, say, in a Worker of its own). Nothing here looks
// at the bytes.
//
// The channel is made once, before the processes start, and every Worker of
// every process is handed it: a control buffer and a small pool of
// connections, each two rings.
//
// A ring is a SharedArrayBuffer: a header of 8 Int32 words, then the bytes.
//
//   word 0  HEAD      bytes ever written (mod 2^32); the writer stores it
//                     and notifies on it
//   word 1  TAIL      bytes ever read (mod 2^32); the reader stores it and
//                     notifies on it
//   word 2  CLOSED    1 once the writer has gone: a reader sees the end of
//                     the stream when the ring is empty
//   word 3  CAPACITY  the number of bytes, a power of two
//
// The control buffer, Int32 words:
//
//   word 0  HOST   0 no replayer yet, 1 one serves the channel, -1 it failed
//   word 1  OPENS  bumped and notified on at each open: what the page's side
//                  waits on to learn of one
//   and per connection, 8 words from word 16:
//   +0  STATE   0 free, 1 open, 2 closed by the guest
//   +1  SEQ     bumped at each open of this connection
//   +2  QUEUE   where the opener's wait queue is in the global kernel's
//   +3  SLOTS   memory, and where the wait slots start: how a reader parked
//               in the kernel is woken from another realm
//   +4  PID     the opener
//   +5  SERVED  the SEQ the page's side has handed to the replayer
//
// The guest's half (`MetalProxy`) is called from inside a system call, in
// whichever Worker the calling thread is: it has no state of its own beyond
// the buffers. It never waits for the replayer: a read of an empty ring says
// so, and the kernel parks the thread on its process's wait queue
// (hosts/browser/src/metal.rs). The page's half (`serveMetal`) hands each new
// connection to the replayer with a MessagePort the replayer posts to
// whenever it has written bytes for the guest; a message on it walks the
// opener's wait queue, which is what wakes the parked reader. A replayer
// needs to know only the ring layout above and that port.

import { notifyQueue } from './wl-bridge.mjs';

export const HEAD = 0, TAIL = 1, CLOSED = 2, CAPACITY = 3;
export const RING_HEADER_BYTES = 32;
const HOST = 0, OPENS = 1;
const SLOT_BASE = 16, SLOT_WORDS = 8;
const STATE = 0, SEQ = 1, QUEUE = 2, WSLOTS = 3, PID = 4, SERVED = 5;
const FREE = 0, OPEN = 1, SHUT = 2;

/** The channel: `connections` streams at once (each open of the node is
 *  one), `capacity` bytes in each ring. */
export function makeMetalChannel({ connections = 8, capacity = 1 << 21 } = {}) {
  if (capacity & (capacity - 1)) throw new Error('a ring\'s capacity is a power of two');
  const ring = () => {
    const b = new SharedArrayBuffer(RING_HEADER_BYTES + capacity);
    new Int32Array(b, 0, 8)[CAPACITY] = capacity;
    return b;
  };
  const control = new SharedArrayBuffer((SLOT_BASE + SLOT_WORDS * connections) * 4);
  return { control, rings: Array.from({ length: connections }, () => ({ toHost: ring(), toGuest: ring() })) };
}

const words = (b) => new Int32Array(b, 0, 8);
const bytes = (b) => new Uint8Array(b, RING_HEADER_BYTES);
const used = (w) => (Atomics.load(w, HEAD) - Atomics.load(w, TAIL)) >>> 0;

/** Say whether a replayer serves the channel (1), none does yet (0) or it
 *  failed (-1): an open while it is not 1 is refused. */
export function setMetalHost(channel, state) {
  const ctl = new Int32Array(channel.control);
  Atomics.store(ctl, HOST, state);
  Atomics.notify(ctl, HOST);
}

/** The guest's half. Every method is called from a system call's import. */
export class MetalProxy {
  #ctl; #rings;

  constructor(channel) {
    this.#ctl = new Int32Array(channel.control);
    this.#rings = channel.rings.map(({ toHost, toGuest }) => ({
      ow: words(toHost), ob: bytes(toHost), iw: words(toGuest), ib: bytes(toGuest),
    }));
  }

  /** A connection for process `pid`, whose wait queue is at `wake`
   *  (`gex.kernelWake()`): its number, or -1 with no replayer or none free. */
  open(pid, wake) {
    const ctl = this.#ctl;
    if (Atomics.load(ctl, HOST) !== 1 || !wake?.addr) return -1;
    for (let i = 0; i < this.#rings.length; i++) {
      const at = SLOT_BASE + i * SLOT_WORDS, r = this.#rings[i];
      // Free, or closed by its last guest and finished with by the replayer.
      const state = Atomics.load(ctl, at + STATE);
      if (state === OPEN || (state === SHUT && Atomics.load(r.iw, CLOSED) !== 1)) continue;
      if (Atomics.compareExchange(ctl, at + STATE, state, OPEN) !== state) continue;
      for (const w of [r.ow, r.iw]) {
        Atomics.store(w, HEAD, 0);
        Atomics.store(w, TAIL, 0);
        Atomics.store(w, CLOSED, 0);
      }
      Atomics.store(ctl, at + QUEUE, wake.addr | 0);
      Atomics.store(ctl, at + WSLOTS, wake.slots | 0);
      Atomics.store(ctl, at + PID, pid | 0);
      Atomics.add(ctl, at + SEQ, 1);
      Atomics.add(ctl, OPENS, 1);
      Atomics.notify(ctl, OPENS);
      return i;
    }
    return -1;
  }

  /** Up to `len` bytes of what the replayer wrote, into `memory` at `ptr`:
   *  how many, 0 when there are none yet, -1 at the end of the stream. */
  read(memory, slot, ptr, len) {
    const r = this.#rings[slot];
    if (!r) return -1;
    const tail = Atomics.load(r.iw, TAIL) >>> 0;
    const n = Math.min((Atomics.load(r.iw, HEAD) - tail) >>> 0, len);
    if (!n) return Atomics.load(r.iw, CLOSED) === 1 && !used(r.iw) ? -1 : 0;
    const cap = r.ib.length, from = tail & (cap - 1), first = Math.min(n, cap - from);
    const into = new Uint8Array(memory.buffer, ptr, n);
    into.set(r.ib.subarray(from, from + first), 0);
    if (n > first) into.set(r.ib.subarray(0, n - first), first);
    Atomics.store(r.iw, TAIL, (tail + n) | 0);
    Atomics.notify(r.iw, TAIL);
    return n;
  }

  /** `len` bytes from `memory` at `ptr` to the replayer: how many were
   *  taken, or -1 if it has gone. A full ring is waited on, briefly: the
   *  replayer takes what is written without waiting for anything of the
   *  guest's, so a full ring empties. */
  write(memory, slot, ptr, len) {
    const r = this.#rings[slot];
    if (!r) return -1;
    const cap = r.ob.length;
    let done = 0;
    const deadline = Date.now() + 30000;
    while (done < len) {
      if (Atomics.load(r.iw, CLOSED) === 1 || Atomics.load(this.#ctl, HOST) !== 1) return done || -1;
      const head = Atomics.load(r.ow, HEAD) >>> 0, tail = Atomics.load(r.ow, TAIL);
      const n = Math.min(cap - ((head - tail) >>> 0), len - done);
      if (!n) {
        if (Date.now() > deadline) break;
        Atomics.wait(r.ow, TAIL, tail, 50);
        continue;
      }
      const at = head & (cap - 1), first = Math.min(n, cap - at);
      const from = new Uint8Array(memory.buffer, ptr + done, n);
      r.ob.set(from.subarray(0, first), at);
      if (n > first) r.ob.set(from.subarray(first, n), 0);
      Atomics.store(r.ow, HEAD, (head + n) | 0);
      Atomics.notify(r.ow, HEAD);
      done += n;
    }
    return done;
  }

  /** Bit 0: a read would not wait (bytes, or the end); bit 1: the end. */
  ready(slot) {
    const r = this.#rings[slot];
    if (!r) return 3;
    if (used(r.iw)) return 1;
    return Atomics.load(r.iw, CLOSED) === 1 ? 3 : 0;
  }

  /** Bytes a read would get now. */
  available(slot) {
    const r = this.#rings[slot];
    return r ? used(r.iw) : 0;
  }

  close(slot) {
    const r = this.#rings[slot];
    if (!r) return;
    const at = SLOT_BASE + slot * SLOT_WORDS;
    if (Atomics.compareExchange(this.#ctl, at + STATE, OPEN, SHUT) !== OPEN) return;
    Atomics.store(r.ow, CLOSED, 1);
    Atomics.notify(r.ow, HEAD);
    Atomics.notify(r.ow, TAIL);
  }
}

/**
 * The page's half: tells the replayer of each connection a guest opens and
 * wakes the guest's readers when the replayer says it wrote.
 *
 * `replayer(connection)` is called once per open with `{ id, pid, toHost,
 * toGuest, wake }`: the two rings (read `toHost`, write `toGuest`, set
 * `toGuest`'s CLOSED when done with a connection whose `toHost` closed) and
 * a MessagePort to post anything to after writing. `kernel()` answers the
 * global kernel's memory, whose wait queues are walked. Returns
 * `{ stop(), drop(pid) }`: `drop` closes what a process that ended without
 * closing left open.
 */
export function serveMetal(channel, { replayer, kernel }) {
  const ctl = new Int32Array(channel.control);
  let stopped = false, opened = 0;
  const live = new Map();   // slot -> its wake port, this side
  const announce = (i) => {
    const at = SLOT_BASE + i * SLOT_WORDS;
    const seq = Atomics.load(ctl, at + SEQ);
    if (Atomics.load(ctl, at + STATE) === FREE || Atomics.load(ctl, at + SERVED) === seq) return;
    Atomics.store(ctl, at + SERVED, seq);
    live.get(i)?.close();
    const queue = Atomics.load(ctl, at + QUEUE) >>> 0, slots = Atomics.load(ctl, at + WSLOTS) >>> 0;
    const { port1, port2 } = new MessageChannel();
    const wake = () => {
      const memory = kernel();
      if (memory) notifyQueue(new Int32Array(memory.buffer), queue, slots);
    };
    port1.onmessage = wake;
    live.set(i, port1);
    const { toHost, toGuest } = channel.rings[i];
    try {
      replayer({ id: ++opened, pid: Atomics.load(ctl, at + PID), toHost, toGuest, wake: port2 });
    } catch (e) {
      // No replayer for this one: its reader sees the end of the stream.
      const w = words(toGuest);
      Atomics.store(w, CLOSED, 1);
      wake();
      console.error('[gex] the Metal replayer refused a connection:', e);
    }
  };
  (async () => {
    while (!stopped) {
      const seen = Atomics.load(ctl, OPENS);
      for (let i = 0; i < channel.rings.length; i++) announce(i);
      if (Atomics.load(ctl, OPENS) !== seen) continue;
      const w = Atomics.waitAsync(ctl, OPENS, seen, 1000);
      if (w.async) await w.value;
    }
  })();
  return {
    stop() {
      stopped = true;
      for (const p of live.values()) p.close();
      live.clear();
    },
    drop(pid) {
      for (let i = 0; i < channel.rings.length; i++) {
        const at = SLOT_BASE + i * SLOT_WORDS;
        if (Atomics.load(ctl, at + PID) !== pid || Atomics.compareExchange(ctl, at + STATE, OPEN, SHUT) !== OPEN) continue;
        const w = words(channel.rings[i].toHost);
        Atomics.store(w, CLOSED, 1);
        Atomics.notify(w, HEAD);
      }
    },
  };
}
