// Mojave's own userland under gexc: macOS 10.14's dyld, libSystem,
// libdispatch (on the kernel's work queue, its events and workloops) and
// daemons, from an extracted x86-64 root at GEX_TEST_MOJAVE_ROOT (skipped
// without one: nothing of Apple's is in this tree).
//
// sw_vers, by itself; then, under gexlaunchd (guest/launchd: the bootstrap
// server in place of launchd, as root, which libxpc insists on of a
// bootstrap server that is not process 1), a user's defaults written and
// read back through Mojave's cfprefsd (its agent and daemon started on
// demand, checking in for their Mach services), and a notification posted
// to a waiter through its notifyd. The user is 501 with a home of the
// test's own. The root is not written to: a passwd with that user, the
// home, and each directory a daemon writes are bound over it, and the
// root itself is bound read-only (--bind-ro ROOT=/), so a write the test
// did not foresee is refused and not made.
import { cpSync, mkdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { inflateSync } from 'node:zlib';
import { exec, native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 3600 };

export default async function (t) {
  const root = process.env.GEX_TEST_MOJAVE_ROOT;
  if (!root) t.skip('GEX_TEST_MOJAVE_ROOT is not set (an extracted macOS 10.14 root, x86-64)');
  if (!existsSync(join(root, 'usr/lib/dyld'))) t.skip(`${root} has no usr/lib/dyld`);
  const n = native(t);
  const d = n.dir('mojave');
  // The root, sealed.
  const sysroot = ['--sysroot', root, '--bind-ro', `${root}=/`];

  const sw = await n.gexc(['run', ...sysroot, join(root, 'usr/bin/sw_vers')], { timeout: 120000 });
  t.check('sw_vers: completes', sw.code === 0, `exit ${sw.code}: ${sw.stderr.trim().split('\n').slice(-2).join(' / ')}`);
  t.check('sw_vers: says 10.14', /^ProductVersion:\s+10\.14/m.test(sw.stdout), sw.stdout.trim());

  const gl = join(d, 'gl');
  const b = await exec(['sh', join(ROOT, 'guest/launchd/build.sh'), gl, 'x86_64']);
  if (b.code !== 0) throw new Error(`guest/launchd/build.sh: ${b.stderr.trim()}`);
  const home = join(d, 'home');
  mkdirSync(join(home, 'Library/Preferences'), { recursive: true });
  // (The home is the user's: a host's file is the first user's, root's
  // here, until it is given an owner, which gex keeps with it.)
  const own = await n.gexc(['run', '--uid', '0', ...sysroot, '--bind', `${home}=/Users/gex`, join(root, 'usr/sbin/chown'), '-R', '501:20', '/Users/gex'], { timeout: 120000 });
  if (own.code !== 0) throw new Error(`chown of the home: exit ${own.code}: ${own.stderr.trim().split('\n').slice(-2).join(' / ')}`);
  const etc = join(d, 'etc');
  mkdirSync(etc, { recursive: true });
  writeFileSync(join(etc, 'passwd'), readFileSync(join(root, 'etc/passwd'), 'utf8') + 'gex:*:501:20:gex:/Users/gex:/bin/sh\n');
  writeFileSync(join(etc, 'master.passwd'), readFileSync(join(root, 'etc/master.passwd'), 'utf8') + 'gex:*:501:20::0:0:gex:/Users/gex:/bin/sh\n');
  // (syslogd's pid and the log's housekeeping are the test's, not the
  // root's: its run and log directories bound over, aslmanager left out.)
  const sessionBinds = [];
  for (const [mine, theirs] of [['run', '/private/var/run'], ['log', '/private/var/log']]) {
    mkdirSync(join(d, 'session', mine), { recursive: true });
    sessionBinds.push('--bind', `${join(d, 'session', mine)}=${theirs}`);
  }
  const session = (script) =>
    n.gexc(
      ['run', '--uid', '0', ...sysroot, '--bind', `${gl}=/gex`, '--bind', `${home}=/Users/gex`,
        '--bind', `${etc}/passwd=/etc/passwd`, '--bind', `${etc}/master.passwd=/etc/master.passwd`, ...sessionBinds,
        '--env', 'HOME=/Users/gex', '--env', 'USER=gex',
        join(gl, 'gexlaunchd.x86_64'), '-v', '-x', 'com.apple.aslmanager', '-u', '501', '--', '/bin/sh', '-c', script],
      { timeout: 600000 },
    );

  const prefs = await session('defaults write com.gex.test Greeting hello && defaults write com.gex.test Number -int 42 && defaults read com.gex.test Number && defaults read com.gex.test Greeting');
  t.check('defaults: written and read back', prefs.code === 0 && prefs.stdout.trim() === '42\nhello', `exit ${prefs.code}, said ${JSON.stringify(prefs.stdout.trim())}`);
  t.check('defaults: through cfprefsd', /check-in com\.apple\.cfprefsd\.agent/.test(prefs.stderr), 'cfprefsd\'s agent never checked in');
  const plist = join(home, 'Library/Preferences/com.gex.test.plist');
  t.check('defaults: the plist is in the home', existsSync(plist));

  const notes = await session('notifyutil -1 com.gex.ping & sleep 3; notifyutil -p com.gex.ping; wait');
  t.check('notifyutil: a post reaches a waiter through notifyd', notes.code === 0 && notes.stdout.trim() === 'com.gex.ping', `exit ${notes.code}, said ${JSON.stringify(notes.stdout.trim())}`);

  // WindowServer, as far as it goes: run as its own job's program under
  // gexlaunchd, with the directories it and the daemons it starts write
  // bound to the test's own (copies of the root's where it has some). It
  // checks CoreDisplay's service in (a name its job does not declare), opens
  // the HID system (gex's IOKit registry: IOHIDSystem, its shared memory
  // made), and goes on to hidd, which names its event queue's memory
  // (mach_make_memory_entry_64) and hands it over, and WindowServer maps
  // it. CoreDisplay finds the GPU wrangler (AppleGPUWrangler: one GPU,
  // driving the framebuffer) and maps the framebuffer's pixels (the
  // kernel's shared memory, which gex reads); IOSurface opens
  // (IOSurfaceRoot, its limits); lsd answers it; and WindowServer checks in
  // its own services, com.apple.windowserver among them: it is up and
  // serving. securityd installs its module directory (MDS: the databases
  // the CDSA modules are found by, written through libsandbox's
  // _amkrtemp and an atomic temporary file).
  // GEX_IOKIT_TRACE says each IOKit request; what it asks last is said in
  // the check's detail.
  //
  // Beside it runs gexloginwindow (guest/loginwindow), in the place of
  // loginwindow, which is one of the system's protected programs: a job of
  // the test's, started with WindowServer. As root it makes the login's
  // audit session (a new one, user 501's, with the flags that say it
  // reaches the window server, a terminal and the console), takes its
  // connection to WindowServer as loginwindow's (its signature has the
  // entitlement WindowServer asks for), which gives WindowServer's session
  // that audit session and an owner, and puts up a window over the display
  // (the framebuffer's: CoreDisplay has it online, its one mode listed)
  // and draws in it: a fill and a white square. WindowServer composites it
  // in software into the framebuffer's pixels, which gex saves as a PNG
  // (GEX_FRAMEBUFFER_PNG): the picture is the window's, and so is what
  // WindowServer says the display shows (the stand-in asks it for the
  // display's image).
  //
  // Then it starts the user's session: Calculator, run as the user (501)
  // in the login's audit session. The user's agents run as the user
  // (gexlaunchd -a), and a name both a daemon and an agent serve is the
  // agent's for the user's processes: Calculator is sandboxed, and its
  // libsystem_secinit registers it with the user's secinitd, which makes a
  // code identity of it (its program's path by proc_pidpath, its slice
  // and cdhash by csops) and checks its signature: the Security
  // framework's module directory asks securityd first, which takes a
  // client by the task port it sends (pid_for_task of it is the client's
  // pid, or the client is refused), the CSP is attached and the
  // signature's CMS digested, trustd evaluates its certificates (its
  // databases locked and written as Apple's sqlite does), and the kernel
  // says the process is the code on disk (csops). Then secinitd makes its
  // container in the user's home and grants the sandbox, and Calculator
  // goes on into AppKit. (mds, which secinitd asks to leave the
  // container's caches out of Spotlight, is one of the system's protected
  // programs, whose text is encrypted: gexlaunchd offers no job of one
  // unless gex has the key for them, GEX_DSMOS_KEY, and without the job
  // secinitd is told there is no Spotlight and does not wait. The Dock,
  // another, is left out by name here either way.)
  //
  // All of that is slow here: lsd checks every application's signature
  // the same way before it serves WindowServer, a quarter of an hour, and
  // secinitd takes five minutes over Calculator's. The run is ended ten
  // minutes after Calculator starts.
  const lw = join(d, 'lw');
  const bl = await exec(['sh', join(ROOT, 'guest/loginwindow/build.sh'), lw, 'x86_64']);
  if (bl.code !== 0) throw new Error(`guest/loginwindow/build.sh: ${bl.stderr.trim()}`);
  const jobs = join(d, 'jobs');
  mkdirSync(jobs, { recursive: true });
  writeFileSync(join(jobs, 'com.gex.loginwindow.plist'), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>com.gex.loginwindow</string>
<key>ProgramArguments</key><array><string>/gex-login/loginwindow</string><string>-u</string><string>501:20</string><string>-n</string><string>gex</string><string>--</string><string>/Applications/Calculator.app/Contents/MacOS/Calculator</string></array>
</dict></plist>
`);
  const fb = join(d, 'fb.png');
  const ws = join(d, 'ws');
  const binds = [];
  for (const [mine, theirs] of [['folders', '/private/var/folders'], ['tmp', '/private/var/tmp'], ['run', '/private/var/run'], ['log', '/private/var/log'], ['caches', '/Library/Caches'], ['mds', '/private/var/db/mds'], ['coreduet', '/private/var/db/CoreDuet'], ['tcc', '/Library/Application Support/com.apple.TCC'], ['tokens', '/private/var/db/TokenCache'],
    // (trustd's: its revocation and pinning databases, and its OCSP
    // cache.)
    ['keychains', '/Library/Keychains'], ['crls', '/private/var/db/crls']]) {
    mkdirSync(join(ws, mine), { recursive: true });
    binds.push('--bind', `${join(ws, mine)}=${theirs}`);
  }
  // (And the system's caches, where secinitd keeps what it has made of
  // the sandbox's profiles.)
  for (const [mine, theirs] of [['prefs', 'Library/Preferences'], ['root', 'private/var/root'], ['caches-system', 'System/Library/Caches']]) {
    cpSync(join(root, theirs), join(ws, mine), { recursive: true });
    binds.push('--bind', `${join(ws, mine)}=/${theirs}`);
  }
  // (It is stopped a few seconds after Calculator has ended, or a while
  // after it started, for the framebuffer to be saved once more.)
  let [after, since] = [0, 0];
  const server = await n.gexc(
    ['run', '--uid', '0', ...sysroot, '--bind', `${gl}=/gex`, ...binds, '--bind', `${join(lw, 'x86_64')}=/gex-login`, '--bind', `${jobs}=/gex-jobs`,
      '--bind', `${home}=/Users/gex`,
      '--bind', `${etc}/passwd=/etc/passwd`, '--bind', `${etc}/master.passwd=/etc/master.passwd`,
      '--env', 'OS_ACTIVITY_DT_MODE=YES',
      // (Not the daemons that would write into the root itself: the
      // network's configuration, the disks', the log's housekeeping.)
      join(gl, 'gexlaunchd.x86_64'), '-v', '-x', 'com.apple.configd', '-x', 'com.apple.diskarbitrationd', '-x', 'com.apple.aslmanager', '-x', 'com.apple.Dock.agent',
      '-a', '501:20', '-j', '/System/Library/LaunchDaemons', '-j', '/System/Library/LaunchAgents', '-j', '/gex-jobs', '-r', 'com.gex.loginwindow',
      '--', '/System/Library/PrivateFrameworks/SkyLight.framework/Resources/WindowServer', '-daemon'],
    // (It serves from then on, and is stopped.)
    { timeout: 2400000, env: { GEX_IOKIT_TRACE: '1', GEX_FRAMEBUFFER_PNG: fb }, until: (out) => (/^gexloginwindow: \/.*: (signal|exit) \d+$/m.test(out) && ++after > 8) || (/^gexloginwindow: started /m.test(out) && ++since > 600) },
  );
  const said = server.stderr;
  const iokit = said.split('\n').filter((l) => /^\[gex:\d+\] IOKit /.test(l));
  const last = iokit.slice(-3).join(' / ');
  t.check('WindowServer: checks in for CoreDisplay', /check-in com\.apple\.CoreDisplay\.master/.test(said), 'never checked in');
  t.check('WindowServer: finds the HID system', /io_service_get_matching_services_bin on master: .*IOHIDSystem.* -> 1 found/.test(said), last);
  t.check('WindowServer: opens it and makes its shared memory', /io_service_open_extended on IOHIDSystem IOHIDSystem: type 0\n/.test(said) && /io_connect_method on connection to IOHIDSystem: selector 0 scalars \[4\] struct 0 bytes\n/.test(said), last);
  t.check('WindowServer: gets past the HID system', !/Cannot open the HID and display\/cursor system/.test(said), 'Cannot open the HID and display/cursor system');
  // hidd, started for it, checks its service in (and in again, its receive
  // right come back to gexlaunchd between), and is taken for the server it
  // is by its entitlements.
  t.check('WindowServer: reaches hidd', /check-in com\.apple\.iohideventsystem/.test(said) && !/HID event server is not entitled/.test(said), (said.match(/.*HID event server.*|.*iohideventsystem.*/g) || []).slice(-2).join(' / '));
  // hidd names the queue's memory and WindowServer maps it.
  t.check('WindowServer: gets its event queue from hidd', !/unable to make mach memory entry|Unable to create event queue via hidd|Unable to map event queue/.test(said), (said.match(/.*event queue.*|.*memory entry.*/g) || []).slice(-2).join(' / '));
  t.check('WindowServer: CoreDisplay finds the GPU and its framebuffer', /io_connect_method on connection to AppleGPUWrangler: selector 2684354562 /.test(said) && /IORegistryEntryID<\/key><integer size="64">0x[0-9a-f]+<\/integer><\/dict> -> 1 found/.test(said), last);
  t.check('WindowServer: maps the framebuffer\'s pixels, shared', /io_connect_map_memory_into_task on connection to IONDRVFramebuffer: type 110 flags 0x1 -> 0x[0-9a-f]+\+0x[0-9a-f]+ \(shared\)/.test(said), last);
  t.check('WindowServer: opens IOSurface', /io_service_open_extended on IOSurfaceRoot IOSurfaceRoot: type 0\n/.test(said) && /io_connect_method on connection to IOSurfaceRoot: get_limits /.test(said), last);
  t.check('WindowServer: is answered by lsd', !/XPC error encountered talking to lsd/.test(said), (said.match(/.*talking to lsd.*/g) || []).join(' / '));
  t.check('WindowServer: checks in its services (it is up)', /check-in com\.apple\.windowserver by \d+/.test(said) && /check-in com\.apple\.windowserver\.active by \d+/.test(said), last);
  t.check('WindowServer: does not crash', !/\(WindowServer\) crashed/.test(said), (said.match(/.*crashed.*/g) || []).join(' / '));
  // gexloginwindow, as far as it says.
  const login = server.stdout.split('\n').filter((l) => l.startsWith('gexloginwindow: '));
  const told = login.slice(-4).join(' / ');
  t.check('loginwindow\'s stand-in: starts in the system\'s audit session', login.some((l) => /^gexloginwindow: at the start: uid 0, audit session 100000 \(ok\), audit user -1, flags 0x1$/.test(l)), told);
  t.check('loginwindow\'s stand-in: makes the login\'s, user 501\'s, with graphic access', login.some((l) => /^gexloginwindow: in the login's session: uid 0, audit session 10000[1-9] \(ok\), audit user 501, flags 0x6030$/.test(l)), told);
  t.check('loginwindow\'s stand-in: connects to WindowServer', login.some((l) => /^gexloginwindow: connection [1-9]\d*$/.test(l)), told);
  t.check('loginwindow\'s stand-in: WindowServer takes its connection as loginwindow\'s', login.includes('gexloginwindow: SLSSetLoginwindowConnection -> 0'), told);
  t.check('loginwindow\'s stand-in: is given a window', login.some((l) => /^gexloginwindow: SLSNewWindow -> 0, window [1-9]\d*$/.test(l)), told);
  t.check('loginwindow\'s stand-in: draws in it', login.some((l) => /^gexloginwindow: context 0x[0-9a-f]+$/.test(l)) && login.includes('gexloginwindow: drawn'), told);
  t.check('loginwindow\'s stand-in: has it ordered in, and stays', login.includes('gexloginwindow: SLSOrderWindow -> 0') && login.includes('gexloginwindow: up'), told);
  t.check('loginwindow\'s stand-in: does not crash', !/\(loginwindow\) crashed/.test(said), (said.match(/.*crashed.*/g) || []).join(' / '));
  // securityd's module directory, in the test's own /private/var/db/mds.
  t.check('securityd: installs its module directory (MDS)', ['mdsObject.db', 'mdsDirectory.db'].every((f) => existsSync(join(ws, 'mds/system', f))), (said.match(/.*securityd\[.*|.*\(securityd\) crashed.*/g) || []).slice(-2).join(' / '));
  // Calculator, the user's.
  const started = login.map((l) => l.match(/^gexloginwindow: started \/Applications\/Calculator\.app\/Contents\/MacOS\/Calculator as 501, pid (\d+)$/)).find(Boolean);
  t.check('Calculator: started as the console user, in the login\'s session', !!started, told);
  const calc = started ? started[1] : 'none';
  const reg = said.split('\n').filter((l) => new RegExp(`secinitd\\[\\d+:\\d+\\] Calculator\\[${calc}\\]`).test(l));
  t.check('Calculator: the user\'s secinitd takes its registration', reg.some((l) => /root path for bundle "\/Applications\/Calculator\.app"/.test(l)) && !/does not match secinitd uid/.test(said), reg.slice(-1).join('') || (said.match(/.*secinitd.*/g) || []).slice(-1).join(''));
  t.check('Calculator: secinitd makes a code identity of it', reg.length > 0 && !reg.some((l) => /Failed to create a code identity/.test(l)), reg.slice(-1).join(''));
  // (A failure is "registration request failed: ... The code signature is
  // not valid", with the Security framework's error: a CSSM one when
  // securityd refused secinitd, an errno when the kernel refused csops.)
  t.check('Calculator: secinitd finds its code signature valid', reg.length > 0 && !reg.some((l) => /registration request failed/.test(l)), reg.slice(-1).join(''));
  t.check('Calculator: secinitd makes its container in the user\'s home', existsSync(join(home, 'Library/Containers/com.apple.calculator/Data')), reg.slice(-1).join(''));
  t.check('Calculator: secinitd grants its sandbox', reg.some((l) => /AppSandbox request successful/.test(l)), reg.slice(-1).join(''));
  t.check('Calculator: secinitd does not crash', !/\(secinitd\) crashed/.test(said), (said.match(/.*\(secinitd\) crashed.*/g) || []).join(' / '));
  // (The sandbox is on, and Calculator is in AppKit: it has not been
  // ended by a signal, as libsystem_secinit ends a program whose
  // registration fails.)
  const ended = login.find((l) => /^gexloginwindow: \/Applications\/Calculator\.app\/.*: (signal|exit) \d+$/.test(l));
  t.check('Calculator: goes on with its sandbox', !ended && !/\(Calculator\) crashed/.test(said), ended || (said.match(/.*\(Calculator\) crashed.*/g) || []).join(' / '));
  // (With the key they run, and none is left out for being one.)
  const left = said.match(/.*is a protected binary.*/g) || [];
  if (process.env.GEX_DSMOS_KEY) t.check('gexlaunchd: offers the protected programs\' jobs, gex having their key', left.length === 0, left.slice(0, 2).join(' / '));
  else t.check('gexlaunchd: offers no job of a protected program, gex having no key for them', left.some((l) => /com\.apple\.metadata\.mds: /.test(l)) && left.some((l) => /com\.apple\.SystemUIServer\.agent: /.test(l)), left.slice(0, 2).join(' / ') || 'none left out');
  t.say(`Calculator's registration: ${reg.slice(-1).join('') || 'none'}`);
  // The display is the framebuffer (1024 by 768, gex's default), and
  // WindowServer's own picture of it is the window's fill.
  t.check('the display: is the framebuffer\'s, online', login.some((l) => /^gexloginwindow: main display \d+, 1024x768 at 0,0, 1024x768 pixels$/.test(l)) && login.includes('gexloginwindow: display online 1, active 1, asleep 0'), told);
  t.check('the display: WindowServer shows the window on it', login.some((l) => /^gexloginwindow: shown 1024x768: at 50,50 3373bf, /.test(l)), told);
  // The framebuffer as gex saved it last (an RGB PNG, each row unfiltered):
  // the window's fill over most of it, and the white square.
  let shown = 'not saved';
  let [fill, white] = [0, 0];
  if (existsSync(fb)) {
    const png = readFileSync(fb);
    const idat = [];
    for (let at = 8; at + 8 <= png.length; at += 12 + png.readUInt32BE(at)) {
      if (png.toString('latin1', at + 4, at + 8) === 'IDAT') idat.push(png.subarray(at + 8, at + 8 + png.readUInt32BE(at)));
    }
    const rows = inflateSync(Buffer.concat(idat));
    const [w, h] = [png.readUInt32BE(16), png.readUInt32BE(20)];
    const stride = 1 + w * 3;
    for (let y = 0; y < h && rows[y * stride] === 0; y++) {
      for (let x = 0; x < w; x++) {
        const at = y * stride + 1 + x * 3;
        const rgb = (rows[at] << 16) | (rows[at + 1] << 8) | rows[at + 2];
        if (rgb === 0x3373bf) fill++;
        else if (rgb === 0xffffff) white++;
      }
    }
    shown = `${w}x${h}, ${fill} pixels of the fill, ${white} white`;
  }
  t.check('the framebuffer: shows what WindowServer composited, the window', fill > 1024 * 768 / 2 && white > 10000, shown);
  t.say(`the framebuffer: ${shown}`);
  t.say(login.join('\n'));
  n.clean();
}
