// The eject library's start() (page/src/app.js) with the scene compositor:
// resize() lays the canvas out at the new size, not only its pixels (a
// canvas keeps the CSS size it was first laid out at).
import { start } from '../../page/src/app.js';

export const meta = {
  hosts: ['browser'], images: ['busybox'], timeout: 180,
  html: '<div id="app" style="position:relative;width:640px;height:480px"></div>',
};

export default async function (t) {
  const { check, eq, until, timeout } = t;
  const adapter = await navigator.gpu?.requestAdapter().catch(() => null);
  if (!adapter) return t.skip('no WebGPU adapter here');
  const { arch, ...switches } = t.debug();
  const zstd = new URL('../vendor/zstddec/zstddec-stream.mjs', new URL(t.baseUrl, location.href)).href;
  const app = await timeout(start({
    container: document.getElementById('app'),
    filesystem: 'memory',
    size: { w: 640, h: 480 },
    baseUrl: t.baseUrl,
    build: {
      arch: arch ?? 'aarch64', tar: [t.work('bb.tar')], zstd, exe: '/bin/sh', argv: ['-c', 'echo up; sleep 600'],
      env: ['PATH=/bin:/usr/bin', 'HOME=/root'], cwd: '/', featuresOff: [], compositor: true, jitFallback: true, gl: false, switches,
    },
  }), 60000, 'start');
  await until(() => /^up$/m.test(app.state.out.replace(/\r/g, '')), 'the program', 60000);
  const canvas = document.querySelector('#app canvas');
  check('a canvas for the compositor', !!canvas);
  eq('resize() says the new size', app.resize({ w: 800, h: 600 }), { w: 800, h: 600 });
  const r = canvas.getBoundingClientRect();
  eq('the canvas laid out at it', [Math.round(r.width), Math.round(r.height)], [800, 600]);
  await until(() => canvas.width === 800 && canvas.height === 600, 'the canvas pixels', 10000).catch(() => {});
  eq('its pixels too', [canvas.width, canvas.height], [800, 600]);
  app.kill();
  eq('kill(): exited 137', await timeout(app.exited, 20000, 'kill'), 137);
}
