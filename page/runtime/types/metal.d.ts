import type { System } from './index';

/** One open of /dev/gex-metal, as the replayer is handed it. */
export interface MetalConnection {
  /** A number of its own, counting from 1. */
  id: number;
  /** The process that opened the node. */
  pid: number;
  /** What the program writes: a ring (8 Int32 words: bytes ever written, bytes ever read, closed, capacity; then the bytes). */
  toHost: SharedArrayBuffer;
  /** What the program reads: the same shape. */
  toGuest: SharedArrayBuffer;
  /** Post anything to it after writing to `toGuest`: the program's reader is woken. */
  wake: MessagePort;
}

/** Events: `open` (`id`, `pid`). */
export declare class Metal extends EventTarget {
  /** Made before boot. `replayer` is called on the page's thread for each open of the node. */
  constructor(sys: System, options: { replayer: (connection: MetalConnection) => void; connections?: number });
  /** Whether programs have the node (not in the single-thread build). */
  readonly available: boolean;
  /** Stop serving. */
  close(): void;
}
