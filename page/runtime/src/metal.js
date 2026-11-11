// `Metal`: a Mac program's Metal, on the page. The runtime has no Metal of
// its own: a Mac program's Metal.framework (mink's, say) turns the program's
// calls into a stream of bytes on /dev/gex-metal and wants a replayer at the
// other end, as natively `gexc --metal PROGRAM` starts one. Here the page's
// embedder supplies it: `replayer(connection)` is called for each open of
// the node with the connection's two rings of shared memory and a port to
// post to after writing. The runtime carries the bytes and wakes the
// program's reader, and looks at none of them. Made before boot.

import { internals } from './internal.js';

export class Metal extends EventTarget {
  #channel = null;
  #served = null;
  #rt = null;

  /**
   * `replayer(connection)`: called on the page's thread for each open of
   * /dev/gex-metal with `{ id, pid, toHost, toGuest, wake }`. `toHost` and
   * `toGuest` are SharedArrayBuffers, each a ring (a header of 8 Int32
   * words: bytes ever written, bytes ever read, closed, capacity; then that
   * many bytes): read what the program wrote from `toHost`, write its
   * answers to `toGuest`, post anything to `wake` (a MessagePort) after
   * writing, and when `toHost` says closed, finish and set `toGuest`'s
   * closed word. All four can be posted on to a Worker. `connections`: how
   * many may be open at once (8). Events: `open` (`id`, `pid`).
   */
  constructor(sys, { replayer, connections = 8 } = {}) {
    super();
    if (typeof replayer !== 'function') throw new TypeError('Metal: `replayer` is a function called with each connection');
    const i = internals(sys, 'Metal');
    i.attach('metal', this, { beforeBoot: true });
    i.devices(async (dev) => {
      // The single-thread build has no shared memory to make a channel of:
      // its programs find no /dev/gex-metal.
      if (dev.single) return;
      this.#rt = dev.rt;
      this.#channel = dev.metalChannel = await dev.rt.openMetalChannel({ connections });
    }, 1);
    i.afterBoot(async (s) => {
      if (!this.#channel) return;
      this.#served = await this.#rt.serveMetalChannel(this.#channel, {
        kernel: () => s.kernel?.memory ?? null,
        replayer: (connection) => {
          this.dispatchEvent(Object.assign(new Event('open'), { id: connection.id, pid: connection.pid }));
          replayer(connection);
        },
      });
    });
  }

  /** Whether programs have the node: false in the single-thread build. */
  get available() {
    return !!this.#channel;
  }

  /** Close what a process that ended without closing left open (the system
   *  calls it as each process exits). */
  drop(pid) {
    this.#served?.drop(pid);
  }

  /** Stop serving: programs with the node open see its end. */
  close() {
    this.#served?.stop();
    this.#served = null;
  }
}
