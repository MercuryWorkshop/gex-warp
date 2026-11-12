// One connection of /dev/gex-services, served: the frames of a pair of rings
// (metal-bridge.mjs's layout: a header of Int32 words, bytes ever written,
// bytes ever read, closed; then the bytes), read and written without ever
// waiting on the thread it runs on, which may be the page's or a Worker's
// (services.js hands a connection to a Worker whose provider asks for one;
// services-worker.js serves it there). Nothing here touches the DOM.
//
// Copies: a frame from the program is read from its ring straight into one
// buffer (its JSON and its data, which the provider is handed as a view of
// it); a frame to the program is written from the provider's own bytes
// straight into the ring, and the provider must not change them until the
// connection's `bufferedAmount` says they have gone. With the program's own
// copy at each end (its write into the ring, its read out of it), a payload
// is copied twice on its way, whichever way it goes.

const HEAD = 0, TAIL = 1, CLOSED = 2, HEADER = 32;
export const JSON_MAX = 64 << 10;
export const DATA_MAX = 4 << 20;
/** The most a frame on a bulk connection (/dev/gex-services-bulk) carries. */
export const BULK_DATA_MAX = 64 << 20;

// Waking a program's reader parked in the kernel: its process's wait queue
// walked in the kernel's memory, as wl-bridge.mjs `notifyQueue` walks it
// (64 words of bits, a wait slot for each), from whichever realm wrote. Where
// the memory is not to hand, the connection's port does it on the page's
// thread (metal-bridge.mjs `serveMetal`).
function wakeQueue(memory, queue, slots) {
  const w = new Int32Array(memory.buffer);
  for (let i = 0; i < 64; i++) {
    let bits = Atomics.load(w, (queue >>> 2) + i) >>> 0;
    while (bits) {
      const b = 31 - Math.clz32(bits & -bits);
      bits &= bits - 1;
      const at = (slots >>> 2) + i * 32 + b;
      Atomics.add(w, at, 1);
      Atomics.notify(w, at);
    }
  }
}

const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8', { fatal: true });

/** One program's connection to a service, as its provider sees it.
 *  Events: `message` (`message`, `data`), `drain` (what was sent has all
 *  gone into the program's ring), `close`. */
export class ServiceConnection extends EventTarget {
  #link; #closed = false;

  constructor({ id, pid, service, version, bulk }, link) {
    super();
    Object.assign(this, { id, pid, service, version, bulk: !!bulk });
    this.#link = link;
  }

  /** Whether the program still has it open. */
  get closed() {
    return this.#closed;
  }

  /** Bytes sent and not yet taken into the program's ring: what a provider
   *  that produces faster than the program reads waits on (`drain`). */
  get bufferedAmount() {
    return this.#link.buffered;
  }

  /** Send `message` (a plain object, made JSON) and `data` (bytes its
   *  `{at, size}` references name), in order with every other. `data` is
   *  not copied until it is written into the ring: do not change it while
   *  `bufferedAmount` counts it. False once the connection has closed.
   *  Throws a RangeError over the limits (a bulk connection's data may be
   *  larger). */
  send(message, data = null) {
    if (this.#closed) return false;
    const json = encoder.encode(JSON.stringify(message));
    const bytes = data ? (data instanceof Uint8Array ? data : ArrayBuffer.isView(data) ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength) : new Uint8Array(data)) : null;
    const max = this.bulk ? BULK_DATA_MAX : DATA_MAX;
    if (json.length > JSON_MAX || (bytes?.length ?? 0) > max) throw new RangeError(`${this.service}: a frame of ${json.length} + ${bytes?.length ?? 0} bytes is over the channel's limits`);
    this.#link.put(json, bytes);
    return true;
  }

  /** End it: the program reads the end of the stream. */
  close() {
    this.#link.end();
  }

  /** (The link's.) */
  _ended() {
    if (this.#closed) return;
    this.#closed = true;
    this.dispatchEvent(new Event('close'));
  }
}

// Every live link of this realm, woken again every 200 ms while it has bytes
// its program has not read: a wake posted as the reader parks may be missed,
// and is then late rather than lost (natively a device nothing announces is
// polled each turn).
const live = new Set();
let timer = null;
function watch(link) {
  live.add(link);
  if (timer) return;
  timer = setInterval(() => {
    for (const l of live) l.rewake();
    if (!live.size) {
      clearInterval(timer);
      timer = null;
    }
  }, 200);
  timer.unref?.();
}

/**
 * The served side of one connection: `c` is `{ id, pid, toHost, toGuest,
 * wake }` (metal-bridge.mjs `serveMetal`'s); `first(message, link)` is
 * called with the first frame (the negotiation) and returns nothing, having
 * called `link.answer` or `link.handOff`; `gone(link)` once it is over.
 * With `negotiated` (`{ service, version, provider, bulk }`) the first frame
 * was read elsewhere and the provider is opened at once; `pending` is what
 * was read after it.
 */
export class Link {
  #c; #r; #w; #wakePort; #first; #gone;
  #head = new Uint8Array(8); #headAt = 0;  // a frame's header, as it comes
  #frame = null; #frameAt = 0; #jl = 0;    // its body, as it comes
  #queue = [];                              // Uint8Arrays to write toward the program
  buffered = 0;
  #ending = false;
  #writing = false;
  conn = null;
  done = false;
  handedOff = false;

  constructor(c, { first, gone, negotiated = null, pending = null }) {
    this.#c = c;
    this.#first = first;
    this.#gone = gone;
    this.#r = { w: new Int32Array(c.toHost, 0, 8), b: new Uint8Array(c.toHost, HEADER) };
    this.#w = { w: new Int32Array(c.toGuest, 0, 8), b: new Uint8Array(c.toGuest, HEADER) };
    this.#wakePort = c.wake;
    watch(this);
    if (negotiated) this.open(negotiated);
    if (pending?.length) this.#take(pending, 0, pending.length);
    this.#read();
  }

  /** Wake the program's reader: directly, with the kernel's memory (`c.memory`,
   *  or a function answering it) and the queue's place; else by the port. */
  #wake() {
    const c = this.#c;
    const memory = typeof c.memory === 'function' ? c.memory() : c.memory;
    if (memory && c.queue != null) wakeQueue(memory, c.queue, c.slots);
    else c.wake.postMessage(0);
  }

  get id() { return this.#c.id; }
  get pid() { return this.#c.pid; }

  /** The provider's side opened, speaking `version`. */
  open({ service, version, provider, bulk }) {
    this.conn = new ServiceConnection({ id: this.#c.id, pid: this.#c.pid, service, version, bulk }, this);
    try {
      provider.open(this.conn);
    } catch (e) {
      console.error(`[gex] the ${service} service's provider failed on a connection:`, e);
      this.end();
    }
  }

  /** Answer the negotiation: `version` 0 ends the connection. */
  answer(service, version, opened) {
    this.put(encoder.encode(JSON.stringify({ service: service.slice(0, 64), version })), null);
    if (!version) this.end();
    else opened?.();
  }

  /** Stop serving it here, leaving what is unread for another realm: the
   *  bytes read past the first frame. */
  handOff() {
    this.handedOff = true;
    this.done = true;
    live.delete(this);
    this.#gone(this);
    return this.#pendingBytes();
  }

  #pendingBytes() {
    // (Only whole bytes past the negotiation are possible here: the first
    // frame was just taken, and what followed it in the same read is in the
    // header or body under way.)
    const out = [];
    if (this.#headAt) out.push(this.#head.slice(0, this.#headAt));
    if (this.#frame) out.push(this.#frame.slice(0, this.#frameAt));
    const n = out.reduce((s, b) => s + b.length, 0), all = new Uint8Array(n);
    let at = 0;
    for (const b of out) { all.set(b, at); at += b.length; }
    return all;
  }

  async #read() {
    const { w, b } = this.#r;
    const cap = b.length;
    for (;;) {
      if (this.done) return;
      const head = Atomics.load(w, HEAD) >>> 0, tail = Atomics.load(w, TAIL) >>> 0;
      const n = (head - tail) >>> 0;
      if (n) {
        const from = tail & (cap - 1), first = Math.min(n, cap - from);
        // Taken a piece at a time, so that a frame's bytes go straight from
        // the ring into its buffer.
        let used = this.#take(b, from, from + first);
        if (used === first && n > first) used += this.#take(b, 0, n - first);
        Atomics.store(w, TAIL, (tail + used) | 0);
        Atomics.notify(w, TAIL);
        continue;
      }
      if (Atomics.load(w, CLOSED) === 1) return this.#finish();
      const r = Atomics.waitAsync(w, HEAD, head | 0, 1000);
      if (r.async) await r.value;
    }
  }

  /** Take bytes `src[from, to)` into the frame under way: how many were
   *  taken (fewer only once the connection is done or handed off). */
  #take(src, from, to) {
    let at = from;
    while (at < to && !this.done) {
      if (!this.#frame) {
        const k = Math.min(8 - this.#headAt, to - at);
        this.#head.set(src.subarray(at, at + k), this.#headAt);
        this.#headAt += k;
        at += k;
        if (this.#headAt < 8) break;
        const v = new DataView(this.#head.buffer);
        const jl = v.getUint32(0, true), dl = v.getUint32(4, true);
        const max = this.conn?.bulk ? BULK_DATA_MAX : DATA_MAX;
        if (jl > JSON_MAX || dl > max) {
          this.#refuse(`a frame of ${jl} + ${dl} bytes, over the limits`);
          return to - from;
        }
        this.#headAt = 0;
        this.#jl = jl;
        this.#frame = new Uint8Array(jl + dl);
        this.#frameAt = 0;
      }
      const k = Math.min(this.#frame.length - this.#frameAt, to - at);
      this.#frame.set(src.subarray(at, at + k), this.#frameAt);
      this.#frameAt += k;
      at += k;
      if (this.#frameAt === this.#frame.length) {
        const f = this.#frame;
        this.#frame = null;
        this.#deliver(f);
      }
    }
    return at - from;
  }

  #deliver(f) {
    let message;
    try {
      message = JSON.parse(decoder.decode(f.subarray(0, this.#jl)));
    } catch {
      message = null;
    }
    if (!message || typeof message !== 'object' || Array.isArray(message)) return this.#refuse('a frame that is not a JSON object');
    const data = f.subarray(this.#jl);
    if (!this.conn) this.#first(message, this);
    else this.conn.dispatchEvent(Object.assign(new Event('message'), { message, data }));
  }

  #refuse(why) {
    console.warn(`[gex] /dev/gex-services (pid ${this.#c.pid}): ${why}: the connection is ended`);
    this.#frame = null;
    this.#headAt = 0;
    this.end();
  }

  /** Queue a frame toward the program: its JSON's bytes, and its data. */
  put(json, data) {
    if (this.#ending || this.done) return;
    const h = new Uint8Array(8 + json.length);
    const v = new DataView(h.buffer);
    v.setUint32(0, json.length, true);
    v.setUint32(4, data?.length ?? 0, true);
    h.set(json, 8);
    this.#queue.push(h);
    this.buffered += h.length;
    if (data?.length) {
      this.#queue.push(data);
      this.buffered += data.length;
    }
    this.#flush();
  }

  end() {
    this.#ending = true;
    this.#flush();
  }

  async #flush() {
    if (this.#writing) return;
    this.#writing = true;
    const { w, b } = this.#w;
    const cap = b.length;
    const had = this.buffered;
    while (this.#queue.length && !this.done) {
      const f = this.#queue[0];
      const head = Atomics.load(w, HEAD) >>> 0, tail = Atomics.load(w, TAIL);
      const room = cap - ((head - tail) >>> 0);
      if (!room) {
        // The program has not read: wait for it to, without holding this thread.
        const r = Atomics.waitAsync(w, TAIL, tail, 1000);
        if (r.async) await r.value;
        if (Atomics.load(this.#r.w, CLOSED) === 1 && Atomics.load(w, TAIL) === tail) break;
        continue;
      }
      const n = Math.min(room, f.length), at = head & (cap - 1), first = Math.min(n, cap - at);
      b.set(f.subarray(0, first), at);
      if (n > first) b.set(f.subarray(first, n), 0);
      Atomics.store(w, HEAD, (head + n) | 0);
      Atomics.notify(w, HEAD);
      this.buffered -= n;
      if (n === f.length) this.#queue.shift();
      else this.#queue[0] = f.subarray(n);
      this.#wake();
    }
    this.#writing = false;
    if (had && !this.buffered && this.conn && !this.done) this.conn.dispatchEvent(new Event('drain'));
    if (this.#ending && !this.#queue.length && !this.done) {
      Atomics.store(w, CLOSED, 1);
      Atomics.notify(w, HEAD);
      this.#wake();
      // Its end is said, and that also tells the program's side that the
      // slot is free: it may open another connection on these same rings at
      // once, so this one touches them no more (it reads nothing further
      // and never marks them again).
      this.#over();
    }
  }

  /** Wake the program's reader again if it has bytes it has not read. */
  rewake() {
    if (this.done) return;
    const w = this.#w.w;
    if (Atomics.load(w, HEAD) !== Atomics.load(w, TAIL)) {
      try { this.#wake(); } catch {}
    }
  }

  /** The program closed its end (or ended): done with this connection,
   *  which the end said toward the program frees for another. */
  #finish() {
    if (this.done) return;
    const { w } = this.#w;
    Atomics.store(w, CLOSED, 1);
    Atomics.notify(w, HEAD);
    try { this.#wake(); } catch {}
    this.#over();
  }

  #over() {
    this.done = true;
    live.delete(this);
    try { this.#wakePort.close(); } catch {}
    this.conn?._ended();
    this.#gone(this);
  }
}
