import type { System } from './index';
import type { Devices } from './devices';

/** The page's WebUSB devices, given to a Devices. */
export declare class WebUSBSource {
  constructor(devices: Devices | System, options?: { usb?: object });
  readonly devices: Devices;
  readonly available: boolean;
  /** The devices granted before; and those that come and go, from now on. */
  start(): Promise<object[]>;
  /** The browser's chooser (from a user's gesture). */
  request(filters?: object[]): Promise<object>;
  forget(device: object): Promise<void>;
}
