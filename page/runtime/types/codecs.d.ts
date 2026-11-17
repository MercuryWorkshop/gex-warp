import type { System } from './index';
import type { ServiceConnection } from './index';

/** What a session takes of a program. */
export declare const LIMITS: Readonly<{ side: number; replayBytes: number; replayChunks: number; buffered: number }>;

/** The `codecs` service's provider: for `sys.services.provide`, or `serveServices` in a Worker. */
export declare function codecsProvider(): { version: number; open(conn: ServiceConnection): void };

/** The browser's video codecs (WebCodecs) for programs: the service `codecs`. */
export declare class Codecs {
  /** One per system; with `worker`, its sessions are served there (codecs-worker's `serveCodecs`). */
  constructor(sys: System, options?: { worker?: Worker | MessagePort | null });
  /** Stop providing it (sessions made stay). */
  stop(): void;
  /** Whether this browser has WebCodecs at all. */
  static readonly available: boolean;
}
