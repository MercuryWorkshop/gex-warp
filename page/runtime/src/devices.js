// `Devices`: the devices the page's shell lets programs have, the
// `devices` service of /dev/gex-services (services.js). One seam for every
// transport: a source (devices-webhid.js over WebHID, devices-webusb.js
// over WebUSB, a test's own) adds a device and removes it, and each
// program's framework (pyrus's IOKit: its registry, HID manager and
// IOUSBHost) hears it arrive and leave, opens it, and moves its reports and
// transfers through this. The README's Devices section is the reference
// for the messages.
//
// A device is the source's object, of a kind:
//
// - `'hid'`: `{ vendorId, productId, version, product, manufacturer,
//   serial, transport, descriptor }` (the report descriptor's bytes) and
//   `open()`, `close()`, `sendReport(type, reportId, bytes)` (`type`
//   `'output'` or `'feature'`), `receiveReport(type, reportId)` (resolves
//   the bytes, without the ID's), as an EventTarget whose `inputreport`
//   events carry `reportId` and `data` (bytes or a DataView).
// - `'usb'`: what WebUSB's USBDevice is (its fields, `configurations`, and
//   `open`, `close`, `selectConfiguration`, `claimInterface`,
//   `releaseInterface`, `selectAlternateInterface`, `controlTransferIn`,
//   `controlTransferOut`, `transferIn`, `transferOut`, `clearHalt`,
//   `reset`), with its descriptors' bytes as `deviceDescriptor` and
//   `configurationDescriptors` where the source has them (otherwise they
//   are made of its fields).
//
// Everything a program sends is checked: a device is named by the number
// this gave it, a length is bounded, an enumeration is one of its names.
// A program's connection opens a device at most once; the device is
// opened on the first and closed after the last, and what a connection
// left open or claimed is let go when it closes.

import { internals } from './internal.js';
import { DATA_MAX } from './services-link.js';

const KINDS = ['hid', 'usb'];
// The most a report or a transfer carries (a frame's data, less the room
// its JSON takes).
export const TRANSFER_MAX = DATA_MAX - (64 << 10);
const REPORT_MAX = 64 << 10;

const bytes = (d) => (d instanceof Uint8Array ? d : ArrayBuffer.isView(d) ? new Uint8Array(d.buffer, d.byteOffset, d.byteLength) : d ? new Uint8Array(d) : new Uint8Array(0));
const text = (s, max = 256) => (typeof s === 'string' ? s.slice(0, max) : '');
const int = (v, lo, hi) => (Number.isInteger(v) && v >= lo && v <= hi ? v : null);

/** The errors a device's promise may reject with (DOMException names), as
 *  the program is told them. */
export function errorName(e) {
  switch (e?.name) {
    case 'NotFoundError': return 'not-found';
    case 'NetworkError': return 'gone';
    case 'InvalidStateError': return 'not-open';
    case 'SecurityError': case 'NotAllowedError': return 'denied';
    case 'AbortError': return 'aborted';
    case 'TimeoutError': return 'timeout';
    case 'NotSupportedError': return 'unsupported';
    case 'TypeError': case 'DataError': case 'RangeError': return 'bad-argument';
    default: return 'error';
  }
}

// -- USB descriptors, where a source has none of its own ----------------------------------------

const bcd = (major, minor, sub) => ((major & 0xff) << 8) | ((minor & 0xf) << 4) | (sub & 0xf);
const ENDPOINT_TYPES = { control: 0, isochronous: 1, bulk: 2, interrupt: 3 };
const CLASSES = (a) => [a.interfaceClass ?? 0, a.interfaceSubclass ?? 0, a.interfaceProtocol ?? 0];

/** A USB device's device descriptor (USB 2.0 9.6.1) made of WebUSB's
 *  fields: bMaxPacketSize0, which WebUSB does not say, is 64. */
export function deviceDescriptor(d) {
  const b = new Uint8Array(18);
  const v = new DataView(b.buffer);
  b[0] = 18; b[1] = 1;
  v.setUint16(2, bcd(d.usbVersionMajor ?? 2, d.usbVersionMinor ?? 0, d.usbVersionSubminor ?? 0), true);
  b[4] = d.deviceClass ?? 0; b[5] = d.deviceSubclass ?? 0; b[6] = d.deviceProtocol ?? 0;
  b[7] = 64;
  v.setUint16(8, d.vendorId ?? 0, true);
  v.setUint16(10, d.productId ?? 0, true);
  v.setUint16(12, bcd(d.deviceVersionMajor ?? 0, d.deviceVersionMinor ?? 0, d.deviceVersionSubminor ?? 0), true);
  b[14] = d.manufacturerName ? 1 : 0; b[15] = d.productName ? 2 : 0; b[16] = d.serialNumber ? 3 : 0;
  b[17] = (d.configurations ?? []).length;
  return b;
}

/** A configuration's descriptor with its interfaces' and endpoints' (USB
 *  2.0 9.6.3 to 9.6.6) made of WebUSB's USBConfiguration: an endpoint's
 *  bInterval, which WebUSB does not say, is 1 for an interrupt endpoint
 *  and 0 for the others; the configuration says it is bus-powered at
 *  100 mA. */
export function configurationDescriptor(c) {
  const parts = [];
  let total = 9;
  for (const f of c.interfaces ?? []) {
    for (const a of f.alternates ?? []) {
      const eps = a.endpoints ?? [];
      const ifd = new Uint8Array([9, 4, f.interfaceNumber ?? 0, a.alternateSetting ?? 0, eps.length, ...CLASSES(a), 0]);
      parts.push(ifd);
      total += 9;
      for (const e of eps) {
        const type = ENDPOINT_TYPES[e.type] ?? 2;
        const size = e.packetSize ?? 64;
        parts.push(new Uint8Array([7, 5, (e.endpointNumber & 0x0f) | (e.direction === 'in' ? 0x80 : 0), type, size & 0xff, size >> 8, type === 3 ? 1 : 0]));
        total += 7;
      }
    }
  }
  const b = new Uint8Array(total);
  b.set([9, 2, total & 0xff, total >> 8, (c.interfaces ?? []).length, c.configurationValue ?? 1, 0, 0x80, 50]);
  let at = 9;
  for (const p of parts) { b.set(p, at); at += p.length; }
  return b;
}

/** The interfaces a USB configuration has, as the program's registry lists
 *  them: each interface in its alternate now. */
function interfacesOf(c) {
  return (c?.interfaces ?? []).map((f) => {
    const a = f.alternate ?? f.alternates?.[0] ?? {};
    const [cls, sub, proto] = CLASSES(a);
    return { number: f.interfaceNumber ?? 0, alternate: a.alternateSetting ?? 0, class: cls, subclass: sub, protocol: proto,
             endpoints: (a.endpoints ?? []).length, name: text(a.interfaceName ?? '') };
  });
}

function usbInfo(d) {
  return {
    vendorId: d.vendorId ?? 0, productId: d.productId ?? 0,
    version: bcd(d.deviceVersionMajor ?? 0, d.deviceVersionMinor ?? 0, d.deviceVersionSubminor ?? 0),
    usbVersion: bcd(d.usbVersionMajor ?? 2, d.usbVersionMinor ?? 0, d.usbVersionSubminor ?? 0),
    class: d.deviceClass ?? 0, subclass: d.deviceSubclass ?? 0, protocol: d.deviceProtocol ?? 0,
    product: text(d.productName), manufacturer: text(d.manufacturerName), serial: text(d.serialNumber),
    configuration: d.configuration?.configurationValue ?? 0,
    configurations: (d.configurations ?? []).map((c) => ({ value: c.configurationValue ?? 1, name: text(c.configurationName ?? '') })),
    interfaces: interfacesOf(d.configuration),
  };
}

// -- the service ----------------------------------------------------------------------------

/**
 * The devices programs may have. One per system; made any time.
 *
 * Events: `add` and `remove` (`device`, `id`), `open` and `close`
 * (`device`, `id`, `pid`: a program opened it, or let it go).
 */
export class Devices extends EventTarget {
  #devices = new Map();   // id -> { id, kind, device, info, data, opened: Set of conn states, claimed, inputs }
  #byDevice = new Map();  // device -> id
  #conns = new Set();     // { conn, pid, opened: Set of ids, claims: Map id -> Set }
  #next = 1;

  constructor(sys) {
    super();
    const i = internals(sys, 'Devices');
    i.attach('devices', this);
    sys.services.provide('devices', { version: 1, open: (conn) => this.#open(conn) });
  }

  /** Let programs have `device` (of `kind`, `'hid'` or `'usb'`: `device.kind`
   *  by default). Its number for programs; the same number again for a
   *  device already added. */
  add(device, { kind = device?.kind } = {}) {
    if (!device || typeof device !== 'object') throw new TypeError('Devices.add: a device is an object');
    if (!KINDS.includes(kind)) throw new TypeError(`Devices.add: no kind of device ${JSON.stringify(kind)} (${KINDS.join(', ')})`);
    const had = this.#byDevice.get(device);
    if (had != null) return had;
    const id = this.#next++;
    const rec = { id, kind, device, opened: new Set(), claims: new Map(), listener: null };
    this.#describe(rec);
    this.#devices.set(id, rec);
    this.#byDevice.set(device, id);
    for (const c of this.#conns) c.conn.send({ op: 'add', device: rec.info }, rec.data);
    this.#emit('add', { device, id });
    return id;
  }

  /** Take `device` (or its number) away: programs hear it leave. */
  remove(device) {
    const id = typeof device === 'number' ? device : this.#byDevice.get(device);
    const rec = this.#devices.get(id);
    if (!rec) return false;
    this.#devices.delete(id);
    this.#byDevice.delete(rec.device);
    if (rec.listener) rec.device.removeEventListener?.('inputreport', rec.listener);
    for (const c of this.#conns) {
      c.opened.delete(id);
      c.conn.send({ op: 'remove', id });
    }
    this.#emit('remove', { device: rec.device, id });
    return true;
  }

  /** The devices programs have: `[{ id, kind, device, open }]`. */
  list() {
    return [...this.#devices.values()].map((r) => ({ id: r.id, kind: r.kind, device: r.device, open: r.opened.size > 0 }));
  }

  /** The device numbered `id`, or null. */
  get(id) {
    return this.#devices.get(id)?.device ?? null;
  }

  #emit(type, props) {
    this.dispatchEvent(Object.assign(new Event(type), props));
  }

  // What programs are told of a device: its JSON and the data its
  // references name.
  #describe(rec) {
    const d = rec.device;
    if (rec.kind === 'hid') {
      const desc = bytes(d.descriptor);
      rec.info = {
        id: rec.id, kind: 'hid', vendorId: d.vendorId ?? 0, productId: d.productId ?? 0, version: d.version ?? 0,
        product: text(d.product ?? d.productName), manufacturer: text(d.manufacturer), serial: text(d.serial),
        transport: text(d.transport ?? 'USB', 32), descriptor: { at: 0, size: desc.length },
      };
      rec.data = desc;
      return;
    }
    const dd = bytes(d.deviceDescriptor ?? deviceDescriptor(d));
    const cds = (d.configurationDescriptors ?? (d.configurations ?? []).map(configurationDescriptor)).map(bytes);
    const parts = [dd, ...cds];
    const data = new Uint8Array(parts.reduce((s, p) => s + p.length, 0));
    let at = 0;
    const refs = parts.map((p) => { data.set(p, at); const r = { at, size: p.length }; at += p.length; return r; });
    rec.info = { id: rec.id, kind: 'usb', ...usbInfo(d), deviceDescriptor: refs[0], configurationDescriptors: refs.slice(1) };
    rec.data = data;
  }

  #open(conn) {
    const c = { conn, pid: conn.pid, opened: new Set(), claims: new Map(), pending: new Map() };
    this.#conns.add(c);
    conn.addEventListener('message', ({ message, data }) => {
      this.#message(c, message, data).catch((e) => console.error('[gex] devices: a message the service could not take:', e));
    });
    conn.addEventListener('close', () => {
      this.#conns.delete(c);
      for (const id of c.opened) this.#let(c, id);
    });
  }

  // `c` lets device `id` go: its claims released, and the device closed
  // when no other connection has it open.
  async #let(c, id) {
    c.opened.delete(id);
    const rec = this.#devices.get(id);
    if (!rec) return;
    rec.opened.delete(c);
    const claims = c.claims.get(id);
    c.claims.delete(id);
    try {
      for (const n of claims ?? []) if (![...rec.opened].some((o) => o.claims.get(id)?.has(n))) await rec.device.releaseInterface?.(n);
      if (!rec.opened.size) {
        if (rec.listener) rec.device.removeEventListener?.('inputreport', rec.listener);
        rec.listener = null;
        await rec.device.close?.();
      }
    } catch {
      // (A device gone meanwhile has nothing to let go.)
    }
    this.#emit('close', { device: rec.device, id, pid: c.pid });
  }

  async #message(c, m, data) {
    if (m.cancel != null) {
      // (A browser's transfer cannot be stopped: the program is told it
      // was, and what comes of it later is dropped.)
      const seq = m.cancel;
      if (c.pending.has(seq)) {
        c.pending.delete(seq);
        c.conn.send({ seq, cancelled: true, error: 'aborted' });
      }
      return;
    }
    const seq = Number.isInteger(m.seq) ? m.seq : null;
    if (m.op === 'hello') {
      const devs = [...this.#devices.values()];
      // (One frame for all: each device's references moved past those
      // before it.)
      const total = devs.reduce((s, r) => s + r.data.length, 0);
      const all = new Uint8Array(total);
      let at = 0;
      const infos = devs.map((r) => {
        all.set(r.data, at);
        const moved = JSON.parse(JSON.stringify(r.info), (k, v) => (v && typeof v === 'object' && Number.isInteger(v.at) && Number.isInteger(v.size) ? { at: v.at + at, size: v.size } : v));
        at += r.data.length;
        return moved;
      });
      c.conn.send({ seq, devices: infos }, all);
      return;
    }
    const rec = this.#devices.get(m.id);
    const answer = (a, d = null) => {
      if (seq == null) return;
      if (!c.pending.has(seq)) return;  // (cancelled)
      c.pending.delete(seq);
      c.conn.send({ seq, ...a }, d);
    };
    if (seq != null) c.pending.set(seq, true);
    if (!rec) return answer({ error: 'gone' });
    const dev = rec.device;
    const run = async (fn) => {
      try {
        await fn();
      } catch (e) {
        answer({ error: errorName(e) });
      }
    };
    switch (m.op) {
      case 'open':
        return run(async () => {
          if (!c.opened.has(rec.id)) {
            if (!rec.opened.size) {
              await dev.open?.();
              if (rec.kind === 'hid' && !rec.listener) {
                rec.listener = (ev) => this.#input(rec, ev);
                dev.addEventListener?.('inputreport', rec.listener);
              }
            }
            c.opened.add(rec.id);
            rec.opened.add(c);
            this.#emit('open', { device: dev, id: rec.id, pid: c.pid });
          }
          answer({});
        });
      case 'close':
        if (c.opened.has(rec.id)) await this.#let(c, rec.id);
        return answer({});
    }
    if (!c.opened.has(rec.id)) return answer({ error: 'not-open' });
    if (rec.kind === 'hid') return run(() => this.#hid(rec, m, data, answer));
    return run(() => this.#usb(c, rec, m, data, answer));
  }

  #input(rec, ev) {
    const id = int(ev.reportId, 0, 255) ?? 0;
    const d = bytes(ev.data);
    if (d.length > REPORT_MAX) return;
    for (const c of rec.opened) c.conn.send({ op: 'input', id: rec.id, report: id, data: { at: 0, size: d.length } }, d);
  }

  async #hid(rec, m, data, answer) {
    const dev = rec.device;
    const report = int(m.report, 0, 255);
    if (report == null) return answer({ error: 'bad-argument' });
    if (m.op === 'setReport') {
      if (m.type !== 'output' && m.type !== 'feature') return answer({ error: 'bad-argument' });
      const d = refOf(m.data, data);
      if (!d || d.length > REPORT_MAX) return answer({ error: 'bad-argument' });
      await dev.sendReport(m.type, report, d.slice());
      return answer({});
    }
    if (m.op === 'getReport') {
      if (!['feature', 'input', 'output'].includes(m.type)) return answer({ error: 'bad-argument' });
      if (!dev.receiveReport) return answer({ error: 'unsupported' });
      const got = bytes(await dev.receiveReport(m.type, report));
      if (got.length > REPORT_MAX) return answer({ error: 'overrun' });
      return answer({ data: { at: 0, size: got.length } }, got);
    }
    return answer({ error: 'unsupported' });
  }

  async #usb(c, rec, m, data, answer) {
    const dev = rec.device;
    const claims = c.claims.get(rec.id) ?? new Set();
    c.claims.set(rec.id, claims);
    const configured = () => {
      rec.info = { ...rec.info, configuration: dev.configuration?.configurationValue ?? 0, interfaces: interfacesOf(dev.configuration) };
      for (const o of this.#conns) o.conn.send({ op: 'configured', id: rec.id, configuration: rec.info.configuration, interfaces: rec.info.interfaces });
    };
    switch (m.op) {
      case 'configure': {
        const v = int(m.configuration, 0, 255);
        if (v == null) return answer({ error: 'bad-argument' });
        if ((dev.configuration?.configurationValue ?? 0) !== v) {
          await dev.selectConfiguration(v);
          configured();
        }
        return answer({ configuration: dev.configuration?.configurationValue ?? 0 });
      }
      case 'claim': case 'release': {
        const n = int(m.interface, 0, 255);
        if (n == null) return answer({ error: 'bad-argument' });
        if (m.op === 'claim') {
          if (!claims.has(n)) await dev.claimInterface(n);
          claims.add(n);
        } else if (claims.has(n)) {
          claims.delete(n);
          await dev.releaseInterface(n);
        }
        return answer({});
      }
      case 'alternate': {
        const n = int(m.interface, 0, 255), a = int(m.alternate, 0, 255);
        if (n == null || a == null) return answer({ error: 'bad-argument' });
        await dev.selectAlternateInterface(n, a);
        configured();
        return answer({});
      }
      case 'control': {
        const rt = int(m.requestType, 0, 255), req = int(m.request, 0, 255), value = int(m.value, 0, 0xffff), index = int(m.index, 0, 0xffff);
        if (rt == null || req == null || value == null || index == null) return answer({ error: 'bad-argument' });
        const setup = { requestType: ['standard', 'class', 'vendor', 'vendor'][(rt >> 5) & 3], recipient: ['device', 'interface', 'endpoint', 'other'][Math.min(rt & 0x1f, 3)], request: req, value, index };
        if (rt & 0x80) {
          const length = int(m.length, 0, 0xffff);
          if (length == null) return answer({ error: 'bad-argument' });
          const r = await dev.controlTransferIn(setup, length);
          const got = bytes(r.data);
          return answer({ status: r.status, data: { at: 0, size: got.length } }, got);
        }
        const d = m.data ? refOf(m.data, data) : new Uint8Array(0);
        if (!d) return answer({ error: 'bad-argument' });
        const r = await dev.controlTransferOut(setup, d.slice());
        return answer({ status: r.status, length: r.bytesWritten ?? d.length });
      }
      case 'transfer': {
        const ep = int(m.endpoint, 0, 255);
        if (ep == null || !(ep & 0x0f)) return answer({ error: 'bad-argument' });
        if (ep & 0x80) {
          const length = int(m.length, 0, TRANSFER_MAX);
          if (length == null) return answer({ error: 'bad-argument' });
          const r = await dev.transferIn(ep & 0x0f, length);
          const got = bytes(r.data);
          return answer({ status: r.status, data: { at: 0, size: got.length } }, got);
        }
        const d = refOf(m.data, data);
        if (!d) return answer({ error: 'bad-argument' });
        const r = await dev.transferOut(ep & 0x0f, d.slice());
        return answer({ status: r.status, length: r.bytesWritten ?? d.length });
      }
      case 'clearHalt': {
        const ep = int(m.endpoint, 0, 255);
        if (ep == null) return answer({ error: 'bad-argument' });
        await dev.clearHalt(ep & 0x80 ? 'in' : 'out', ep & 0x0f);
        return answer({});
      }
      case 'reset':
        await dev.reset();
        return answer({});
    }
    return answer({ error: 'unsupported' });
  }
}

/** The bytes a `{at, size}` reference names in a frame's data, or null. */
function refOf(ref, data) {
  if (!ref || typeof ref !== 'object') return null;
  const { at, size } = ref;
  if (!Number.isInteger(at) || !Number.isInteger(size) || at < 0 || size < 0 || at + size > data.length) return null;
  return data.subarray(at, at + size);
}
