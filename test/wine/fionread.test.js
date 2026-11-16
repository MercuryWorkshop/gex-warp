// A response read the way Qt reads a socket (fixtures/wine/src/fionread.c): on
// FD_READ, ask FIONREAD how much is waiting and read that much. On the page
// FIONREAD said 1 whenever anything was waiting, so each byte cost a wakeup: a
// 4 KiB response in three parts took 2.3 s against 80 ms read 64 KiB at a
// time, and a Qt program's sign-in timed out. The page's network
// here answers each connection's request in three parts 30 ms apart:
// every response whole, FIONREAD more than 1 byte, and the worst FIONREAD-sized
// request within 4 times the worst fixed-size one (plus 250 ms for a loaded
// host). wow64/fionread is the same program natively.
import { bootWine } from '../lib/wine.mjs';

export const meta = { hosts: ['browser'], arches: ['x86'], images: ['wine64', 'win-progs'], timeout: 900 };

const PART = new TextEncoder().encode('x'.repeat(1400));

/** A connection whose first write is answered in three parts, 30 ms apart. */
function threeParts() {
  let push;
  const readable = new ReadableStream({ start: (c) => { push = c; } });
  let asked = false;
  const writable = new WritableStream({
    write: () => {
      if (asked) return;
      asked = true;
      push.enqueue(new TextEncoder().encode(`HTTP/1.1 200 OK\r\nContent-Length: ${3 * PART.length}\r\nConnection: close\r\n\r\n`));
      push.enqueue(PART);
      setTimeout(() => push.enqueue(PART), 30);
      setTimeout(() => { push.enqueue(PART); push.close(); }, 60);
    },
  });
  return { readable, writable };
}

export default async function (t) {
  const w = await bootWine(t, { nullDriver: true });
  w.sys.network = { connect: () => threeParts() };
  const r = await w.run('fionread.exe', ['parts.test', '80', '8'], { timeout: 600000 });
  const m = /^fionread worst ([\d.]+) fixed worst ([\d.]+) largest-fionread (\d+)/m.exec(r.stdout);
  t.check('every response whole', !!m, r.stdout.slice(-1500));
  if (m) {
    const [sized, fixed, largest] = [Number(m[1]), Number(m[2]), Number(m[3])];
    t.say(`FIONREAD-sized worst ${sized} ms, 64 KiB worst ${fixed} ms, largest FIONREAD ${largest} bytes`);
    t.check(`FIONREAD says the bytes waiting (largest ${largest})`, largest > 1);
    t.check(`FIONREAD-sized reads within 4x fixed-size ones (${sized} against ${fixed} ms)`, sized <= 4 * fixed + 250);
  }
  await w.sys.shutdown();
}
