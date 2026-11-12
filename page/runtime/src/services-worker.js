// `serveServices`: a service's provider living in a Worker of the page's
// own (a codec over WebCodecs, a device over WebUSB or WebHID: what can run
// off the page's thread should). The page provides the service with
// `sys.services.provide(name, { worker })`; each program's connection to it
// is negotiated on the page's thread and then posted to the Worker, which
// serves it from then on: its frames are read and written here, and the
// page's thread is not in their path.
//
//   // worker.js, the page's own
//   import { serveServices } from '@gexemu/runtime/services-worker';
//   serveServices({ echo: { open(conn) { conn.addEventListener('message', ({ message, data }) => conn.send(message, data)); } } });
//
//   // the page
//   sys.services.provide('echo', { worker: new Worker(new URL('./worker.js', import.meta.url), { type: 'module' }) });

import { Link, ServiceConnection, JSON_MAX, DATA_MAX, BULK_DATA_MAX } from './services-link.js';

export { ServiceConnection, JSON_MAX, DATA_MAX, BULK_DATA_MAX };

/**
 * Serve the connections the page posts to `scope` (the Worker's global, by
 * default) with `providers`, an object of a provider (`{ open(conn) }`, or
 * a function) by service name. Returns a function that stops listening
 * (connections being served stay).
 */
export function serveServices(providers, { scope = globalThis } = {}) {
  const take = (e) => {
    const g = e.data?.gexService;
    if (!g) return;
    const p = providers[g.service];
    const provider = typeof p === 'function' ? { open: p } : p;
    const c = { id: g.id, pid: g.pid, toHost: g.toHost, toGuest: g.toGuest, wake: g.wake, memory: g.memory, queue: g.queue, slots: g.slots };
    const link = new Link(c, {
      first: () => {},
      gone: () => {},
      negotiated: provider?.open ? { service: g.service, version: g.version, provider, bulk: g.bulk } : null,
      pending: g.pending,
    });
    // (Posted for a service this Worker does not serve: the program reads
    // the end.)
    if (!provider?.open) link.end();
  };
  scope.addEventListener('message', take);
  return () => scope.removeEventListener('message', take);
}
