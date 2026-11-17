// The `codecs` service (codecs.js) served in a Worker of the page's own,
// off the page's thread: the page makes the Worker with this module's
// `serveCodecs` and hands it to `new Codecs(sys, { worker })`.
//
//   // codecs.worker.js, the page's own
//   import { serveCodecs } from '@gexemu/runtime/codecs-worker';
//   serveCodecs();
//
//   // the page
//   new Codecs(sys, { worker: new Worker(new URL('./codecs.worker.js', import.meta.url), { type: 'module' }) });

import { serveServices } from './services-worker.js';
import { codecsProvider } from './codecs.js';

/** Serve `codecs` in this Worker (`scope`, its global by default). Returns
 *  a function that stops listening. */
export function serveCodecs({ scope = globalThis } = {}) {
  return serveServices({ codecs: codecsProvider() }, { scope });
}
