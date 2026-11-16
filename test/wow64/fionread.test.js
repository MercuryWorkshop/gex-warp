// wine/fionread's program natively (fixtures/wine/src/fionread.c): a
// response read the way Qt reads a socket, FIONREAD-sized, from a server in
// this process that answers each request in three parts 30 ms apart. Every
// response whole, FIONREAD more than 1 byte, and the worst FIONREAD-sized
// request within 4 times the worst fixed-size one (plus 250 ms for a loaded
// host). The page's leg is wine/fionread, in the browser: the Node lane's
// sockets are each Worker's own, so wineserver cannot poll a program's.
import { createServer } from 'node:net';
import { networkInterfaces } from 'node:os';
import { join } from 'node:path';
import { wow64 } from '../lib/winerun.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 900 };

export default async function (t) {
  const w = wow64(t);
  // An address of this machine's that is not loopback: the guest's own
  // 127.0.0.1 is its own.
  const host = Object.values(networkInterfaces()).flat().find((a) => a?.family === 'IPv4' && !a.internal)?.address;
  if (!host) return t.skip('no IPv4 address here but loopback');
  const PART = 'x'.repeat(1400);
  const server = createServer((s) => {
    s.once('data', () => {
      s.write(`HTTP/1.1 200 OK\r\nContent-Length: ${3 * PART.length}\r\nConnection: close\r\n\r\n${PART}`);
      setTimeout(() => s.write(PART), 30);
      setTimeout(() => s.end(PART), 60);
    });
    s.on('error', () => {});
  });
  await new Promise((r) => server.listen(0, '0.0.0.0', r));
  const port = server.address().port;
  try {
    const exe = await w.mingw('fionread', { x64: true, libs: ['-lws2_32'], src: join(import.meta.dirname, '../fixtures/wine/src/fionread.c') });
    for (const [leg, r] of [
      ['native', await w.native([`C:\\fionread.exe`, host, String(port), '8'], { files: { 'fionread.exe': exe }, name: 'native' })],
    ]) {
      const m = /^fionread worst ([\d.]+) fixed worst ([\d.]+) largest-fionread (\d+)/m.exec(r.out);
      t.check(`${leg}: every response whole`, !!m, w.detail(r));
      if (!m) continue;
      const [sized, fixed, largest] = [Number(m[1]), Number(m[2]), Number(m[3])];
      t.say(`${leg}: FIONREAD-sized worst ${sized} ms, 64 KiB worst ${fixed} ms, largest FIONREAD ${largest} bytes`);
      t.check(`${leg}: FIONREAD says the bytes waiting (largest ${largest})`, largest > 1);
      t.check(`${leg}: FIONREAD-sized reads within 4x fixed-size ones (${sized} against ${fixed} ms)`, sized <= 4 * fixed + 250);
    }
  } finally {
    server.close();
    w.clean();
  }
}
