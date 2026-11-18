// An XPC client of Mojave's own libxpc whose service ends with its request
// unanswered (fixtures/darwin/mojave/xpcdie.c and xpcdie-service.c), under
// gexlaunchd (guest/launchd) in an extracted macOS 10.14 root at
// GEX_TEST_MOJAVE_ROOT (skipped without one; bound read-only).
//
// The service takes the connection's receive right (libxpc's check-in)
// and ends: with the request still queued on that right, or having taken
// it. Either way the client's synchronous call is to end with
// XPC_ERROR_CONNECTION_INTERRUPTED, as on a Mac, and its connection's
// handler told the same; a second request starts the service again. In
// the first case the right comes back to the client
// (MACH_NOTIFY_PORT_DESTROYED), and libxpc's notify server, which takes
// that notification only in XNU's form, destroys it and the request with
// it, which tells the reply port. (fontd waited for good on FontWorker
// so, and the application asking fontd with it.) And a service made with
// libxpc's listener, killed and started again by the next request, answers
// it (a receive right leaves its guard behind, as on XNU).
import { mkdirSync, writeFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { exec, native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['x86'], timeout: 900 };

export default async function (t) {
  const root = process.env.GEX_TEST_MOJAVE_ROOT;
  if (!root) t.skip('GEX_TEST_MOJAVE_ROOT is not set (an extracted macOS 10.14 root, x86-64)');
  if (!existsSync(join(root, 'usr/lib/system/libxpc.dylib'))) t.skip(`${root} has no libxpc`);
  const n = native(t);
  const d = n.dir('mojave-xpc');
  const gl = join(d, 'gl');
  const b = await exec(['sh', join(ROOT, 'guest/launchd/build.sh'), gl, 'x86_64']);
  if (b.code !== 0) throw new Error(`guest/launchd/build.sh: ${b.stderr.trim()}`);
  const fx = join(ROOT, 'test/fixtures/darwin/mojave');
  const bin = join(d, 'bin');
  mkdirSync(bin, { recursive: true });
  for (const name of ['xpcdie', 'xpcdie-service', 'xpcdie-listener']) {
    const cc = await exec(['xcrun', 'clang', '-arch', 'x86_64', '-mmacosx-version-min=10.14', '-O1', '-Wall', '-Werror', '-o', join(bin, name), join(fx, `${name}.c`)]);
    if (cc.code !== 0) t.skip(`no Mac toolchain to build ${name}: ${cc.stderr.trim().split('\n')[0]}`);
  }
  const session = [];
  for (const [mine, theirs] of [['run', '/private/var/run'], ['log', '/private/var/log']]) {
    mkdirSync(join(d, 'session', mine), { recursive: true });
    session.push('--bind', `${join(d, 'session', mine)}=${theirs}`);
  }
  for (const how of ['queued', 'take']) {
    const jobs = join(d, `jobs-${how}`);
    mkdirSync(jobs, { recursive: true });
    const args = ['/gex-x/xpcdie-service', ...(how === 'take' ? ['take'] : [])].map((a) => `<string>${a}</string>`).join('');
    writeFileSync(join(jobs, 'com.gex.xpcdie.plist'), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>com.gex.xpcdie</string>
<key>ProgramArguments</key><array>${args}</array>
<key>MachServices</key><dict><key>com.gex.xpcdie</key><true/></dict>
</dict></plist>
`);
    const r = await n.gexc(
      ['run', '--uid', '0', '--sysroot', root, '--bind-ro', `${root}=/`, '--bind', `${gl}=/gex`, '--bind', `${bin}=/gex-x`, '--bind', `${jobs}=/gex-jobs`, ...session,
        join(gl, 'gexlaunchd.x86_64'), '-v', '-x', 'com.apple.aslmanager', '-j', '/gex-jobs', '--', '/gex-x/xpcdie', '2'],
      { timeout: 300000 },
    );
    const out = r.stdout.split('\n');
    const why = `exit ${r.code}: ${JSON.stringify(r.stdout.trim())} / ${r.stderr.trim().split('\n').filter((l) => /xpcdie/.test(l)).slice(-3).join(' / ')}`;
    const what = how === 'queued' ? 'the request still queued' : 'the request taken';
    t.check(`${how}: the service ended with ${what}`, /xpcdie-service: ends/.test(r.stderr), why);
    t.check(`${how}: the sync call ends, interrupted`, out.includes('xpcdie: reply 1: interrupted'), why);
    t.check(`${how}: the connection's handler is told`, out.includes('xpcdie: told interrupted'), why);
    t.check(`${how}: a second request starts the service again, and ends the same`, out.includes('xpcdie: reply 2: interrupted') && r.code === 0, why);
  }

  // A service made with libxpc's listener, killed after its first answer:
  // the next request starts it again, and the new one answers. (The port
  // it guarded comes back to gexlaunchd without its guard, as XNU clears a
  // receive right's guard and context when it leaves a space: the new
  // one's libxpc guards it again; with the old guard left on that failed,
  // libxpc gave the port up and FontWorker, started again, answered no one.)
  {
    const jobs = join(d, 'jobs-restart');
    mkdirSync(jobs, { recursive: true });
    writeFileSync(join(jobs, 'com.gex.xpcdie.plist'), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>com.gex.xpcdie</string>
<key>ProgramArguments</key><array><string>/gex-x/xpcdie-listener</string></array>
<key>MachServices</key><dict><key>com.gex.xpcdie</key><true/></dict>
</dict></plist>
`);
    const r = await n.gexc(
      ['run', '--uid', '0', '--sysroot', root, '--bind-ro', `${root}=/`, '--bind', `${gl}=/gex`, '--bind', `${bin}=/gex-x`, '--bind', `${jobs}=/gex-jobs`, ...session,
        join(gl, 'gexlaunchd.x86_64'), '-v', '-x', 'com.apple.aslmanager', '-j', '/gex-jobs', '--', '/gex-x/xpcdie', 'restart'],
      { timeout: 300000 },
    );
    const out = r.stdout.split('\n');
    const why = `exit ${r.code}: ${JSON.stringify(r.stdout.trim())} / ${r.stderr.trim().split('\n').filter((l) => /xpcdie/.test(l)).slice(-3).join(' / ')}`;
    t.check('restart: the listener answers', out.includes('xpcdie: reply 1: a reply') && out.includes('xpcdie: killed: 0'), why);
    t.check('restart: started again, it answers the next request', out.includes('xpcdie: reply 2: a reply') && out.includes('xpcdie: answered by the service started again'), why);
  }
  n.clean();
}
