import type { System } from './index';
import type { Devices, HostHIDDevice } from './devices';

/** A report descriptor that says what WebHID's collections say. */
export declare function reportDescriptor(collections: readonly object[]): Uint8Array;
/** Each report ID's length (after the ID) of a kind ('inputReports', 'outputReports', 'featureReports'). */
export declare function reportLengths(collections: readonly object[], kind: string): Map<number, number>;
/** The 32-bit HID unit of a WebHID report item. */
export declare function unitOf(item: object): number;

/** One HIDDevice as Devices takes it. */
export declare class WebHIDDevice extends EventTarget implements HostHIDDevice {
  constructor(hid: object);
  readonly kind: 'hid';
  readonly vendorId: number;
  readonly productId: number;
  readonly descriptor: Uint8Array;
  /** The browser's device. */
  readonly hid: object;
  open(): Promise<void>;
  close(): Promise<void>;
  sendReport(type: 'output' | 'feature', reportId: number, data: Uint8Array): Promise<void>;
  receiveReport(type: 'feature' | 'input' | 'output', reportId: number): Promise<Uint8Array>;
}

/** The page's WebHID devices, given to a Devices. */
export declare class WebHIDSource {
  constructor(devices: Devices | System, options?: { hid?: object });
  readonly devices: Devices;
  readonly available: boolean;
  /** The devices granted before; and those that come and go, from now on. */
  start(): Promise<WebHIDDevice[]>;
  /** The browser's chooser (from a user's gesture). */
  request(filters?: object[]): Promise<WebHIDDevice[]>;
  forget(device: object): Promise<void>;
}
