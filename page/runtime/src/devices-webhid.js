// WebHID's devices as `Devices` takes them (devices.js): each HIDDevice the
// user has let the page have, its report descriptor made again of what
// WebHID says of it (its collections and their reports' items: WebHID
// does not give the descriptor's own bytes), its reports moved either way.
//
// What a program sees of a device is what WebHID says: the same reports,
// laid out alike, though the descriptor's bytes may differ from the
// device's own (each item is written with all its global state, usages in
// their four-byte form, and a collection's reports before its children's).
// A browser gives only the devices the user chose in its chooser, which a
// page may open only from a user's gesture (`request`), and none of the
// kinds it keeps from pages (keyboards, mice and the other devices a
// browser blocks); the devices granted before are there at once (`start`).

import { Devices } from './devices.js';

const bytes = (d) => (d instanceof Uint8Array ? d : ArrayBuffer.isView(d) ? new Uint8Array(d.buffer, d.byteOffset, d.byteLength) : new Uint8Array(d ?? 0));

// -- a report descriptor made again --------------------------------------------------------

// A short item (HID 1.11, 6.2.2.2): its prefix, and its data in the fewest
// bytes that hold it (signed where the item is).
function item(out, type, tag, value = null, signed = false) {
  if (value == null) {
    out.push((tag << 4) | (type << 2));
    return;
  }
  let size;
  if (signed) size = value >= -128 && value <= 127 ? 1 : value >= -32768 && value <= 32767 ? 2 : 4;
  else size = value >= 0 && value <= 0xff ? 1 : value >= 0 && value <= 0xffff ? 2 : 4;
  out.push((tag << 4) | (type << 2) | (size === 4 ? 3 : size));
  for (let i = 0; i < size; i++) out.push((value >>> (8 * i)) & 0xff);
}
const MAIN = 0, GLOBAL = 1, LOCAL = 2;

const UNIT_SYSTEMS = { none: 0, 'si-linear': 1, 'si-rotation': 2, 'english-linear': 3, 'english-rotation': 4, 'vendor-defined': 0xf, reserved: 0 };
const nibble = (v) => (v ?? 0) & 0xf;

/** The 32-bit unit (HID 1.11, 6.2.2.7) of a WebHID report item. */
export function unitOf(it) {
  const sys = UNIT_SYSTEMS[it.unitSystem] ?? 0;
  if (!sys) return 0;
  return (sys | nibble(it.unitFactorLengthExponent) << 4 | nibble(it.unitFactorMassExponent) << 8 | nibble(it.unitFactorTimeExponent) << 12 |
    nibble(it.unitFactorTemperatureExponent) << 16 | nibble(it.unitFactorCurrentExponent) << 20 | nibble(it.unitFactorLuminousIntensityExponent) << 24) >>> 0;
}

function main(out, tag, it) {
  const flags = (it.isConstant ? 1 : 0) | (it.isArray ? 0 : 2) | (it.isAbsolute === false ? 4 : 0) | (it.wrap ? 8 : 0) | (it.isLinear === false ? 0x10 : 0) |
    (it.hasPreferredState === false ? 0x20 : 0) | (it.hasNull ? 0x40 : 0) | (it.isVolatile ? 0x80 : 0) | (it.isBufferedBytes ? 0x100 : 0);
  item(out, MAIN, tag, flags);
}

function reportItems(out, reports, tag, pageNow) {
  for (const r of reports ?? []) {
    if (r.reportId) item(out, GLOBAL, 8, r.reportId);
    for (const it of r.items ?? []) {
      const usages = it.usages ?? [];
      const first = usages.length ? usages[0] : it.isRange ? it.usageMinimum : null;
      // (Each item says all its state: a page, its ranges, its unit, its
      // size; its usages in their four-byte form, page and all.)
      if (first != null && (first >>> 16) !== pageNow.page) {
        pageNow.page = first >>> 16;
        item(out, GLOBAL, 0, pageNow.page);
      }
      item(out, GLOBAL, 1, it.logicalMinimum ?? 0, true);
      item(out, GLOBAL, 2, it.logicalMaximum ?? 0, true);
      item(out, GLOBAL, 3, it.physicalMinimum ?? 0, true);
      item(out, GLOBAL, 4, it.physicalMaximum ?? 0, true);
      item(out, GLOBAL, 5, nibble(it.unitExponent));
      item(out, GLOBAL, 6, unitOf(it));
      item(out, GLOBAL, 7, it.reportSize ?? 0);
      item(out, GLOBAL, 9, it.reportCount ?? 0);
      if (it.isRange) {
        out.push(0x1b, ...[0, 1, 2, 3].map((i) => (it.usageMinimum >>> (8 * i)) & 0xff));
        out.push(0x2b, ...[0, 1, 2, 3].map((i) => (it.usageMaximum >>> (8 * i)) & 0xff));
      } else {
        for (const u of usages) out.push(0x0b, ...[0, 1, 2, 3].map((i) => (u >>> (8 * i)) & 0xff));
      }
      main(out, tag, it);
    }
  }
}

function collection(out, c, pageNow) {
  if ((c.usagePage ?? 0) !== pageNow.page) {
    pageNow.page = c.usagePage ?? 0;
    item(out, GLOBAL, 0, pageNow.page);
  }
  item(out, LOCAL, 0, c.usage ?? 0);
  item(out, MAIN, 10, c.type ?? 1);
  reportItems(out, c.inputReports, 8, pageNow);
  reportItems(out, c.outputReports, 9, pageNow);
  reportItems(out, c.featureReports, 11, pageNow);
  for (const k of c.children ?? []) collection(out, k, pageNow);
  item(out, MAIN, 12);
}

/** A report descriptor that says what WebHID's `collections` say. */
export function reportDescriptor(collections) {
  const out = [];
  const pageNow = { page: -1 };
  for (const c of collections ?? []) collection(out, c, pageNow);
  return new Uint8Array(out);
}

/** The bytes (after its ID) each report of a kind has: report ID -> length. */
export function reportLengths(collections, kind) {
  const bits = new Map();
  const walk = (c) => {
    for (const r of c[kind] ?? []) bits.set(r.reportId ?? 0, (bits.get(r.reportId ?? 0) ?? 0) + (r.items ?? []).reduce((s, it) => s + (it.reportSize ?? 0) * (it.reportCount ?? 0), 0));
    for (const k of c.children ?? []) walk(k);
  };
  for (const c of collections ?? []) walk(c);
  return new Map([...bits].map(([id, b]) => [id, Math.ceil(b / 8)]));
}

/** One HIDDevice as `Devices` takes it. */
export class WebHIDDevice extends EventTarget {
  kind = 'hid';
  #hid;
  #features;

  constructor(hid) {
    super();
    this.#hid = hid;
    Object.assign(this, { vendorId: hid.vendorId ?? 0, productId: hid.productId ?? 0, product: hid.productName ?? '', version: 0, transport: 'USB' });
    this.descriptor = reportDescriptor(hid.collections);
    this.#features = reportLengths(hid.collections, 'featureReports');
    hid.addEventListener('inputreport', (e) => this.dispatchEvent(Object.assign(new Event('inputreport'), { reportId: e.reportId ?? 0, data: bytes(e.data) })));
  }

  /** The browser's device. */
  get hid() {
    return this.#hid;
  }

  async open() {
    if (!this.#hid.opened) await this.#hid.open();
  }

  async close() {
    if (this.#hid.opened) await this.#hid.close();
  }

  sendReport(type, reportId, data) {
    return type === 'feature' ? this.#hid.sendFeatureReport(reportId, data) : this.#hid.sendReport(reportId, data);
  }

  async receiveReport(type, reportId) {
    if (type !== 'feature') throw new DOMException('WebHID gets only feature reports', 'NotSupportedError');
    let b = bytes(await this.#hid.receiveFeatureReport(reportId));
    // (A browser that gives the report's ID byte with it: taken off.)
    const want = this.#features.get(reportId);
    if (reportId && b.length && b[0] === reportId && want != null && b.length === want + 1) b = b.subarray(1);
    return b;
  }
}

/**
 * The page's WebHID devices, given to `devices` (a `Devices`, or a System,
 * for which one is made): `hid` is `navigator.hid` by default (or one of a
 * test's own). `start()` adds the devices the user granted before and
 * hears them come and go; `request(filters)`, from a user's gesture,
 * opens the browser's chooser and adds what the user picked.
 */
export class WebHIDSource {
  #hid;
  #wrapped = new Map();  // HIDDevice -> WebHIDDevice
  devices;

  constructor(devices, { hid = globalThis.navigator?.hid } = {}) {
    this.devices = devices instanceof Devices ? devices : new Devices(devices);
    this.#hid = hid;
  }

  /** Whether this browser has WebHID. */
  get available() {
    return !!this.#hid;
  }

  #add(hd) {
    let w = this.#wrapped.get(hd);
    if (!w) {
      w = new WebHIDDevice(hd);
      this.#wrapped.set(hd, w);
    }
    this.devices.add(w);
    return w;
  }

  #remove(hd) {
    const w = this.#wrapped.get(hd);
    if (!w) return;
    this.#wrapped.delete(hd);
    this.devices.remove(w);
  }

  async start() {
    if (!this.#hid) return [];
    this.#hid.addEventListener('connect', (e) => this.#add(e.device));
    this.#hid.addEventListener('disconnect', (e) => this.#remove(e.device));
    return (await this.#hid.getDevices()).map((d) => this.#add(d));
  }

  /** The browser's chooser (from a user's gesture): the devices picked. */
  async request(filters = []) {
    if (!this.#hid) throw new DOMException('this browser has no WebHID', 'NotSupportedError');
    return (await this.#hid.requestDevice({ filters })).map((d) => this.#add(d));
  }

  /** Let programs no longer have `device` (a HIDDevice), and forget the
   *  browser's grant of it. */
  async forget(device) {
    this.#remove(device);
    await device.forget?.();
  }
}
