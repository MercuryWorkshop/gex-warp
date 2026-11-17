// A librarian that only listens (fixtures/darwin/midi.c `listen`): connecting
// an input port to a browser source asks for the browser's second permission,
// once however many ports connect, and does not wait for it.
import { listen } from './midi-helpers.js';
import { tail } from './midi-helpers.js';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 900 };

// Refused: system exclusive from an input is dropped, the rest is delivered, and
// the browser is not asked again.
export default async function (t) {
  const { eq } = t;
  const { r, b, events } = await listen(t, true);
  eq('both ports were given the notes and not the system exclusive', r.stdout.split('\n').filter((l) => l.startsWith('port')), [
    'port 1: 90 3c 64', 'port 1: b0 07 7f',
    'port 2: 90 3c 64', 'port 2: b0 07 7f',
  ], tail(r));
  eq('asked once, and refused', b.requests, [{ sysex: false }, { sysex: true }]);
  eq('the service heard it', events.filter((e) => e.startsWith('access')), ['access plain true', 'access sysex false']);
}
