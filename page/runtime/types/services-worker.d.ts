import type { ServiceConnection } from './index';

export type { ServiceConnection };

/** Serve, in a Worker of the page's own, the connections the page hands it (`sys.services.provide(name, { worker })`): `providers` by service name. Returns a function that stops listening. */
export declare function serveServices(
  providers: Record<string, { open(connection: ServiceConnection): void } | ((connection: ServiceConnection) => void)>,
  options?: { scope?: { addEventListener(type: 'message', fn: (e: MessageEvent) => void): void; removeEventListener(type: 'message', fn: (e: MessageEvent) => void): void } },
): () => void;

export declare const JSON_MAX: number;
export declare const DATA_MAX: number;
export declare const BULK_DATA_MAX: number;
