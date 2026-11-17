// Input for Mojave's own HID event system, from the host: what gexc is
// given (GEX_HID_INPUT, lines of `move X Y`, `click X Y`, `press CODE`)
// is posted to the kernel's IOHIDSystem, which hands it to hidd the way
// Mojave's does. hidd's event system has the system for one of its
// services (its HIDServiceSupport), the NX event router its plug-in, and
// that makes an event queue on the system's event system connection
// (type 3): each event is put there as an IOHIDEvent of Apple's vendor
// page holding an NXEvent, and the queue's port is kicked. hidd gives the
// event to its clients, as it gives WindowServer's: here a monitor of the
// event system (fixtures/darwin/mojave/hidmon.c), which says each one.
// From an extracted macOS 10.14 root at GEX_TEST_MOJAVE_ROOT (skipped
// without one), never written: it is bound read-only and what the daemons
// write is bound to the test's own directories.
import { mkdirSync, writeFileSync, appendFileSync, existsSync, cpSync } from 'node:fs';
import { join } from 'node:path';
import { exec, native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 1200 };

export default async function (t) {
  const root = process.env.GEX_TEST_MOJAVE_ROOT;
  if (!root) t.skip('GEX_TEST_MOJAVE_ROOT is not set (an extracted macOS 10.14 root, x86-64)');
  if (!existsSync(join(root, 'usr/libexec/hidd'))) t.skip(`${root} has no usr/libexec/hidd`);
  const n = native(t);
  const d = n.dir('mojave-hid');
  const gl = join(d, 'gl');
  const b = await exec(['sh', join(ROOT, 'guest/launchd/build.sh'), gl, 'x86_64']);
  if (b.code !== 0) throw new Error(`guest/launchd/build.sh: ${b.stderr.trim()}`);
  // The monitor, signed with the entitlement hidd asks of one.
  const mon = join(d, 'hidmon');
  mkdirSync(mon, { recursive: true });
  const cc = await exec(['xcrun', 'clang', '-arch', 'x86_64', '-mmacosx-version-min=10.14', '-O1', '-Wall', '-Werror',
    '-framework', 'CoreFoundation', '-framework', 'IOKit', '-o', join(mon, 'hidmon'), join(ROOT, 'test/fixtures/darwin/mojave/hidmon.c')]);
  if (cc.code !== 0) t.skip(`no Mac toolchain to build the monitor: ${cc.stderr.trim().split('\n')[0]}`);
  writeFileSync(join(mon, 'entitlements.plist'), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>com.apple.private.hid.client.event-monitor</key><true/></dict></plist>
`);
  const sign = await exec(['codesign', '-f', '-s', '-', '--entitlements', join(mon, 'entitlements.plist'), join(mon, 'hidmon')]);
  if (sign.code !== 0) throw new Error(`codesign: ${sign.stderr.trim()}`);
  // Its job, started with gexlaunchd (as root, as a daemon is).
  const jobs = join(d, 'jobs');
  mkdirSync(jobs, { recursive: true });
  writeFileSync(join(jobs, 'com.gex.hidmon.plist'), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>com.gex.hidmon</string>
<key>ProgramArguments</key><array><string>/gex-hidmon/hidmon</string></array>
</dict></plist>
`);
  const binds = [];
  for (const [mine, theirs] of [['folders', '/private/var/folders'], ['tmp', '/private/var/tmp'], ['run', '/private/var/run'], ['log', '/private/var/log'], ['caches', '/Library/Caches']]) {
    mkdirSync(join(d, 'env', mine), { recursive: true });
    binds.push('--bind', `${join(d, 'env', mine)}=${theirs}`);
  }
  cpSync(join(root, 'Library/Preferences'), join(d, 'env', 'prefs'), { recursive: true });
  binds.push('--bind', `${join(d, 'env', 'prefs')}=/Library/Preferences`);
  // The input, written once the monitor is up (hidd has the system for a
  // service then, and its queue): a move, and a click somewhere else.
  const input = join(d, 'input');
  writeFileSync(input, '');
  let [since, written] = [0, 0];
  const r = await n.gexc(
    ['run', '--uid', '0', '--sysroot', root, '--bind-ro', `${root}=/`, '--bind', `${gl}=/gex`, '--bind', `${mon}=/gex-hidmon`, '--bind', `${jobs}=/gex-jobs`, ...binds,
      join(gl, 'gexlaunchd.x86_64'), '-v', '-x', 'com.apple.configd', '-x', 'com.apple.diskarbitrationd', '-x', 'com.apple.aslmanager',
      // (hidd started for the monitor, on demand, as for WindowServer.)
      '-j', '/System/Library/LaunchDaemons', '-j', '/gex-jobs', '-r', 'com.gex.hidmon', '--', '/bin/sleep', '3600'],
    {
      timeout: 900000,
      env: { GEX_HID_INPUT: input, GEX_IOKIT_TRACE: '1' },
      until: (out) => {
        if (!/^hidmon: up$/m.test(out)) return false;
        // (Again, should the first come before hidd's queue.)
        if (++since === 5 || (since === 30 && !/^hidmon: event /m.test(out))) appendFileSync(input, 'move 100 100\nclick 200 150\n'), written++;
        return (out.match(/^hidmon: event .* nx 2 at /gm) || []).length > 0 && since > 8;
      },
    },
  );
  const said = r.stdout.split('\n').filter((l) => l.startsWith('hidmon: '));
  const told = said.slice(-4).join(' / ') || r.stderr.trim().split('\n').slice(-2).join(' / ');
  t.check('hidd: has the HID system for one of its services', said.some((l) => /^hidmon: [1-9]\d* services$/.test(l)), told);
  t.check('hidd: makes its event queue on the system\'s event system connection', /io_connect_method on connection to IOHIDSystem: selector 0 scalars \[0, [0-9a-f]+\]/.test(r.stderr) && /io_connect_set_notification_port on connection to IOHIDSystem: type \d+/.test(r.stderr), told);
  t.check('the input: written', written > 0, `since ${since}`);
  const events = said.filter((l) => /^hidmon: event /.test(l));
  // NX_MOUSEMOVED (5) to 100,100, then to 200,150, the left button down
  // (NX_LMOUSEDOWN, 1) and up (NX_LMOUSEUP, 2) there: Apple's NX events,
  // vendor page 0xff00, usage 23.
  const nx = events.map((l) => l.match(/page 0xff00 usage 23 length 88 nx (\d+) at (\d+),(\d+)$/)).filter(Boolean).map((m) => m.slice(1).join(' '));
  t.check('the monitor: is given the move, as an NX event', nx.includes('5 100 100'), events.slice(0, 4).join(' / ') || told);
  t.check('the monitor: is given the click, where it was', nx.includes('5 200 150') && nx.includes('1 200 150') && nx.includes('2 200 150'), events.slice(0, 4).join(' / ') || told);
  t.check('the monitor: in the order they were posted', nx.indexOf('1 200 150') > nx.indexOf('5 100 100') && nx.indexOf('2 200 150') > nx.indexOf('1 200 150'), nx.join(' / '));
  n.clean();
}
