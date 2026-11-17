// The `devices` service on the page runtime (page/runtime devices.js, and
// its WebHID and WebUSB sources): a Mac program's IOKit (pyrus's, in a Mac
// root in the page's System) finds the devices the page gives it in its
// registry, and hears them arrive and leave. fixtures/darwin/iodevices.c,
// built against the SDK's IOKit, is run twice:
//
// - with devices of the test's own given to `Devices` (a HID game pad
//   with a report descriptor written here from the HID specification's
//   items, a second pad the page adds when the program sets an output
//   report and takes away at the next, a USB device with descriptors
//   written here): the program prints what fixtures/darwin/iodevices.expected
//   says (the registry's entries and their properties, the pad's elements,
//   its feature report and the value it gave, an input report and its
//   values, the second pad matched and first-matched, then removed and
//   terminated); the pad was opened, asked, written to and closed as it
//   should be;
// - with the same devices as a browser's WebHID and WebUSB would give them
//   (a mocked `navigator.hid` and `navigator.usb`: a HIDDevice that says
//   its collections and not its descriptor, a USBDevice that says its
//   fields and not its descriptors), through WebHIDSource and
//   WebUSBSource: the program prints the same, but for the manufacturer
//   and serial number WebHID does not say.
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { Devices, configurationDescriptor, deviceDescriptor } from '../../page/runtime/src/devices.js';
import { WebHIDSource, reportDescriptor } from '../../page/runtime/src/devices-webhid.js';
import { WebUSBSource } from '../../page/runtime/src/devices-webusb.js';
import { bytesOf } from '../../page/runtime/src/internal.js';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 900 };

const hex = (b) => [...b].map((x) => x.toString(16).padStart(2, '0')).join(' ');

// A game pad (HID 1.11 and the HID Usage Tables' Generic Desktop, Button,
// LED and vendor pages): input report 1 is eight buttons and an X and a Y
// of -127..127; output report 2 four LEDs and four bits of padding;
// feature report 3 two bytes of the vendor's.
const PAD = new Uint8Array([
  0x05, 0x01, 0x09, 0x05, 0xa1, 0x01,
  0x85, 0x01,
  0x05, 0x09, 0x19, 0x01, 0x29, 0x08, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02,
  0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x81, 0x25, 0x7f, 0x75, 0x08, 0x95, 0x02, 0x81, 0x02,
  0x85, 0x02,
  0x05, 0x08, 0x19, 0x01, 0x29, 0x04, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x04, 0x91, 0x02,
  0x95, 0x04, 0x91, 0x01,
  0x85, 0x03,
  0x06, 0x00, 0xff, 0x09, 0x01, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x02, 0xb1, 0x02,
  0xc0,
]);

// The same pad as WebHID describes it: its collections.
const item = (o) => ({ isAbsolute: true, isArray: false, isBufferedBytes: false, isConstant: false, isLinear: true, isRange: false, isVolatile: false, hasNull: false,
  hasPreferredState: true, wrap: false, usages: [], unitSystem: 'none', unitExponent: 0, physicalMinimum: 0, physicalMaximum: 0, ...o });
const PAD_COLLECTIONS = [{
  usagePage: 1, usage: 5, type: 1, children: [],
  inputReports: [{ reportId: 1, items: [
    item({ isRange: true, usageMinimum: 0x00090001, usageMaximum: 0x00090008, reportSize: 1, reportCount: 8, logicalMinimum: 0, logicalMaximum: 1 }),
    item({ usages: [0x00010030, 0x00010031], reportSize: 8, reportCount: 2, logicalMinimum: -127, logicalMaximum: 127 }),
  ] }],
  outputReports: [{ reportId: 2, items: [
    item({ isRange: true, usageMinimum: 0x00080001, usageMaximum: 0x00080004, reportSize: 1, reportCount: 4, logicalMinimum: 0, logicalMaximum: 1 }),
    item({ isConstant: true, reportSize: 1, reportCount: 4, logicalMinimum: 0, logicalMaximum: 1 }),
  ] }],
  featureReports: [{ reportId: 3, items: [item({ usages: [0xff000001], reportSize: 8, reportCount: 2, logicalMinimum: 0, logicalMaximum: 255 })] }],
}];

// A USB device of a vendor's class: one interface, a bulk endpoint each
// way and an interrupt one in.
function usbDevice(log) {
  const endpoints = [
    { endpointNumber: 1, direction: 'in', type: 'bulk', packetSize: 64 },
    { endpointNumber: 2, direction: 'out', type: 'bulk', packetSize: 64 },
    { endpointNumber: 3, direction: 'in', type: 'interrupt', packetSize: 8 },
  ];
  const alt = { alternateSetting: 0, interfaceClass: 0xff, interfaceSubclass: 0x01, interfaceProtocol: 0x02, interfaceName: 'Bulk', endpoints };
  const iface = { interfaceNumber: 0, alternate: alt, alternates: [alt], claimed: false };
  const config = { configurationValue: 1, configurationName: 'Only', interfaces: [iface] };
  let stored = new Uint8Array(0);
  return {
    vendorId: 0x1209, productId: 0x0002, deviceClass: 0, deviceSubclass: 0, deviceProtocol: 0,
    usbVersionMajor: 2, usbVersionMinor: 0, usbVersionSubminor: 0, deviceVersionMajor: 1, deviceVersionMinor: 2, deviceVersionSubminor: 3,
    manufacturerName: 'pyrus tests', productName: 'Test Bulk', serialNumber: 'T0002',
    configuration: config, configurations: [config], opened: false,
    async open() { log.push('usb open'); this.opened = true; },
    async close() { log.push('usb close'); this.opened = false; },
    async selectConfiguration(v) { log.push(`usb configure ${v}`); },
    async claimInterface(n) { log.push(`usb claim ${n}`); },
    async releaseInterface(n) { log.push(`usb release ${n}`); },
    async selectAlternateInterface(n, a) { log.push(`usb alternate ${n} ${a}`); },
    async controlTransferIn(setup, length) { log.push(`usb control in ${setup.requestType} ${setup.recipient} ${setup.request} ${length}`); return { status: 'ok', data: new DataView(new TextEncoder().encode('hello').buffer) }; },
    async controlTransferOut(setup, data) { log.push(`usb control out ${setup.request} ${hex(data)}`); return { status: 'ok', bytesWritten: data.length }; },
    async transferOut(ep, data) { log.push(`usb out ${ep} ${hex(data)}`); stored = new Uint8Array(data).reverse(); return { status: 'ok', bytesWritten: data.length }; },
    async transferIn(ep, length) { log.push(`usb in ${ep} ${length}`); return ep === 3 ? { status: 'stall' } : { status: 'ok', data: new DataView(stored.slice(0, length).buffer) }; },
    async clearHalt(dir, ep) { log.push(`usb clear ${dir} ${ep}`); },
    async reset() { log.push('usb reset'); },
  };
}

// A HID device of the test's own, as `Devices` takes one.
class Pad extends EventTarget {
  kind = 'hid';
  constructor(productId, product, log, onOutput) {
    super();
    Object.assign(this, { vendorId: 0x1209, productId, product, manufacturer: 'pyrus tests', serial: `P000${productId}`, version: 0x0100, descriptor: PAD });
    this.log = log;
    this.onOutput = onOutput;
  }
  async open() { this.log.push(`${this.productId} open`); }
  async close() { this.log.push(`${this.productId} close`); }
  async sendReport(type, id, data) { this.log.push(`${this.productId} ${type} ${id}: ${hex(data)}`); this.onOutput?.(data); }
  async receiveReport(type, id) { this.log.push(`${this.productId} get ${type} ${id}`); return new Uint8Array([0x11, 0x22]); }
  input(id, b) { this.dispatchEvent(Object.assign(new Event('inputreport'), { reportId: id, data: new DataView(new Uint8Array(b).buffer) })); }
}

// The same as WebHID's HIDDevice: no descriptor, its collections instead;
// its feature report with the ID's byte before it, as a browser may give it.
class MockHID extends EventTarget {
  constructor(productId, productName, log, onOutput) {
    super();
    Object.assign(this, { vendorId: 0x1209, productId, productName, collections: PAD_COLLECTIONS, opened: false });
    this.log = log;
    this.onOutput = onOutput;
  }
  async open() { this.opened = true; this.log.push(`${this.productId} open`); }
  async close() { this.opened = false; this.log.push(`${this.productId} close`); }
  async sendReport(id, data) { this.log.push(`${this.productId} output ${id}: ${hex(new Uint8Array(data.buffer ?? data))}`); this.onOutput?.(new Uint8Array(data.buffer ?? data)); }
  async sendFeatureReport(id, data) { this.log.push(`${this.productId} feature ${id}: ${hex(data)}`); }
  async receiveFeatureReport(id) { this.log.push(`${this.productId} get feature ${id}`); return new DataView(new Uint8Array([id, 0x11, 0x22]).buffer); }
  input(id, b) { this.dispatchEvent(Object.assign(new Event('inputreport'), { reportId: id, data: new DataView(new Uint8Array(b).buffer) })); }
}

class MockNavigatorPart extends EventTarget {
  constructor(granted) { super(); this.granted = granted; }
  async getDevices() { return this.granted; }
  async requestDevice() { throw new DOMException('no gesture', 'SecurityError'); }
}

export default async function (t) {
  const { eq, check, timeout } = t;
  const arch = t.arch === 'x86' ? 'x86' : 'aarch64';
  let prog;
  try {
    prog = await bytesOf(t.work(`darwin-iodevices.${arch}`));
  } catch {
    t.skip(`no Mac root or no Mac compiler for ${t.arch} (GEX_TEST_DARWIN_ROOT${t.arch === 'x86' ? '_X86' : ''}: pyrus builds one)`);
  }
  const want = readFileSync(join(import.meta.dirname, '..', 'fixtures', 'darwin', 'iodevices.expected'), 'utf8');
  const { arch: _, ...debug } = t.debug();

  // The descriptor made again of the collections: the same items' sizes.
  const again = reportDescriptor(PAD_COLLECTIONS);
  check('a descriptor made of collections', again.length > 0 && again[0] === 0x05);
  const dd = deviceDescriptor(usbDevice([]));
  eq('a USB device descriptor made of its fields', hex(dd), '12 01 00 02 00 00 00 40 09 12 02 00 23 01 01 02 03 01');
  const cd = configurationDescriptor(usbDevice([]).configuration);
  eq('and its configuration\'s', hex(cd), '09 02 27 00 01 01 00 80 32 09 04 00 00 03 ff 01 02 00 07 05 81 02 40 00 00 07 05 02 02 40 00 00 07 05 83 03 08 00 01');

  // One system, its Devices given devices of the test's own first, then
  // the browser's.
  const sys = new System({ baseUrl: t.baseUrl, arch, debug });
  const fs = new MemoryFilesystem(sys);
  fs.addTar(t.work(`darwin-root.${arch}.tar`));
  fs.addFiles({ '/usr/local/bin/iodevices': prog }, { mode: 0o755 });
  const devices = new Devices(sys);
  const events = [];
  for (const type of ['add', 'remove', 'open', 'close']) devices.addEventListener(type, ({ id }) => events.push(`${type} ${id}`));
  const run = async (label) => {
    const r = await timeout(sys.run(['/usr/local/bin/iodevices'], { env: { PATH: '/usr/bin:/bin', HOME: '/' }, cwd: '/' }), 300000, `${label}: the program`);
    await new Promise((res) => setTimeout(res, 500));
    return r;
  };

  // Devices of the test's own.
  const log = [];
  let second = null;
  const pad = new Pad(1, 'Test Pad', log, (data) => {
    if (data[0] === 0x05) {
      pad.input(1, [0x01, 0xfb, 0x07]);
      second = new Pad(3, 'Second Pad', log);
      devices.add(second);
    } else if (data[0] === 0x06) {
      devices.remove(second);
    }
  });
  devices.add(pad);
  const u = usbDevice(log);
  u.deviceDescriptor = deviceDescriptor(u);
  u.configurationDescriptors = [configurationDescriptor(u.configuration)];
  devices.add(u, { kind: 'usb' });
  await timeout(sys.boot(), 120000, 'boot');
  const own = await run('own devices');
  eq('the program, with devices of the page\'s own', own.stdout, want, own.stderr.trim().split('\n').slice(-3).join(' / '));
  // (The feature report got once for itself, once for its element's value.)
  eq('the pad was opened, asked, written to and closed', log, ['1 open', '1 get feature 3', '1 get feature 3', '1 output 2: 05', '3 open', '1 output 2: 06', '1 close']);
  eq('the service\'s events', events, ['add 1', 'add 2', 'open 1', 'add 3', 'open 3', 'remove 3', 'close 1']);
  eq('nothing is open after', devices.list().map((d) => d.open), [false, false]);
  for (const d of devices.list()) devices.remove(d.id);

  // The same devices, as a browser's WebHID and WebUSB give them.
  const wlog = [];
  const hid = new MockNavigatorPart([]);
  let wsecond = null;
  const wpad = new MockHID(1, 'Test Pad', wlog, (data) => {
    if (data[0] === 0x05) {
      wpad.input(1, [0x01, 0xfb, 0x07]);
      wsecond = new MockHID(3, 'Second Pad', wlog);
      hid.dispatchEvent(Object.assign(new Event('connect'), { device: wsecond }));
    } else if (data[0] === 0x06) {
      hid.dispatchEvent(Object.assign(new Event('disconnect'), { device: wsecond }));
    }
  });
  hid.granted.push(wpad);
  const usb = new MockNavigatorPart([usbDevice(wlog)]);
  const hs = new WebHIDSource(devices, { hid }), us = new WebUSBSource(devices, { usb });
  await hs.start();
  await us.start();
  let refused = null;
  try { await hs.request(); } catch (e) { refused = e.name; }
  check('a chooser without a gesture is refused', refused === 'SecurityError');
  const web = await run('WebHID and WebUSB');
  // (What WebHID does not say is absent; the descriptor made again is of
  // another length, the same elements; the USB device is at the place its
  // number gives it.)
  const same = (s) => s.replace(/^( {2}ReportDescriptor:) \d+ bytes$/m, '$1 made again').replace(/@00[0-9a-f]{2}0000/g, '@LOCATION').replace(/(locationID:) 0x[0-9a-f]+/g, '$1 LOCATION');
  const wanted = same(want.replace(/^ {2}(Manufacturer|SerialNumber): .*$/gm, '  $1: absent'));
  eq('the program, with WebHID\'s and WebUSB\'s', same(web.stdout), wanted, web.stderr.trim().split('\n').slice(-3).join(' / '));
  eq('the pad, through WebHID', wlog, ['1 open', '1 get feature 3', '1 get feature 3', '1 output 2: 05', '3 open', '1 output 2: 06', '1 close']);
  await sys.shutdown();
}
