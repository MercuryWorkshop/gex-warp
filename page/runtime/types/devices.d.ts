import type { System } from './index';

/** A HID device as a source gives it (WebHIDDevice is one): an EventTarget whose `inputreport` events carry `reportId` and `data`. */
export interface HostHIDDevice extends EventTarget {
  readonly kind?: 'hid';
  readonly vendorId: number;
  readonly productId: number;
  readonly version?: number;
  readonly product?: string;
  readonly manufacturer?: string;
  readonly serial?: string;
  /** 'USB' by default. */
  readonly transport?: string;
  /** The report descriptor's bytes. */
  readonly descriptor: Uint8Array;
  open(): Promise<void>;
  close(): Promise<void>;
  sendReport(type: 'output' | 'feature', reportId: number, data: Uint8Array): Promise<void>;
  /** The report's bytes, without the ID's. */
  receiveReport?(type: 'feature' | 'input' | 'output', reportId: number): Promise<Uint8Array | DataView>;
}

/** A USB device as a source gives it: what WebUSB's USBDevice is, and its descriptors' bytes where the source has them. */
export interface HostUSBDevice {
  readonly kind?: 'usb';
  readonly vendorId: number;
  readonly productId: number;
  readonly deviceDescriptor?: Uint8Array;
  readonly configurationDescriptors?: Uint8Array[];
  readonly [field: string]: unknown;
}

export type HostDevice = HostHIDDevice | HostUSBDevice;

export interface DeviceEvent extends Event {
  readonly device: HostDevice;
  readonly id: number;
  /** For `open` and `close`: the program's process. */
  readonly pid?: number;
}

/** The most a USB transfer carries. */
export declare const TRANSFER_MAX: number;
/** A USB device descriptor made of WebUSB's fields. */
export declare function deviceDescriptor(device: object): Uint8Array;
/** A configuration's descriptors made of WebUSB's USBConfiguration. */
export declare function configurationDescriptor(configuration: object): Uint8Array;
/** A DOMException's name as a program is told it. */
export declare function errorName(e: unknown): string;

/** The devices programs may have: the service `devices`, one seam for every source. */
export declare class Devices extends EventTarget {
  constructor(sys: System);
  /** Let programs have `device`: its number for them. */
  add(device: HostDevice, options?: { kind?: 'hid' | 'usb' }): number;
  /** Take it away: programs hear it leave. */
  remove(device: HostDevice | number): boolean;
  list(): { id: number; kind: 'hid' | 'usb'; device: HostDevice; open: boolean }[];
  get(id: number): HostDevice | null;
  addEventListener(type: 'add' | 'remove' | 'open' | 'close', listener: (e: DeviceEvent) => void, options?: boolean | AddEventListenerOptions): void;
  addEventListener(type: string, listener: EventListenerOrEventListenerObject | null, options?: boolean | AddEventListenerOptions): void;
}
