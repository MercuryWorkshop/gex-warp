// start()'s restAhead (page/src/app.js): with it the site's rest bundle downloads in the background once the
// title files are in, before any read; without it (the default) not a byte of it comes before the first read,
// which fetches its frame by Range. Each setting has its own page and its own copy of the bundle
// (fixtures/images.mjs restBundle); a program reads its one lazy file 8 s after it starts, and the checks
// are on the requests the page made. The single-thread build has no lazy files' owner, and restAhead does
// nothing there: both of its phases are checked as restAhead false.
import { start } from '../../page/src/app.js';

export const meta = {
  hosts: ['browser'], images: ['busybox'], timeout: 240,
  html: '<div id="app" style="position:relative;width:320px;height:240px"></div>',
};

export default async function (t) {
  const { say, check, until, timeout } = t;
  const { arch, ...switches } = t.debug();
  const zstd = new URL('../vendor/zstddec/zstddec-stream.mjs', new URL(t.baseUrl, location.href)).href;
  // phase 1 without it, phase 2 (a fresh page) with it
  {
    const ahead = t.phase >= 2;
    const dir = ahead ? 'ra-on' : 'ra-off', bin = `${dir}/rest.bin`;
    const from = (await t.asked()).length;
    const app = await timeout(start({
      container: document.getElementById('app'),
      filesystem: 'memory',
      size: { w: 320, h: 240 },
      baseUrl: t.baseUrl,
      ...(ahead ? { restAhead: true } : {}),
      build: {
        arch: arch ?? 'aarch64', tar: [t.work('bb.tar')], lazy: t.work(`${dir}/lazy.json`), zstd,
        exe: '/bin/sh', argv: ['-c', 'echo up; sleep 8; wc -c < /data/f0; head -c 3 /data/f0; echo; sleep 600'],
        env: ['PATH=/bin:/usr/bin', 'HOME=/root'], cwd: '/', featuresOff: [], compositor: false, jitFallback: true, gl: false,
        switches,
      },
    }), 60000, `start (restAhead ${ahead})`);
    const name = `restAhead ${ahead}`;
    // The single-thread build has no owner (it never starts one, so state.lazyCache is unset) and restAhead does
    // nothing there: either setting reads a rest frame at its first read only, as restAhead false does here
    const sp = app.state.lazyCache === undefined;
    if (sp) say(`${name}: no lazy files' owner (the single-thread build): checked as restAhead false`);
    else if (!app.state.lazyCache?.on) {
      check(`${name}: the lazy files' owner runs (shared memory, Cache Storage)`, false, JSON.stringify(app.state.lazyCache));
      app.kill();
      return ahead ? undefined : t.reload();
    }
    await until(() => /^up$/m.test(app.state.out), 'the program', 30000);
    const atUp = await t.hits(bin);
    if (ahead && !sp) {
      // the download needs no read: it is under way before the program's read at 8 s
      await until(async () => (await t.hits(bin)) > 0, 'the rest bundle downloading ahead', 6000).catch(() => {});
      const before = await t.hits(bin), out = app.state.out;
      check(`${name}: the rest bundle is fetched before any read`, before > 0 && !/65536/.test(out), `${before} request(s), output ${JSON.stringify(out)}`);
      const order = (await t.asked()).slice(from);
      const first = order.indexOf(bin), tar = order.indexOf('bb.tar'), list = order.indexOf(`${dir}/lazy.json`);
      check(`${name}: ... after the title files (the bundle and the lazy list)`, first > list && (tar < 0 || first > tar), order.join(' '));
    } else {
      // up to just before the read: nothing of the rest bundle
      await new Promise((r) => setTimeout(r, 5000));
      const before = await t.hits(bin);
      check(`${name}: no rest bundle byte before the first read`, atUp === 0 && before === 0 && !/65536/.test(app.state.out), `${atUp} at the program's start, ${before} 5 s on`);
    }
    await until(() => /^65536\naaa$/m.test(app.state.out.replace(/\r/g, '')), 'the read', 30000)
      .catch((e) => { throw new Error(`${e.message}: ${app.state.out.slice(-400)}`); });
    const out = app.state.out.replace(/\r/g, '');
    check(`${name}: the read answers with the file's size and bytes`, /^65536\naaa$/m.test(out), out.slice(-200));
    if (!ahead || sp) check(`${name}: ... fetched from the rest bundle by its read`, (await t.hits(bin)) > 0, `${await t.hits(bin)} request(s)`);
    say(`${name}: ${await t.hits(bin)} rest.bin request(s)`);
    app.kill();
    await timeout(app.exited, 20000, 'kill');
    if (!ahead) return t.reload();
  }
}
