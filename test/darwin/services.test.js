// /dev/gex-services natively (hosts/native/src/services.rs): the channel the
// host's shell provides services to programs through, and its first
// service, notifications, with gexc's stand-in shell (notify.rs) answering
// from a script. fixtures/darwin/services.c speaks the frames by hand, as a
// framework does: a service nobody provides is version 0 and the end of the
// stream; notifications says hello, is asked about and granted
// authorization, posts (a replacement among them; an image that is not one
// is left out), sets its badge, and hears the shell's answers on a thread of
// its own; a frame over the limits ends the connection. What the program
// prints is fixtures/darwin/services.expected, the same as on the page
// (darwin/services-page), and what the shell is given is checked line by
// line. Without GEX_NOTIFY the service is refused like any other.
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { native, ROOT } from '../lib/native.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

export default async function (t) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') t.skip('a Mac program is built and checked on an arm64 Mac');
  const x86 = t.arch === 'x86';
  const mac = process.env[x86 ? 'GEX_TEST_DARWIN_ROOT_X86' : 'GEX_TEST_DARWIN_ROOT'] || join(ROOT, '..', 'pyrus', 'out', x86 ? 'root-x86_64' : 'root');
  if (!existsSync(join(mac, 'usr/lib/dyld'))) t.skip(`no Mac root at ${mac} (GEX_TEST_DARWIN_ROOT${x86 ? '_X86' : ''}: pyrus builds one)`);
  const n = native(t);
  const d = n.dir('services');
  const prog = join(d, 'services');
  execFileSync('xcrun', ['clang', '-arch', x86 ? 'x86_64' : 'arm64', '-O1', '-Wall', '-Werror', '-o', prog, join(ROOT, 'test/fixtures/darwin/services.c')], { stdio: 'pipe' });
  const script = join(d, 'script');
  writeFileSync(script, 'on one after 0.3 activate reply text Hi there\non two dismiss\n');
  const want = readFileSync(join(ROOT, 'test/fixtures/darwin/services.expected'), 'utf8');
  const r = await n.gexc(['run', '--sysroot', mac, prog], { timeout: 120000, env: { GEX_NOTIFY_INPUT: script } });
  t.check('the program hears what a Mac framework is to hear', r.code === 0 && r.stdout === want, (r.stdout + r.stderr).trim().split('\n').slice(-6).join(' / '));
  const shell = r.stderr.split('\n').filter((l) => l.startsWith('[notify]'));
  const said = [
    '[notify] hello app=org.example.services name="Services Test" icon=image/png:70',
    '[notify] authorize app=org.example.services options=alert,sound,badge -> authorized',
    '[notify] post app=org.example.services id="one" title="Hello" subtitle="from a test" body="Line one\\nline two" sound=default badge=2 thread="t1" category="greeting" banner list',
    '[notify]   action "ok" "OK" foreground',
    '[notify]   action "reply" "Reply" text button="Send" placeholder="Say something"',
    '[notify]   image image/png 70 bytes',
    '[notify] post app=org.example.services id="one" title="Hello" subtitle="from a test" body="Line one\\nline two" sound=default badge=2 thread="t1" category="greeting" banner list replaces',
    '[notify]   action "ok" "OK" foreground',
    '[notify]   action "reply" "Reply" text button="Send" placeholder="Say something"',
    '[notify]   image image/png 70 bytes',
    '[notify] post app=org.example.services id="two" title="Second" subtitle="" body="<b>not markup</b>" list',
    '[notify]   (image left out: image: not a PNG, JPEG, GIF or WebP image)',
    '[notify] badge app=org.example.services count=5',
    '[notify] dismiss app=org.example.services id="two"',
    '[notify] activate app=org.example.services id="one" action="reply" text="Hi there"',
    '[notify] withdraw app=org.example.services all (0)',
    '[notify] badge app=org.example.services count=0',
    '[notify] bye app=org.example.services (0 outstanding withdrawn)',
  ];
  const missing = said.filter((l) => !shell.includes(l));
  t.check('the shell is given what the program sent, checked', missing.length === 0, missing.slice(0, 3).join(' / '));
  const off = await n.gexc(['run', '--sysroot', mac, prog, 'refused'], { timeout: 120000 });
  t.check('without GEX_NOTIFY no service is provided', off.code === 0 && off.stdout === want.split('\n').slice(0, 2).join('\n') + '\n', off.stdout.trim());
}
