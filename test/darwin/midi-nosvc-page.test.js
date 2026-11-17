// CoreMIDI of pyrus's, over the page's `midi` service (page/runtime
// A page that does not provide the `midi` service: a Mac with no MIDI
// hardware, whose virtual endpoints work all the same.
import { bootMidi, ENV as env, tail } from './midi-helpers.js';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 900 };

export default async function (t) {
  const { eq, check, timeout } = t;
  // ---- no `midi` service on the page: a Mac with no MIDI hardware
  {
    const { sys } = await bootMidi(t, null);
    const r = await timeout(sys.run(['/usr/local/bin/midi', 'denied'], { env, cwd: '/' }), 300000, 'the program on a page without the service');
    check('the same, with no service at all', r.stdout.startsWith('client: 0\nno browser MIDI: 0 devices\n') && r.stdout.includes('destination got: 90 3c 64'), r.stdout + tail(r));
    await sys.shutdown();
  }

}
