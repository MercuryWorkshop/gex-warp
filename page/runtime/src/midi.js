// `Midi`: programs' MIDI, through the browser's Web MIDI. A service of
// /dev/gex-services (services.js): a program's framework (pyrus's
// CoreMIDI; a Linux or Windows bridge later) connects to `midi`, and sees
// the browser's MIDI inputs and outputs as devices, hears when they come and
// go, receives what the inputs send and sends to the outputs. The README's
// MIDI section is the reference for the messages.
//
// The browser asks the user for access when a page calls
// `navigator.requestMIDIAccess()`, and for a second permission for system
// exclusive messages (`{ sysex: true }`): this asks for the first when a
// program first connects, and for the second only when a program sends a
// system exclusive message. Everything a program sends is the program's
// word: ports are only those the browser listed, and what is sent to one is
// the browser's to accept or refuse.

import { internals } from './internal.js';
import { cleanText } from './notifications.js';

/** What a shell takes of what a program sends. */
export const LIMITS = Object.freeze({ message: 4 << 20, name: 256, id: 256 });

/** A port as the program is told it (frozen, bounded text). */
function snapshot(p) {
  return Object.freeze({
    id: cleanText(String(p.id ?? ''), LIMITS.id),
    name: cleanText(String(p.name ?? ''), LIMITS.name),
    manufacturer: cleanText(String(p.manufacturer ?? ''), LIMITS.name),
    version: cleanText(String(p.version ?? ''), LIMITS.name),
    type: p.type === 'input' ? 'input' : 'output',
    state: p.state === 'disconnected' ? 'disconnected' : 'connected',
    connection: typeof p.connection === 'string' ? p.connection : 'closed',
  });
}

/** The values of a Map-like or iterable the browser gives. */
function valuesOf(m) {
  if (!m) return [];
  if (typeof m.values === 'function') return [...m.values()];
  return [...m];
}

/**
 * The page's shell gives programs the browser's MIDI. One per system; made
 * any time.
 *
 * `requestMIDIAccess(options)` is the browser's `navigator.requestMIDIAccess`
 * (what the shell has by default; a test gives its own), `authorize(request)`
 * decides whether a program may have MIDI (`request` is `{ pid, sysex }`,
 * `sysex` when it is the second permission; it returns or resolves a
 * boolean, and a throw is a refusal; without it every program is allowed).
 * `sysex: 'never'` refuses system exclusive messages without asking the
 * browser, `'always'` asks for the permission with the first.
 *
 * Events: `connect` and `disconnect` (`pid`), `access` (`pid`, `sysex`,
 * `granted`), `send` (`pid`, `port`, `data`, `at`, the message a program
 * sent and the browser time it is for, or null), `listen` (`pid`, `port`,
 * `on`).
 */
export class Midi extends EventTarget {
  #request;
  #authorize;
  #sysexMode;
  #conns = new Set();  // { conn, pid, hello, listening: Set of port ids }
  #access = null;      // the MIDIAccess in use
  #pending = null;     // the promise of the first
  #sysexGranted = false;
  #sysexPending = null;
  #ports = new Map();  // id -> port as the browser has it
  #hooked = new Set(); // inputs whose messages are listened to
  #attached = new WeakSet(); // the port objects a listener is on (a new access has new ones)

  constructor(sys, { requestMIDIAccess = null, authorize = null, sysex = 'ask' } = {}) {
    super();
    if (authorize != null && typeof authorize !== 'function') throw new TypeError('Midi: `authorize` is a function');
    if (!['ask', 'never', 'always'].includes(sysex)) throw new TypeError("Midi: `sysex` is 'ask', 'never' or 'always'");
    const i = internals(sys, 'Midi');
    i.attach('midi', this);
    this.#request = requestMIDIAccess ?? (globalThis.navigator?.requestMIDIAccess ? (o) => globalThis.navigator.requestMIDIAccess(o) : null);
    this.#authorize = authorize;
    this.#sysexMode = sysex;
    sys.services.provide('midi', { version: 1, open: (conn) => this.#open(conn) });
  }

  /** The ports the browser has, as programs are told them. */
  ports() {
    return [...this.#ports.values()].map(snapshot);
  }

  /** The programs connected: their pids. */
  programs() {
    return [...this.#conns].map((c) => c.pid);
  }

  #emit(type, props) {
    this.dispatchEvent(Object.assign(new Event(type), props));
  }

  #open(conn) {
    const c = { conn, pid: conn.pid, hello: false, listening: new Set() };
    this.#conns.add(c);
    conn.addEventListener('message', ({ message, data }) => {
      this.#message(c, message, data).catch((e) => console.error('[gex] midi: a message the service could not take:', e));
    });
    conn.addEventListener('close', () => {
      this.#conns.delete(c);
      this.#emit('disconnect', { pid: c.pid });
    });
  }

  /** The browser's access, asked for once (and again for system exclusive). */
  async #acquire(sysex) {
    if (!this.#request) return null;
    if (sysex) {
      if (this.#sysexGranted) return this.#access;
      this.#sysexPending ??= (async () => {
        const access = await this.#request({ sysex: true });
        this.#sysexGranted = true;
        this.#use(access);
        return access;
      })().finally(() => { this.#sysexPending = null; });
      return this.#sysexPending;
    }
    this.#pending ??= (async () => {
      const access = await this.#request({ sysex: this.#sysexMode === 'always' });
      this.#sysexGranted = this.#sysexMode === 'always';
      this.#use(access);
      return access;
    })().catch((e) => { this.#pending = null; throw e; });
    return this.#pending;
  }

  /** Take `access` as the browser's: its ports, and what happens to them. */
  #use(access) {
    const old = this.#access;
    this.#access = access;
    if (old && old !== access) old.onstatechange = null;
    const seen = new Set();
    this.#hooked.clear();
    for (const p of [...valuesOf(access.inputs), ...valuesOf(access.outputs)]) {
      seen.add(p.id);
      this.#ports.set(p.id, p);
    }
    for (const id of [...this.#ports.keys()]) if (!seen.has(id)) this.#ports.delete(id);
    access.onstatechange = (e) => this.#changed(e?.port);
    // (Ports the programs listen to are listened to on the new access too.)
    for (const c of this.#conns) for (const id of c.listening) this.#hook(id);
  }

  #changed(p) {
    if (!p || p.id == null) return;
    const known = this.#ports.has(p.id);
    this.#ports.set(p.id, p);
    const s = snapshot(p);
    for (const c of this.#conns) if (c.hello) c.conn.send({ op: 'port', change: known ? 'changed' : 'added', ...s });
  }

  /** A port the browser took away (a test's access that drops one). */
  removePort(id) {
    const p = this.#ports.get(id);
    if (!p) return;
    this.#ports.delete(id);
    this.#hooked.delete(id);
    const s = snapshot(p);
    for (const c of this.#conns) {
      c.listening.delete(id);
      if (c.hello) c.conn.send({ op: 'port', change: 'removed', ...s });
    }
  }

  #hook(id) {
    const p = this.#ports.get(id);
    if (!p || p.type !== 'input' || this.#hooked.has(id)) return;
    this.#hooked.add(id);
    const listener = (e) => {
      if (this.#ports.get(id) !== p) return;
      const data = e.data instanceof Uint8Array ? e.data : new Uint8Array(e.data ?? []);
      if (!data.length) return;
      const now = globalThis.performance?.now() ?? 0;
      for (const c of this.#conns) if (c.listening.has(id)) c.conn.send({ op: 'message', port: id, t: e.timeStamp ?? now, now }, data);
    };
    if (this.#attached.has(p)) return;
    this.#attached.add(p);
    if (typeof p.addEventListener === 'function') p.addEventListener('midimessage', listener);
    else p.onmidimessage = listener;
    // (An input is opened by listening to it; the browser's open() is
    // asked for in case it is not.)
    Promise.resolve(typeof p.open === 'function' ? p.open() : null).catch(() => {});
  }

  async #message(c, m, data) {
    const op = m.op;
    if (op === 'hello') {
      if (c.hello) return;
      c.hello = true;
      this.#emit('connect', { pid: c.pid });
      let ok = true;
      if (this.#authorize) {
        try {
          ok = !!(await this.#authorize(Object.freeze({ pid: c.pid, sysex: false })));
        } catch {
          ok = false;
        }
      }
      if (!ok) {
        c.conn.send({ op: 'access', state: 'denied', sysex: false });
        this.#emit('access', { pid: c.pid, sysex: false, granted: false });
        return;
      }
      if (!this.#request) {
        c.conn.send({ op: 'access', state: 'unavailable', sysex: false });
        return;
      }
      try {
        await this.#acquire(false);
      } catch {
        c.conn.send({ op: 'access', state: 'denied', sysex: false });
        this.#emit('access', { pid: c.pid, sysex: false, granted: false });
        return;
      }
      this.#emit('access', { pid: c.pid, sysex: false, granted: true });
      c.conn.send({ op: 'access', state: 'granted', sysex: this.#sysexGranted });
      c.conn.send({ op: 'ports', ports: this.ports() });
    } else if (!c.hello) {
      return;
    } else if (op === 'listen') {
      const id = typeof m.port === 'string' ? m.port : '';
      if (m.on === true) {
        if (!this.#ports.has(id)) return;
        c.listening.add(id);
        this.#hook(id);
      } else c.listening.delete(id);
      this.#emit('listen', { pid: c.pid, port: id, on: m.on === true });
    } else if (op === 'send') {
      const id = typeof m.port === 'string' ? m.port : '';
      const p = this.#ports.get(id);
      if (!p || p.type !== 'output' || !data || !data.length || data.length > LIMITS.message) return;
      const bytes = data.slice();
      const at = Number.isFinite(m.at) && m.at > 0 && Number.isFinite(m.now) ? Math.max(0, (m.at - m.now) / 1e6) : 0;
      const when = at > 0 ? (globalThis.performance?.now() ?? 0) + at : null;
      try {
        this.#emit('send', { pid: c.pid, port: id, data: bytes, at: when });
        const sysex = bytes[0] === 0xf0;
        if (sysex && this.#sysexMode === 'never') return;
        const resolved = sysex && !this.#sysexGranted ? await this.#grantSysex(c) : true;
        if (!resolved) return;
        if (when != null) await p.send(bytes, when);
        else await p.send(bytes);
      } catch (e) {
        c.conn.send({ op: 'failed', port: id, reason: cleanText(String(e?.message ?? e), 200) });
      }
    } else if (op === 'clear') {
      const p = this.#ports.get(typeof m.port === 'string' ? m.port : '');
      if (p?.type === 'output' && typeof p.clear === 'function') p.clear();
    } else if (op === 'sysex') {
      const seq = Number.isInteger(m.seq) ? m.seq : 0;
      const granted = this.#sysexMode !== 'never' && (await this.#grantSysex(c));
      c.conn.send({ op: 'sysex', seq, granted });
    }
  }

  /** The second permission, for `c`'s program: true once it is given. */
  async #grantSysex(c) {
    if (this.#sysexGranted) return true;
    if (this.#sysexMode === 'never' || !this.#request) return false;
    if (this.#authorize) {
      try {
        if (!(await this.#authorize(Object.freeze({ pid: c.pid, sysex: true })))) return false;
      } catch {
        return false;
      }
    }
    try {
      await this.#acquire(true);
    } catch {
      this.#emit('access', { pid: c.pid, sysex: true, granted: false });
      return false;
    }
    this.#emit('access', { pid: c.pid, sysex: true, granted: true });
    for (const x of this.#conns) if (x.hello) x.conn.send({ op: 'access', state: 'granted', sysex: true });
    return true;
  }
}
