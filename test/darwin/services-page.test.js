// /dev/gex-services on the page runtime (hosts/browser/src/services.rs,
// page/runtime services.js and notifications.js): the same node as gexc's
// (darwin/services), its other end the page's `sys.services`, with the
// notifications service a page's shell provides through `Notifications`.
// fixtures/darwin/services.c in a Mac root in the page's System: a service
// the page does not provide is version 0 and the end of the stream; the
// shell is asked to authorize (and answers), is given each notification as
// frozen, checked data (text bounded and kept as text, the image a Blob of
// the type its bytes say, one that is not an image left out, a replacement
// said to be one), answers them through their methods, hears the badge and
// the withdrawal; and the program prints what it printed under gexc,
// fixtures/darwin/services.expected. Then a provider of the test's own on
// `sys.services`, which the same program speaks to: a lower version agreed
// on, a request and its data answered, an event sent unasked, the
// connection's close heard.
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { Notifications } from '../../page/runtime/src/notifications.js';
import { bytesOf } from '../../page/runtime/src/internal.js';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 600 };

export default async function (t) {
  const { eq, check, timeout } = t;
  const arch = t.arch === 'x86' ? 'x86' : 'aarch64';
  let prog;
  try {
    prog = await bytesOf(t.work(`darwin-services.${arch}`));
  } catch {
    t.skip(`no Mac root or no Mac compiler for ${t.arch} (GEX_TEST_DARWIN_ROOT${t.arch === 'x86' ? '_X86' : ''}: pyrus builds one)`);
  }
  const want = readFileSync(join(import.meta.dirname, '..', 'fixtures', 'darwin', 'services.expected'), 'utf8');
  const { arch: _, ...debug } = t.debug();
  const sys = new System({ baseUrl: t.baseUrl, arch, debug });
  const fs = new MemoryFilesystem(sys);
  fs.addTar(t.work(`darwin-root.${arch}.tar`));
  fs.addFiles({ '/usr/local/bin/services': prog }, { mode: 0o755 });
  const asked = [];
  const notes = new Notifications(sys, { authorize: async (r) => { asked.push(r); return true; } });
  const seen = [], withdrawn = [], badges = [];
  notes.addEventListener('notification', ({ notification: n }) => {
    seen.push(n);
    if (n.id === 'two') n.dismiss();
    else if (n.replaces) setTimeout(() => n.activate('reply', { text: 'Hi there' }), 300);
  });
  notes.addEventListener('withdrawn', ({ notification, reason }) => withdrawn.push(`${notification.id} ${reason}`));
  notes.addEventListener('badge', ({ count }) => badges.push(count));
  // A provider of the test's own, beside it.
  const raw = [];
  sys.services.provide('echo', { version: 2, open: (c) => {
    raw.push(`open ${c.service} v${c.version}`);
    c.addEventListener('message', ({ message, data }) => { raw.push(`${message.op} ${data.length}`); c.send({ seq: message.seq, back: message.op }, data); c.send({ op: 'event' }); });
    c.addEventListener('close', () => raw.push('close'));
  } });
  await timeout(sys.boot(), 120000, 'boot');
  check('programs have the node', sys.services.available);
  const r = await timeout(sys.run(['/usr/local/bin/services'], { env: { PATH: '/usr/bin:/bin', HOME: '/' }, cwd: '/' }), 240000, 'the program');
  eq('the program hears what it heard under gexc', r.stdout, want, r.stderr.trim().split('\n').slice(-3).join(' / '));
  eq('the shell was asked once', asked.map((a) => [a.app.id, a.app.name, a.options.join(','), Object.isFrozen(a)]), [['org.example.services', 'Services Test', 'alert,sound,badge', true]]);
  check('the icon is a PNG blob', asked[0]?.app.icon instanceof Blob && asked[0].app.icon.type === 'image/png' && asked[0].app.icon.size === 70);
  const [one, again, two] = seen;
  eq('the first notification', one && [one.id, one.title, one.subtitle, one.body, one.sound, one.badge, one.thread, one.category, one.banner, one.list, one.replaces, Object.isFrozen(one)],
    ['one', 'Hello', 'from a test', 'Line one\nline two', 'default', 2, 't1', 'greeting', true, true, false, true]);
  eq('its actions', one?.actions.map((a) => [a.id, a.title, a.foreground, a.destructive, a.text && a.text.button, a.text && a.text.placeholder]),
    [['ok', 'OK', true, false, null, null], ['reply', 'Reply', false, false, 'Send', 'Say something']]);
  check('its image is a PNG blob', one?.image instanceof Blob && one.image.type === 'image/png');
  eq('the replacement says so', again && [again.id, again.replaces], ['one', true]);
  eq('text stays text, and what is not an image is left out', two && [two.body, two.image, two.sound], ['<b>not markup</b>', null, null]);
  eq('withdrawals', withdrawn, ['one replaced', 'two answered', 'one answered']);
  eq('the badge', badges, [5, 0]);
  eq('nothing is outstanding', notes.list().length, 0);

  // The provider of the test's own, through the same program.
  const e = await timeout(sys.run(['/usr/local/bin/services', 'echo'], { env: { PATH: '/usr/bin:/bin', HOME: '/' }, cwd: '/' }), 240000, 'the echo service');
  eq('a provider of the page\'s own: negotiated, answered, an event unasked', e.stdout,
    'echo: {"service":"echo","version":2}\necho: {"seq":4,"back":"ping"}\necho: {"op":"event"}\n', e.stderr.trim().split('\n').slice(-3).join(' / '));
  await new Promise((res) => setTimeout(res, 500));
  eq('and it heard the connection come and go, and the data', raw, ['open echo v2', 'ping 3', 'close']);
  await sys.shutdown();
}
