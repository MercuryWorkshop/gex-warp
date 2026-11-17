// A librarian that only listens (fixtures/darwin/midi.c `listen`): connecting
// an input port to a browser source asks for the browser's second permission,
// once however many ports connect, and does not wait for it.
import { listen } from './midi-helpers.js';
import { tail } from './midi-helpers.js';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 900 };

// Granted: the system exclusive an input sends is delivered with the rest.
export default async function (t) {
  const { eq } = t;
  const { r, b, events } = await listen(t, false);
  eq('both ports were given all three, system exclusive too', r.stdout.split('\n').filter((l) => l.startsWith('port')), [
    'port 1: 90 3c 64', 'port 1: b0 07 7f', 'port 1: f0 7d 10 f7',
    'port 2: 90 3c 64', 'port 2: b0 07 7f', 'port 2: f0 7d 10 f7',
  ], tail(r));
  eq('the second permission was asked of the browser once', b.requests, [{ sysex: false }, { sysex: true }]);
  eq('and the service heard it, once, after the first', events.filter((e) => e.startsWith('access')), ['access plain true', 'access sysex true']);
}
