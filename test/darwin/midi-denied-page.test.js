// CoreMIDI of pyrus's, over the page's `midi` service (page/runtime
// A page whose browser refuses MIDI (and, below, one that has no `midi`
// service at all): the program (fixtures/darwin/midi.c `denied`) sees no
// devices, and virtual endpoints work between its own ports all the same.
import { fakeBrowser, bootMidi, ENV as env, tail } from './midi-helpers.js';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 900 };

export default async function (t) {
  const { eq, check, timeout } = t;
  // ---- a browser that refuses
  {
    const b = fakeBrowser();
    const asked = [];
    const { sys } = await bootMidi(t, { requestMIDIAccess: b.request, authorize: async (r) => { asked.push(r.sysex); return false; } });
    const r = await timeout(sys.run(['/usr/local/bin/midi', 'denied'], { env, cwd: '/' }), 300000, 'the program on a page that refuses');
    eq('no devices, and virtual endpoints work all the same', r.stdout, [
      'client: 0',
      'no browser MIDI: 0 devices',
      'no browser MIDI: 0 sources, 0 destinations',
      'the virtual ones are listed: 1 1',
      'destination got: 90 3c 64',
      'port got: 90 3c 64',
      "system exclusive between the program's own endpoints: destination got: f0 7d 01 02 f7",
      'sysex sent: complete 1, 0 left',
      '',
    ].join('\n'), tail(r));
    eq('the shell was asked once, not for system exclusive', asked, [false]);
    eq('and nothing was asked of the browser', b.requests, []);
    await sys.shutdown();
  }

}
