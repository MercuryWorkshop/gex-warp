// WebUSB's devices as `Devices` takes them (devices.js): each USBDevice
// the user has let the page have, as it is (Devices reads its fields and
// calls its methods), its descriptors made of its fields (WebUSB gives
// them only to a device opened, by control transfers of the program's
// own). A browser gives only the devices the user chose in its chooser,
// opened from a user's gesture (`request`), and none of the classes it
// keeps from pages (HID, mass storage, audio, video, smart cards and
// others); isochronous transfers are not offered here.

import { Devices } from './devices.js';

/**
 * The page's WebUSB devices, given to `devices` (a `Devices`, or a System,
 * for which one is made): `usb` is `navigator.usb` by default (or one of a
 * test's own). `start()` adds the devices the user granted before and
 * hears them come and go; `request(filters)`, from a user's gesture, opens
 * the browser's chooser and adds what the user picked.
 */
export class WebUSBSource {
  #usb;
  devices;

  constructor(devices, { usb = globalThis.navigator?.usb } = {}) {
    this.devices = devices instanceof Devices ? devices : new Devices(devices);
    this.#usb = usb;
  }

  /** Whether this browser has WebUSB. */
  get available() {
    return !!this.#usb;
  }

  async start() {
    if (!this.#usb) return [];
    this.#usb.addEventListener('connect', (e) => this.devices.add(e.device, { kind: 'usb' }));
    this.#usb.addEventListener('disconnect', (e) => this.devices.remove(e.device));
    const got = await this.#usb.getDevices();
    for (const d of got) this.devices.add(d, { kind: 'usb' });
    return got;
  }

  /** The browser's chooser (from a user's gesture): the device picked. */
  async request(filters = []) {
    if (!this.#usb) throw new DOMException('this browser has no WebUSB', 'NotSupportedError');
    const d = await this.#usb.requestDevice({ filters });
    this.devices.add(d, { kind: 'usb' });
    return d;
  }

  /** Let programs no longer have `device`, and forget the browser's grant. */
  async forget(device) {
    this.devices.remove(device);
    await device.forget?.();
  }
}
