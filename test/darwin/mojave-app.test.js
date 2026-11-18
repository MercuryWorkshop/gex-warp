// An application's window on Mojave's desktop, and input reaching it:
// Mojave's WindowServer under gexlaunchd (guest/launchd), gexloginwindow
// (guest/loginwindow) in loginwindow's place -- the session's owner, and
// checked in as its process, which gives WindowServer's process list its
// session and so a front process -- and as the user, an application of
// this tree's own (fixtures/darwin/mojave/gexapp.m): AppKit, one window
// with a coloured view, made key and ordered front once it has finished
// launching, the application made active. gexloginwindow hands it the
// open event a launch through LaunchServices would (-o).
//
// Checked: AppKit finishes launching it; its window is visible and on the
// screen (in WindowServer's list of what is on screen), WindowServer has a
// front process; the framebuffer shows the view's colour where the window
// is; a click in the window and a key press reach the application as its
// events.
//
// From an extracted macOS 10.14 root at GEX_TEST_MOJAVE_ROOT (skipped
// without one), never written: it is bound read-only and what the daemons
// write is bound to the test's own directories. GEX_TEST_MOJAVE_SEED: a
// directory of a desktop run's saved state (scripts/mojave-desktop.sh's
// SEED), without which LaunchServices registers every application first
// (a quarter of an hour or more). Slow: most of an hour.
import { mkdirSync, writeFileSync, readFileSync, appendFileSync, existsSync, cpSync, copyFileSync } from 'node:fs';
import { join } from 'node:path';
import { inflateSync } from 'node:zlib';
import { exec, native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 7200 };

// The colour of a PNG's pixel at x,y (gexc's framebuffer PNG: 8-bit RGB or
// RGBA, filter bytes per row).
function pixel(png, x, y) {
  let at = 8, w = 0, h = 0, kind = 0;
  const idat = [];
  while (at < png.length) {
    const len = png.readUInt32BE(at), type = png.toString('latin1', at + 4, at + 8), body = png.subarray(at + 8, at + 8 + len);
    if (type === 'IHDR') [w, h, kind] = [body.readUInt32BE(0), body.readUInt32BE(4), body[9]];
    if (type === 'IDAT') idat.push(body);
    at += 12 + len;
  }
  const bpp = kind === 6 ? 4 : 3, row = w * bpp, raw = inflateSync(Buffer.concat(idat)), img = Buffer.alloc(row * h);
  for (let r = 0; r < h; r++) {
    const f = raw[r * (row + 1)], line = raw.subarray(r * (row + 1) + 1, (r + 1) * (row + 1));
    for (let i = 0; i < row; i++) {
      const a = i >= bpp ? img[r * row + i - bpp] : 0, b = r ? img[(r - 1) * row + i] : 0, c = r && i >= bpp ? img[(r - 1) * row + i - bpp] : 0;
      const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
      const pred = [0, a, b, (a + b) >> 1, pa <= pb && pa <= pc ? a : pb <= pc ? b : c][f];
      img[r * row + i] = (line[i] + pred) & 0xff;
    }
  }
  return [...img.subarray(y * row + x * bpp, y * row + x * bpp + 3)];
}

export default async function (t) {
  const root = process.env.GEX_TEST_MOJAVE_ROOT;
  if (!root) t.skip('GEX_TEST_MOJAVE_ROOT is not set (an extracted macOS 10.14 root, x86-64)');
  if (!existsSync(join(root, 'System/Library/PrivateFrameworks/SkyLight.framework/Resources/WindowServer'))) t.skip(`${root} has no WindowServer`);
  const n = native(t);
  const d = n.dir('mojave-app');
  const gl = join(d, 'gl'), lw = join(d, 'lw');
  for (const [script, out] of [['guest/launchd/build.sh', gl], ['guest/loginwindow/build.sh', lw]]) {
    const b = await exec(['sh', join(ROOT, script), out, 'x86_64']);
    if (b.code !== 0) throw new Error(`${script}: ${b.stderr.trim()}`);
  }
  // The application, a bundle.
  const fx = join(ROOT, 'test/fixtures/darwin/mojave');
  const app = join(d, 'app', 'gexapp.app', 'Contents');
  mkdirSync(join(app, 'MacOS'), { recursive: true });
  copyFileSync(join(fx, 'gexapp.plist'), join(app, 'Info.plist'));
  const cc = await exec(['xcrun', 'clang', '-arch', 'x86_64', '-mmacosx-version-min=10.14', '-O1', '-Wall', '-Werror', '-fno-objc-arc',
    '-framework', 'AppKit', '-o', join(app, 'MacOS', 'gexapp'), join(fx, 'gexapp.m')]);
  if (cc.code !== 0) t.skip(`no Mac toolchain to build the application: ${cc.stderr.trim().split('\n')[0]}`);
  const sign = await exec(['codesign', '-f', '-s', '-', join(d, 'app', 'gexapp.app')]);
  if (sign.code !== 0) throw new Error(`codesign: ${sign.stderr.trim()}`);
  // gexloginwindow's job: the session, the app as the user, its open
  // event every 30 s until it is taken, and WindowServer's windows said.
  const jobs = join(d, 'jobs');
  mkdirSync(jobs, { recursive: true });
  writeFileSync(join(jobs, 'com.gex.loginwindow.plist'), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>com.gex.loginwindow</string>
<key>ProgramArguments</key><array><string>/gex-login/loginwindow</string><string>-u</string><string>501:20</string><string>-n</string><string>gex</string><string>-o</string><string>30</string><string>-w</string><string>20</string><string>--</string><string>/gex-app/gexapp.app/Contents/MacOS/gexapp</string></array>
</dict></plist>
`);
  // The written directories (from the seed when there is one), the user's
  // passwd and home.
  const seed = process.env.GEX_TEST_MOJAVE_SEED;
  const env = join(d, 'env');
  const binds = [];
  for (const [mine, theirs, copy] of [['folders', '/private/var/folders'], ['tmp', '/private/var/tmp'], ['run', '/private/var/run'], ['log', '/private/var/log'],
    ['caches', '/Library/Caches'], ['tokens', '/private/var/db/TokenCache'], ['mds', '/private/var/db/mds'], ['coreduet', '/private/var/db/CoreDuet'],
    ['tcc', '/Library/Application Support/com.apple.TCC'], ['keychains', '/Library/Keychains'], ['crls', '/private/var/db/crls'], ['home', '/Users/gex'],
    ['prefs', '/Library/Preferences', true], ['root', '/private/var/root', true], ['syscaches', '/System/Library/Caches', true], ['cvms', '/private/var/db/CVMS', true]]) {
    const at = join(env, mine);
    if (seed && existsSync(join(seed, mine))) cpSync(join(seed, mine), at, { recursive: true });
    else if (copy && existsSync(join(root, theirs))) cpSync(join(root, theirs), at, { recursive: true });
    else mkdirSync(at, { recursive: true });
    binds.push('--bind', `${at}=${theirs}`);
  }
  for (const sub of ['Library/Preferences', 'Library/Caches', 'Library/Application Support']) mkdirSync(join(env, 'home', sub), { recursive: true });
  const etc = join(d, 'etc');
  mkdirSync(etc, { recursive: true });
  writeFileSync(join(etc, 'passwd'), readFileSync(join(root, 'etc/passwd'), 'utf8') + 'gex:*:501:20:gex:/Users/gex:/bin/sh\n');
  writeFileSync(join(etc, 'master.passwd'), readFileSync(join(root, 'etc/master.passwd'), 'utf8') + 'gex:*:501:20::0:0:gex:/Users/gex:/bin/sh\n');
  const sysroot = ['--sysroot', root, '--bind-ro', `${root}=/`];
  if (!(seed && existsSync(join(seed, 'home')))) {
    const own = await n.gexc(['run', '--uid', '0', ...sysroot, '--bind', `${join(env, 'home')}=/Users/gex`, join(root, 'usr/sbin/chown'), '-R', '501:20', '/Users/gex'], { timeout: 300000 });
    if (own.code !== 0) throw new Error(`chown of the home: exit ${own.code}: ${own.stderr.trim().split('\n').slice(-2).join(' / ')}`);
  }
  const lean = ['spindump', 'spindump_agent', 'symptomsd', 'locationd', 'airportd', 'timed', 'mobileassetd', 'nsurlsessiond', 'nsurlsessiond_privileged',
    'nsurlstoraged', 'nehelper', 'dasd', 'coreduetd', 'contextstored', 'UserEventAgent-System', 'UserEventAgent-Aqua', 'UserEventAgent-LoginWindow'];
  const png = join(d, 'fb.png'), input = join(d, 'input');
  writeFileSync(input, '');
  // Once the window is on the screen: a click in its view, then a key
  // ('a', virtual key 0); again every half a minute until they are seen.
  let window = null, sent = 0, since = 0;
  const r = await n.gexc(
    ['run', '--uid', '0', ...sysroot, ...binds, '--bind', `${etc}/passwd=/etc/passwd`, '--bind', `${etc}/master.passwd=/etc/master.passwd`,
      '--bind', `${gl}=/gex`, '--bind', `${join(lw, 'x86_64')}=/gex-login`, '--bind', `${jobs}=/gex-jobs`, '--bind', `${join(d, 'app')}=/gex-app`,
      '--env', 'OS_ACTIVITY_DT_MODE=YES',
      join(gl, 'gexlaunchd.x86_64'), '-v', '-x', 'com.apple.configd', '-x', 'com.apple.diskarbitrationd', '-x', 'com.apple.aslmanager',
      ...lean.flatMap((l) => ['-x', `com.apple.${l}`]),
      '-a', '501:20', '-j', '/System/Library/LaunchDaemons', '-j', '/System/Library/LaunchAgents', '-j', '/gex-jobs', '-r', 'com.gex.loginwindow', '-r', 'com.apple.lsd',
      '--', '/System/Library/PrivateFrameworks/SkyLight.framework/Resources/WindowServer', '-daemon'],
    {
      timeout: 6900000,
      env: { GEX_DATE: '2020-01-15', GEX_FRAMEBUFFER_PNG: png, GEX_HID_INPUT: input },
      until: (out) => {
        if (!window) {
          const m = out.match(/^gexapp: window (\d+) at (\d+),(\d+) (\d+)x(\d+): visible 1, on screen 1/m);
          if (!m) return false;
          window = m.slice(2).map(Number);
        }
        // (The screen's height, for AppKit's frame counts from its bottom.)
        const shown = out.match(/^gexloginwindow: main display \d+, (\d+)x(\d+)/m);
        const [x, y, w, h] = window, height = shown ? Number(shown[2]) : 768;
        const [cx, cy] = [x + (w >> 1), height - (y + (h >> 1))];
        if (since++ % 30 === 5) appendFileSync(input, `move ${cx} ${cy}\nsleep 200\nclick ${cx} ${cy}\nsleep 500\npress 0\n`), sent++;
        return /^gexapp: event 1 at /m.test(out) && /^gexapp: event 10 key 0 /m.test(out) && since > 10;
      },
    },
  );
  const said = r.stdout.split('\n').filter((l) => /^gex(app|loginwindow): /.test(l));
  const told = said.filter((l) => l.startsWith('gexapp: ')).slice(-3).join(' / ') || said.slice(-3).join(' / ') || r.stderr.trim().split('\n').slice(-2).join(' / ');
  t.check('gexloginwindow: checked in as the session\'s process', said.some((l) => /^gexloginwindow: checked in as the session's process: 0,/.test(l)), told);
  t.check('the app: AppKit finishes launching it (its open event)', said.includes('gexapp: did finish launching'), told);
  t.check('the app: its window visible and on the screen', !!window, told);
  const front = said.filter((l) => /front process \d+\/\d+ \(pid \d+\)/.test(l)).pop();
  t.check('WindowServer: has a front process', !!front && !/\(pid -1\)/.test(front), front || told);
  if (window && existsSync(png)) {
    // The view's colour (0.9, 0.3, 0.1, give or take the colour space)
    // a quarter of the way into the window.
    const shown = r.stdout.match(/^gexloginwindow: main display \d+, (\d+)x(\d+)/m);
    const height = shown ? Number(shown[2]) : 768;
    const [x, y, w] = window;
    const [pr, pg, pb] = pixel(readFileSync(png), x + ((3 * w) >> 2), height - (y + 40));
    t.check('the framebuffer: shows the window\'s view', pr > 180 && pg < 140 && pb < 90, `${pr},${pg},${pb}`);
  }
  t.check('input: sent', sent > 0, `since ${since}`);
  t.check('the app: is given the click, in its window', /^gexapp: event 1 at \d+,\d+ in window [1-9]/m.test(r.stdout), told);
  t.check('the app: is given the key press', /^gexapp: event 10 key 0 'a'/m.test(r.stdout), told);
  n.clean();
}
