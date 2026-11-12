// The bench provider (bench-provider.mjs) in a Worker: what a provider that
// can live off the page's thread does (page/runtime services-worker.js).
// A web Worker's global, or under Node a worker_threads Worker's parentPort.
import { serveServices } from '../../../page/runtime/src/services-worker.js';
import { bench } from './bench-provider.mjs';

const scope = globalThis.process?.versions?.node ? (await import('node:worker_threads')).parentPort : globalThis;
serveServices({ bench }, { scope });
