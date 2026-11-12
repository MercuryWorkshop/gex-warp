// `bench`, the synthetic service test/darwin/services-bench measures the
// channel with, on the page (natively: hosts/native service_bench.rs, whose
// comment has its messages). The same provider on the page's thread and in
// a Worker (bench-worker.mjs).
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pad = (o, want) => {
  const base = JSON.stringify({ ...o, p: '' }).length;
  return { ...o, p: 'x'.repeat(Math.max(0, want - base)) };
};

export const bench = {
  version: 1,
  open(conn) {
    conn.addEventListener('message', async ({ message: m }) => {
      if (m.op === 'ping') {
        conn.send(pad({ seq: m.seq, op: 'pong', t: m.t }, 56));
      } else if (m.op === 'pull') {
        const size = Math.min(m.size | 0, 64 << 20), count = m.count | 0, rate = +m.rate || 0;
        // Frame n's byte i is (n + i) & 255: a window of one buffer, which is
        // never changed, so nothing is copied to make a frame.
        const all = new Uint8Array(size + 256);
        for (let i = 0; i < all.length; i++) all[i] = i & 255;
        const start = performance.now();
        for (let n = 0; n < count && !conn.closed; n++) {
          if (rate > 0) {
            const wait = start + (n * 1000) / rate - performance.now();
            if (wait > 0) await sleep(wait);
          }
          // Two frames ahead of the program at most.
          while (conn.bufferedAmount > 2 * size && !conn.closed) await new Promise((r) => conn.addEventListener('drain', r, { once: true }));
          conn.send({ op: 'frame', n }, all.subarray(n & 255, (n & 255) + size));
        }
        conn.send({ seq: m.seq, op: 'pulled' });
      } else if (m.op === 'ticks') {
        const start = performance.now();
        for (let n = 0; n < (m.count | 0) && !conn.closed; n++) {
          const wait = start + n * (+m.interval || 0) - performance.now();
          if (wait > 0) await sleep(wait);
          conn.send(pad({ op: 'tick', n }, 56));
        }
        conn.send({ seq: m.seq, op: 'ticked' });
      }
    });
  },
};
