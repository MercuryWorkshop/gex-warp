// The clocks of every Wine process's KUSER_SHARED_DATA page, kept from one
// place (a Worker of the host's, `Host#usdClock`).
//
// The page is written by the wineserver in its own wasm memory, which no other
// process can map: each process reads a private copy (ntdll's
// virtual_map_user_shared_data), whose times stand still unless something
// advances them. A process names its copy at start (`gex.usd_tick`), and every
// 16 ms, as wineserver refreshes its own page (server/fd.c,
// user_shared_data_timeout), this writes SystemTime, InterruptTime and
// TickCount into each one, as the server's set_user_shared_data_time does; about
// once a second it copies the time zone from the server's page too, which the
// server recomputes (a process may have copied its page before the server
// had, and the bias changes with daylight saving time). It was a thread in each
// process: a Worker each, 60 wakeups a second each.
//
// A KSYSTEM_TIME is written High2Time, LowPart, High1Time, as Windows's clock
// interrupt writes it: a reader takes High1Time, LowPart, then High2Time, and
// tries again while the two highs differ, so it never sees half of a write.

/** KUSER_SHARED_DATA's fields this writes, by offset. */
const TICK_LOW = 0x000, INTERRUPT = 0x008, SYSTEM = 0x014, ZONE_BIAS = 0x020, ZONE_ID = 0x240, TICK = 0x320;
/** How often the times move (ms), and every how many of those the time zone. */
export const USD_TICK_MS = 16;
const ZONE_EVERY = 64;
/** 100 ns intervals from 1601 to 1970 (ntdll's ticks_from_time_t). */
const EPOCH_1601 = 116444736000000000n;

const perf = globalThis.performance;
// The guest's clock base: hostNowNs's (gex.mjs), the same in every Worker.
const ORIGIN_NS = BigInt(Math.round(perf.timeOrigin * 1000)) * 1000n;
const nowNs = () => ORIGIN_NS + BigInt(Math.round(perf.now() * 1e6));

/** Store v (a BigInt) into the KSYSTEM_TIME at byte offset at of i32. */
function storeTime(i32, at, v) {
  const lo = Number(BigInt.asUintN(32, v)) | 0, hi = Number(BigInt.asIntN(32, v >> 32n));
  Atomics.store(i32, (at + 8) >>> 2, hi);
  Atomics.store(i32, at >>> 2, lo);
  Atomics.store(i32, (at + 4) >>> 2, hi);
}

export class UsdClock {
  /** pid -> { memory, at, clock: { boot, scale, shift } } */
  #pages = new Map();
  /** The wineserver's door ({ memory, area }), for its page's time zone. */
  #door = null;
  #timer = null;
  #n = 0;

  /** Process pid's page at address at of memory, its guest clock as the
   *  process's runtime reads it (gex.mjs `#guestMono`): monotonic is
   *  (now - boot) * scale, the wall clock now - shift seconds. Replaces the
   *  pid's page from before an exec. */
  add(pid, memory, at, clock) {
    this.#pages.set(pid, { memory, at: Number(at), clock });
    this.#tickOne(this.#pages.get(pid), nowNs(), true);
    if (this.#timer === null) {
      this.#timer = setInterval(() => this.tick(), USD_TICK_MS);
      // (Node: the clock alone keeps no run going.)
      this.#timer.unref?.();
    }
  }

  /** Every page forgotten and the timer stopped: the host has stopped. */
  stop() {
    this.#pages.clear();
    if (this.#timer !== null) { clearInterval(this.#timer); this.#timer = null; }
  }

  /** The clock in this realm, driven as its Worker is (`{kind: 'usdClock',
   *  op, ...}` messages, `terminate`): the single-thread build's, whose
   *  processes are fibers of this realm. */
  static inRealm() {
    const c = new UsdClock();
    return {
      postMessage: (m) => {
        if (m.op === 'add') c.add(m.pid, m.memory, m.at, m.clock);
        else if (m.op === 'drop') c.drop(m.pid);
        else if (m.op === 'door') c.door(m.door);
      },
      terminate: () => c.stop(),
    };
  }

  /** The process ended (or its memory is gone): its page is no longer written. */
  drop(pid) {
    this.#pages.delete(pid);
    if (!this.#pages.size && this.#timer !== null) { clearInterval(this.#timer); this.#timer = null; }
  }

  door(d) { this.#door = d ?? null; }

  get size() { return this.#pages.size; }

  tick() {
    const ns = nowNs(), zone = !(++this.#n % ZONE_EVERY);
    for (const p of this.#pages.values()) this.#tickOne(p, ns, zone);
  }

  #tickOne(p, ns, zone) {
    const buf = p.memory.buffer;
    if (p.at + 0x338 > buf.byteLength) return;
    const i32 = new Int32Array(buf, p.at, 0x338 >>> 2), c = p.clock;
    let mono = ns - c.boot;
    if (c.scale !== 1) mono = BigInt(Math.floor(Number(mono) * c.scale));
    const wall = ns - c.shift * 1000000000n;
    const interrupt = mono / 100n, tick = interrupt / 10000n;
    storeTime(i32, SYSTEM, wall / 100n + EPOCH_1601);
    storeTime(i32, INTERRUPT, interrupt);
    storeTime(i32, TICK, tick);
    Atomics.store(i32, TICK_LOW >>> 2, Number(BigInt.asUintN(32, tick)) | 0);
    if (zone) this.#zone(i32);
  }

  /** The time zone from the server's page (door area +140: its offset). */
  #zone(i32) {
    const d = this.#door;
    if (!d?.memory) return;
    const sbuf = d.memory.buffer;
    if (d.area + 148 > sbuf.byteLength) return;
    const usd = new DataView(sbuf).getUint32(d.area + 140, true);
    if (!usd || usd + 0x338 > sbuf.byteLength) return;
    const s = new Int32Array(sbuf, usd, 0x338 >>> 2);
    let hi, lo;
    do { hi = Atomics.load(s, (ZONE_BIAS + 4) >>> 2); lo = Atomics.load(s, ZONE_BIAS >>> 2); } while (hi !== Atomics.load(s, (ZONE_BIAS + 8) >>> 2));
    storeTime(i32, ZONE_BIAS, (BigInt(hi) << 32n) | BigInt(lo >>> 0));
    Atomics.store(i32, ZONE_ID >>> 2, Atomics.load(s, ZONE_ID >>> 2));
  }
}
