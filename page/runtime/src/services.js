// `sys.services`: the services the page's shell provides to guest programs
// (notifications first; a location, Bluetooth, MIDI, USB and HID, and video
// and audio codecs to come), each through one channel: /dev/gex-services,
// and /dev/gex-services-bulk for a service's heavy streams.
//
// A program's framework opens a node once for each connection it wants; what
// crosses each is frames (the README's Services section is their reference):
//
//   u32 LE  J, the JSON text's length   (at most 64 KiB)
//   u32 LE  D, the data's length        (at most 4 MiB; 64 MiB on a bulk node)
//   J bytes a JSON object
//   D bytes binary payloads, which the object names as {at, size, type}
//
// The first frame names a service and a version; this answers with the
// version the page's provider speaks, or 0 (then the end) when the page
// provides none of that name: a program cannot tell a page without the
// service from one that refuses it. Every frame after it is the provider's.
// A frame over the limits, or one that is not a JSON object, ends the
// connection. Connections of /dev/gex-services have small rings (the
// control path); those of /dev/gex-services-bulk are a pool of large rings
// the page reserves (`new System({ services: { bulk, bulkCapacity } })`),
// for the streams a service puts on connections of their own. The frames
// are read and written by services-link.js, on the page's thread, or in the
// Worker a provider lives in, to which a connection is handed once its
// first frame has named that provider's service.

import { internals } from './internal.js';
import { Link, ServiceConnection, JSON_MAX, DATA_MAX, BULK_DATA_MAX } from './services-link.js';

export { ServiceConnection, JSON_MAX, DATA_MAX, BULK_DATA_MAX };

const NAME = /^[a-z0-9][a-z0-9.-]{0,63}$/;

export class Services {
  #providers = new Map();
  #links = new Set();
  #channel = null;
  #served = null;

  constructor(sys) {
    const i = internals(sys, 'Services');
    i.devices(async (dev) => {
      // The single-thread build has no shared memory to make the channel
      // of: its programs find no node, as if every service were refused.
      if (dev.single || typeof SharedArrayBuffer !== 'function' || globalThis.crossOriginIsolated === false) return;
      const { bulk = 0, bulkCapacity = 1 << 24 } = i.opts.services ?? {};
      this.#channel = dev.servicesChannel = await dev.rt.openMetalChannel({
        connections: 16, capacity: 1 << 17, toGuestCapacity: 1 << 16, bulk: bulk > 0 ? { connections: bulk, capacity: bulkCapacity } : null,
      });
      // Served from before the first process, so that an init's first open
      // finds it; the kernel's memory, whose wait queues a reader parks on,
      // is there by the time a program has opened the node.
      this.#served = await dev.rt.serveMetalChannel(this.#channel, {
        kernel: () => i.sys()?.kernel?.memory ?? null,
        replayer: (c) => {
          c.memory = () => i.sys()?.kernel?.memory ?? null;
          this.#links.add(new Link(c, { first: (m, link) => this.#negotiate(m, link, c), gone: (link) => this.#links.delete(link) }));
        },
      });
    }, 1);
  }

  /** Whether programs have the node: false in the single-thread build. */
  get available() {
    return !!this.#channel;
  }

  /**
   * Provide the service `name` (lower-case letters, digits, `.` and `-`):
   * `provider.open(connection)` for each program's connection to it, at
   * `provider.version` (1) or the lower one the program asks for. A
   * function is `open` alone. With `provider.worker` (a Worker, or a
   * MessagePort to one) the service is served there instead: each
   * connection is posted to it once negotiated, and services-worker.js's
   * `serveServices` opens it with the Worker's own provider. Returns a
   * function that stops providing it (connections made stay). A second
   * provider of one name throws.
   */
  provide(name, provider) {
    if (typeof name !== 'string' || !NAME.test(name)) throw new TypeError(`services.provide: no service may be named ${JSON.stringify(name)}`);
    const p = typeof provider === 'function' ? { open: provider } : provider;
    if (typeof p?.open !== 'function' && !p?.worker) throw new TypeError('services.provide: a provider has `open(connection)`, or a `worker` that serves it');
    const version = p.version ?? 1;
    if (!Number.isInteger(version) || version < 1) throw new TypeError('services.provide: `version` is a whole number from 1');
    if (this.#providers.has(name)) throw new Error(`the ${name} service is already provided`);
    const entry = { open: (c) => p.open(c), version, worker: p.worker ?? null };
    this.#providers.set(name, entry);
    return () => {
      if (this.#providers.get(name) === entry) this.#providers.delete(name);
    };
  }

  /** The names provided. */
  names() {
    return [...this.#providers.keys()];
  }

  /** The open connections, of one service or of all. */
  connections(name) {
    return [...this.#links].map((l) => l.conn).filter((c) => c && !c.closed && (!name || c.service === name));
  }

  /** The first frame of a connection: the service it asks for. A
   *  provider in a Worker is handed the connection there (its rings, the
   *  port that wakes its program, and what was read past the first frame),
   *  and this thread is out of its path from then on. */
  #negotiate(m, link, c) {
    const name = typeof m.service === 'string' ? m.service : '';
    const asked = Number.isInteger(m.version) && m.version > 0 ? m.version : 1;
    const p = NAME.test(name) ? this.#providers.get(name) : null;
    const version = p ? Math.min(asked, p.version) : 0;
    const bulk = !!this.#channel?.bulkFrom && c.slot >= this.#channel.bulkFrom;
    link.answer(name, version, () => {
      if (!p.worker) return link.open({ service: name, version, provider: p, bulk });
      const pending = link.handOff();
      // (With the kernel's memory, which the Worker wakes the program's
      // reader in itself: nothing of the connection's goes through this
      // thread from now on.)
      p.worker.postMessage({ gexService: {
        service: name, version, bulk, id: c.id, pid: c.pid, toHost: c.toHost, toGuest: c.toGuest, wake: c.wake, pending,
        memory: c.memory() ?? undefined, queue: c.queue, slots: c.slots,
      } }, [c.wake]);
    });
  }
  /** Close what a process that ended without closing left open. */
  drop(pid) {
    this.#served?.drop(pid);
  }
}
