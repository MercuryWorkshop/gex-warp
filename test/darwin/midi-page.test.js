// CoreMIDI of pyrus's, over the page's `midi` service (page/runtime
// src/midi.js): a Mac program in the way of a studio's (fixtures/darwin/
// midi.c) sees the browser's MIDI inputs and outputs as devices, hears them
// come and go, receives what an input sends, and sends to an output, now, at
// a time and as system exclusive (which asks the browser for its second
// permission, once). The browser is a test's own: `requestMIDIAccess` here
// is a mock whose ports the test moves. Then a page whose browser refuses
// (the program has no devices, and virtual endpoints work all the same), and
// two processes that make virtual endpoints between them with the page's
// service never asked (fixtures/darwin/midipeers.c, which prints on a Mac
// what midipeers.expected says).
import { FakePort, FakeOutput, fakeBrowser, bootMidi, ENV as env, tail } from './midi-helpers.js';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 900 };

export default async function (t) {
  const { eq, check, timeout } = t;
  // ---- the studio's program, and a browser with a keyboard and a synthesizer
  {
    const b = fakeBrowser();
    const { sys, midi } = await bootMidi(t, { requestMIDIAccess: b.request });
    const events = [];
    for (const type of ['connect', 'access', 'listen', 'send', 'disconnect']) {
      midi.addEventListener(type, (e) => events.push(`${type}${e.sysex !== undefined ? (e.sysex ? ' sysex' : ' plain') : ''}${e.port ? ` ${e.port}` : ''}${e.on !== undefined ? ` ${e.on}` : ''}`));
    }
    // The program listens to the keyboard: it plays three messages, and the
    // browser reads what the program sends; after its system exclusive a
    // pad is plugged in and the keyboard unplugged.
    midi.addEventListener('listen', ({ port, on }) => {
      if (port !== 'in-keys' || !on) return;
      setTimeout(() => {
        b.keys.fire([0x90, 0x3c, 0x64]);
        b.keys.fire([0xb0, 0x07, 0x7f]);
        b.keys.fire([0xf0, 0x7d, 0x10, 0xf7]);
      }, 50);
    });
    midi.addEventListener('send', ({ data }) => {
      if (data[0] !== 0xf0) return;
      setTimeout(() => {
        const padIn = new FakePort('in-pad', 'Late Pad', 'input'), padOut = new FakeOutput('out-pad', 'Late Pad', 'output');
        b.access.inputs.set(padIn.id, padIn);
        b.access.outputs.set(padOut.id, padOut);
        b.access.change(padIn);
        b.access.change(padOut);
        setTimeout(() => {
          b.keys.state = 'disconnected';
          b.access.change(b.keys);
        }, 150);
      }, 100);
    });
    const r = await timeout(sys.run(['/usr/local/bin/midi'], { env, cwd: '/' }), 300000, 'the studio program');
    const out = r.stdout;
    const line = (s) => out.split('\n').find((l) => l.startsWith(s)) ?? null;
    check('the program ran to its end', r.code === 0 || r.code === undefined, `${r.code}: ${tail(r)}`);
    eq('the start: the keyboard and the synthesizer are two devices',
      out.split('\n').slice(0, 11), [
        'client: 0',
        'at the start: 2 devices',
        " device 'Fake Keys' by 'Acme', model 'Fake Keys', offline 0, 1 entities",
        "  entity 'Fake Keys': 1 sources, 0 destinations",
        "   source 'Fake Keys' (back to its device 1, offline 0, protocol 1)",
        " device 'Fake Synth' by 'Acme', model 'Fake Synth', offline 0, 1 entities",
        "  entity 'Fake Synth': 0 sources, 1 destinations",
        "   destination 'Fake Synth'",
        'at the start: 1 sources, 1 destinations',
        'found: 1 1',
        "the source's display name: 'Fake Keys', manufacturer 'Acme'",
      ], tail(r));
    eq('connected to the keyboard', line('connect:'), 'connect: 0');
    const messages = out.split('\n').filter((l) => l.startsWith('message:'));
    eq('what the keyboard played, with its time and the connection', messages, [
      'message: 90 3c 64, time in range 1, connection 7',
      'message: b0 07 7f, time in range 1, connection 7',
      'message: f0 7d 10 f7, time in range 1, connection 7',
    ], tail(r));
    eq('the program sent and was told', ['send:', 'send later:', 'send system exclusive:'].map(line), ['send: 0', 'send later: 0', 'send system exclusive: 0']);
    check('and its system exclusive was sent', out.includes('sysex sent: complete 1, 0 left'));
    eq('a device came and one went', line('a device came:'), 'a device came: 1, one went: 1');
    check('told of the device, its endpoints and that the setup changed', ["notification: a device came: 'Late Pad'", 'notification: an endpoint came, type 2, on the run loop\'s thread 1',
      'notification: an endpoint came, type 3, on the run loop\'s thread 1', "notification: 'Fake Keys' is now offline 1", 'notification: the setup changed'].every((s) => out.includes(s)), out);
    const end = out.split('\n').slice(out.split('\n').indexOf('at the end: 3 devices'));
    eq('at the end: the keyboard is still a device, offline, and not in the lists', end.filter((l) => l.startsWith(' device') || l.startsWith('at the end')), [
      'at the end: 3 devices',
      " device 'Fake Keys' by 'Acme', model 'Fake Keys', offline 1, 1 entities",
      " device 'Fake Synth' by 'Acme', model 'Fake Synth', offline 0, 1 entities",
      " device 'Late Pad' by 'Acme', model 'Late Pad', offline 0, 1 entities",
      'at the end: 1 sources, 2 destinations',
    ], out);
    // The browser's side.
    eq('the notes the program sent', b.synth.sent.slice(0, 2).map((s) => s.data), [[0x90, 0x3c, 0x64], [0x80, 0x3c, 0x00]]);
    eq('now has no time', b.synth.sent[0].t, null);
    check('and the other is for 150 ms after it was sent', b.synth.sent[1].t != null && Math.abs(b.synth.sent[1].t - b.synth.sent[0].at - 150) < 120, `${b.synth.sent[1].t} ${b.synth.sent[0].at}`);
    eq('the system exclusive message, whole', b.synth.sent[2]?.data, [0xf0, 0x7d, 0x01, 0x02, 0xf7]);
    eq('the browser was asked for the first permission, and the second when system exclusive was sent', b.requests, [{ sysex: false }, { sysex: true }]);
    check('the keyboard was opened', b.keys.opened >= 1);
    eq('the service heard it', events.filter((e) => !e.startsWith('send')), ['connect', 'access plain', 'listen in-keys true', 'access sysex', 'disconnect']);
    await sys.shutdown();
  }

}
