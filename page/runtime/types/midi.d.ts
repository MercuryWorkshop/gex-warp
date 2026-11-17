import type { System } from './index';

/** A port as a program is told it: Web MIDI's, with bounded text. */
export interface MidiPort {
  readonly id: string;
  readonly name: string;
  readonly manufacturer: string;
  readonly version: string;
  readonly type: 'input' | 'output';
  readonly state: 'connected' | 'disconnected';
  readonly connection: string;
}

export interface MidiOptions {
  /** The browser's `navigator.requestMIDIAccess` (the default), or a test's own. */
  requestMIDIAccess?: ((options: { sysex: boolean; software?: boolean }) => Promise<unknown>) | null;
  /** Whether a program may have MIDI (and, with `sysex`, system exclusive); a throw is a refusal. */
  authorize?: ((request: { readonly pid: number; readonly sysex: boolean }) => boolean | Promise<boolean>) | null;
  /** `'ask'` (the default), `'always'` or `'never'`: the browser's second permission, for system exclusive. */
  sysex?: 'ask' | 'never' | 'always';
}

export declare const LIMITS: Readonly<{ message: number; name: number; id: number }>;

/** The page's shell gives programs the browser's MIDI (the service `midi`). One per system. */
export declare class Midi extends EventTarget {
  constructor(sys: System, options?: MidiOptions);
  /** The ports the browser has, as programs are told them. */
  ports(): MidiPort[];
  /** The pids of the programs connected. */
  programs(): number[];
  /** The browser took a port away: the programs are told. */
  removePort(id: string): void;
  addEventListener(type: 'connect' | 'disconnect', listener: (e: Event & { pid: number }) => void): void;
  addEventListener(type: 'access', listener: (e: Event & { pid: number; sysex: boolean; granted: boolean }) => void): void;
  addEventListener(type: 'listen', listener: (e: Event & { pid: number; port: string; on: boolean }) => void): void;
  addEventListener(type: 'send', listener: (e: Event & { pid: number; port: string; data: Uint8Array; at: number | null }) => void): void;
}
