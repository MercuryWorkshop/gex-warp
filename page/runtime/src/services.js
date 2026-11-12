// `sys.services`: the services the page's shell provides to guest programs
// (notifications first; a program's location, Bluetooth and more to come),
// each through one channel, /dev/gex-services.
//
// A program's framework opens the node once for each service it uses; each
// open is a connection, and what crosses it is frames (the README's
// Services section is their reference):
//
//   u32 LE  J, the JSON text's length   (at most 64 KiB)
//   u32 LE  D, the data's length        (at most 4 MiB)
//   J bytes a JSON object
//   D bytes binary payloads, which the object names as {at, size, type}
//
// The first frame names a service and a version; this answers with the
// version the page's provider speaks, or 0 (then the end) when the page
// provides none of that name: a program cannot tell a page without the
// service from one that refuses it. Every frame after it is the provider's.
// A frame over the limits, or one that is not a JSON object, ends the
// connection. The rings are metal-bridge.mjs's (a header of Int32 words:
// bytes ever written, bytes ever read, closed; then the bytes), read and
// written here on the page's thread without ever waiting on it: a full ring
// toward the program is written later, when the program has read.

import { internals } from './internal.js';

const HEAD = 0, TAIL = 1, CLOSED = 2, HEADER = 32;
export const JSON_MAX = 64 << 10;
export const DATA_MAX = 4 << 20;
const NAME = /^[a-z0-9][a-z0-9.-]{0,63}$/;

/** One program's connection to a service, as its provider sees it. */
export class ServiceConnection extends EventTarget {
  #out; #closed = false;

  constructor({ id, pid, service, version }, out) {
    super();
    Object.assign(this, { id, pid, service, version });
    this.#out = out;
  }

  /** Whether the program still has it open. */
  get closed() {
    return this.#closed;
  }

  /** Send `message` (a plain object, made JSON) and `data` (bytes its
   *  `{at, size}` references name), in order with every other. False once
   *  the connection has closed. Throws a RangeError over the limits. */
  send(message, data = null) {
    if (this.#closed) return false;
    const json = new TextEncoder().encode(JSON.stringify(message));
    const bytes = data ? (data instanceof Uint8Array ? data : new Uint8Array(data)) : new Uint8Array(0);
    if (json.length > JSON_MAX || bytes.length > DATA_MAX) throw new RangeError(`${this.service}: a frame of ${json.length} + ${bytes.length} bytes is over the channel's limits`);
    this.#out(json, bytes);
    return true;
  }

  /** End it: the program reads the end of the stream. */
  close() {
    this.#out(null, null);
  }

  /** (The channel's.) */
  _ended() {
    if (this.#closed) return;
    this.#closed = true;
    this.dispatchEvent(new Event('close'));
  }
}

/** One open of the node, served. */
class Link {
  #c; #r; #w; #wake; #services;
  #in = new Uint8Array(0);
  #pending = [];      // Uint8Arrays to write toward the program
  #ending = false;    // the page closed it: the end after what is pending
  #writing = false;
  conn = null;        // the ServiceConnection, once negotiated
  done = false;

  constructor(c, services) {
    this.#c = c;
    this.#services = services;
    this.#r = { w: new Int32Array(c.toHost, 0, 8), b: new Uint8Array(c.toHost, HEADER) };
    this.#w = { w: new Int32Array(c.toGuest, 0, 8), b: new Uint8Array(c.toGuest, HEADER) };
    this.#wake = c.wake;
    this.#read();
  }

  async #read() {
    const { w, b } = this.#r;
    const cap = b.length;
    for (;;) {
      if (this.done) return;
      const head = Atomics.load(w, HEAD) >>> 0, tail = Atomics.load(w, TAIL) >>> 0;
      const n = (head - tail) >>> 0;
      if (n) {
        const got = new Uint8Array(n), from = tail & (cap - 1), first = Math.min(n, cap - from);
        got.set(b.subarray(from, from + first), 0);
        if (n > first) got.set(b.subarray(0, n - first), first);
        Atomics.store(w, TAIL, (tail + n) | 0);
        Atomics.notify(w, TAIL);
        const all = new Uint8Array(this.#in.length + n);
        all.set(this.#in, 0);
        all.set(got, this.#in.length);
        this.#in = all;
        this.#frames();
        continue;
      }
      if (Atomics.load(w, CLOSED) === 1) return this.#finish();
      const r = Atomics.waitAsync(w, HEAD, head | 0, 1000);
      if (r.async) await r.value;
    }
  }

  #frames() {
    while (this.#in.length >= 8 && !this.done && !this.#ending) {
      const v = new DataView(this.#in.buffer, this.#in.byteOffset, this.#in.length);
      const jl = v.getUint32(0, true), dl = v.getUint32(4, true);
      if (jl > JSON_MAX || dl > DATA_MAX) return this.#refuse(`a frame of ${jl} + ${dl} bytes, over the limits`);
      if (this.#in.length < 8 + jl + dl) return;
      let message;
      try {
        message = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(this.#in.subarray(8, 8 + jl)));
      } catch {
        message = null;
      }
      const data = this.#in.slice(8 + jl, 8 + jl + dl);
      this.#in = this.#in.slice(8 + jl + dl);
      if (!message || typeof message !== 'object' || Array.isArray(message)) return this.#refuse('a frame that is not a JSON object');
      if (!this.conn) this.#negotiate(message);
      else this.conn.dispatchEvent(Object.assign(new Event('message'), { message, data }));
    }
  }

  #negotiate(m) {
    const name = typeof m.service === 'string' ? m.service : '';
    const asked = Number.isInteger(m.version) && m.version > 0 ? m.version : 1;
    const p = NAME.test(name) ? this.#services.provider(name) : null;
    const version = p ? Math.min(asked, p.version) : 0;
    this.#put({ service: name.slice(0, 64), version });
    if (!version) return this.#end();
    const conn = new ServiceConnection({ id: this.#c.id, pid: this.#c.pid, service: name, version }, (json, data) => {
      if (json) this.#put(null, json, data);
      else this.#end();
    });
    this.conn = conn;
    try {
      p.open(conn);
    } catch (e) {
      console.error(`[gex] the ${name} service's provider failed on a connection:`, e);
      this.#end();
    }
  }

  #refuse(why) {
    console.warn(`[gex] /dev/gex-services (pid ${this.#c.pid}): ${why}: the connection is ended`);
    this.#in = new Uint8Array(0);
    this.#end();
  }

  /** Queue a frame toward the program: an object, or JSON bytes and data. */
  #put(message, json = null, data = null) {
    if (this.#ending || this.done) return;
    json ??= new TextEncoder().encode(JSON.stringify(message));
    data ??= new Uint8Array(0);
    const f = new Uint8Array(8 + json.length + data.length);
    const v = new DataView(f.buffer);
    v.setUint32(0, json.length, true);
    v.setUint32(4, data.length, true);
    f.set(json, 8);
    f.set(data, 8 + json.length);
    this.#pending.push(f);
    this.#flush();
  }

  #end() {
    this.#ending = true;
    this.#flush();
  }

  async #flush() {
    if (this.#writing) return;
    this.#writing = true;
    const { w, b } = this.#w;
    const cap = b.length;
    while (this.#pending.length && !this.done) {
      const f = this.#pending[0];
      const head = Atomics.load(w, HEAD) >>> 0, tail = Atomics.load(w, TAIL);
      const room = cap - ((head - tail) >>> 0);
      if (!room) {
        // The program has not read: wait for it to, without holding the page.
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
      if (n === f.length) this.#pending.shift();
      else this.#pending[0] = f.subarray(n);
      this.#wake.postMessage(0);
    }
    this.#writing = false;
    if (this.#ending && !this.#pending.length) {
      Atomics.store(w, CLOSED, 1);
      Atomics.notify(w, HEAD);
      this.#wake.postMessage(0);
    }
  }

  /** The program closed its end (or ended): done with this connection. */
  /** Wake the program's reader again if it has bytes it has not read: a
   *  wake that crossed its parking is then late, not lost. */
  rewake() {
    if (this.done) return;
    const w = this.#w.w;
    if (Atomics.load(w, HEAD) !== Atomics.load(w, TAIL)) {
      try { this.#wake.postMessage(0); } catch {}
    }
  }

  #finish() {
    if (this.done) return;
    this.done = true;
    const { w } = this.#w;
    Atomics.store(w, CLOSED, 1);
    Atomics.notify(w, HEAD);
    try { this.#wake.postMessage(0); } catch {}
    try { this.#wake.close(); } catch {}
    this.conn?._ended();
    this.#services.gone(this);
  }
}

/**
 * The page's side of /dev/gex-services (`sys.services`): the providers it
 * registered and the connections programs made to them.
 */
export class Services {
  #providers = new Map();
  #links = new Set();
  #channel = null;
  #served = null;
  #timer = null;

  constructor(sys) {
    const i = internals(sys, 'Services');
    i.devices(async (dev) => {
      // The single-thread build has no shared memory to make the channel
      // of: its programs find no node, as if every service were refused.
      if (dev.single || typeof SharedArrayBuffer !== 'function' || globalThis.crossOriginIsolated === false) return;
      this.#channel = dev.servicesChannel = await dev.rt.openMetalChannel({ connections: 16, capacity: 1 << 17, toGuestCapacity: 1 << 16 });
      // Served from before the first process, so that an init's first open
      // finds it; the kernel's memory, whose wait queues a reader parks on,
      // is there by the time a program has opened the node.
      this.#served = await dev.rt.serveMetalChannel(this.#channel, {
        kernel: () => i.sys()?.kernel?.memory ?? null,
        replayer: (c) => {
          this.#links.add(new Link(c, this));
          this.#watch();
        },
      });
    }, 1);
  }

  /** While connections are open: every 200 ms, wake the reader of each
   *  that has bytes waiting. (A wake posted as the reader parks may be
   *  missed; natively a device nothing announces is polled each turn.) */
  #watch() {
    if (this.#timer) return;
    this.#timer = setInterval(() => {
      for (const l of this.#links) l.rewake();
      if (!this.#links.size) {
        clearInterval(this.#timer);
        this.#timer = null;
      }
    }, 200);
    this.#timer.unref?.();
  }

  /** Whether programs have the node: false in the single-thread build. */
  get available() {
    return !!this.#channel;
  }

  /**
   * Provide the service `name` (lower-case letters, digits, `.` and `-`):
   * `provider.open(connection)` for each program's connection to it, at
   * `provider.version` (1) or the lower one the program asks for. A
   * function is `open` alone. Returns a function that stops providing it
   * (connections made stay). A second provider of one name throws.
   */
  provide(name, provider) {
    if (typeof name !== 'string' || !NAME.test(name)) throw new TypeError(`services.provide: no service may be named ${JSON.stringify(name)}`);
    const p = typeof provider === 'function' ? { open: provider } : provider;
    if (typeof p?.open !== 'function') throw new TypeError('services.provide: a provider has `open(connection)`');
    const version = p.version ?? 1;
    if (!Number.isInteger(version) || version < 1) throw new TypeError('services.provide: `version` is a whole number from 1');
    if (this.#providers.has(name)) throw new Error(`the ${name} service is already provided`);
    const entry = { open: (c) => p.open(c), version };
    this.#providers.set(name, entry);
    return () => {
      if (this.#providers.get(name) === entry) this.#providers.delete(name);
    };
  }

  /** The names provided. */
  names() {
    return [...this.#providers.keys()];
  }

  /** The open connections, of one service or of all. */
  connections(name) {
    return [...this.#links].map((l) => l.conn).filter((c) => c && !c.closed && (!name || c.service === name));
  }

  /** (Links' and the system's.) */
  provider(name) {
    return this.#providers.get(name) ?? null;
  }
  gone(link) {
    this.#links.delete(link);
  }
  /** Close what a process that ended without closing left open. */
  drop(pid) {
    this.#served?.drop(pid);
  }
}
