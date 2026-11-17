// What the midi-*-page tests share: a mock of the browser's Web MIDI (ports
// that are EventTargets a test moves), and a page with a Mac's root and the
// programs of fixtures/darwin/midi.c and midipeers.c in it.
import { System } from '../../page/runtime/src/index.js';
import { MemoryFilesystem } from '../../page/runtime/src/fs.js';
import { Midi } from '../../page/runtime/src/midi.js';
import { bytesOf } from '../../page/runtime/src/internal.js';

/** A port as Web MIDI has it: what a test needs of an EventTarget. */
export class FakePort extends EventTarget {
  constructor(id, name, type, state = 'connected') {
    super();
    Object.assign(this, { id, name, manufacturer: 'Acme', version: '1.0', type, state, connection: 'closed' });
    this.opened = 0;
  }
  open() {
    this.opened++;
    this.connection = 'open';
    return Promise.resolve(this);
  }
  /** A message from an input, as the browser delivers it. */
  fire(bytes) {
    const e = new Event('midimessage');
    Object.defineProperty(e, 'data', { value: new Uint8Array(bytes) });
    Object.defineProperty(e, 'timeStamp', { value: performance.now() });
    this.dispatchEvent(e);
  }
}
export class FakeOutput extends FakePort {
  sent = [];
  cleared = 0;
  send(data, t) {
    this.sent.push({ data: [...data], t: t ?? null, at: performance.now() });
  }
  clear() {
    this.cleared++;
  }
}

export function fakeBrowser() {
  const keys = new FakePort('in-keys', 'Fake Keys', 'input');
  const synth = new FakeOutput('out-synth', 'Fake Synth', 'output');
  const access = {
    inputs: new Map([[keys.id, keys]]),
    outputs: new Map([[synth.id, synth]]),
    onstatechange: null,
    change(port) {
      this.onstatechange?.({ port });
    },
  };
  const requests = [];
  return { keys, synth, access, requests, request: async (o) => { requests.push({ ...o }); return access; } };
}


/** A page with the programs in a Mac's root, and the `midi` service made
 *  with `options` (none: the page does not provide it). */
export async function bootMidi(t, options) {
  const { timeout } = t;
  const arch = t.arch === 'x86' ? 'x86' : 'aarch64';
  let studio, peers;
  try {
    studio = await bytesOf(t.work(`darwin-midi.${arch}`));
    peers = await bytesOf(t.work(`darwin-midipeers.${arch}`));
  } catch {
    t.skip(`no Mac root or no Mac compiler for ${t.arch} (GEX_TEST_DARWIN_ROOT${t.arch === 'x86' ? '_X86' : ''}: pyrus builds one)`);
  }
  const { arch: _, ...debug } = t.debug();
  const sys = new System({ baseUrl: t.baseUrl, arch, debug });
  const fs = new MemoryFilesystem(sys);
  fs.addTar(t.work(`darwin-root.${arch}.tar`));
  fs.addFiles({ '/usr/local/bin/midi': studio, '/usr/local/bin/midipeers': peers }, { mode: 0o755 });
  const midi = options ? new Midi(sys, options) : null;
  await timeout(sys.boot(), 120000, 'boot');
  return { sys, midi };
}

export const ENV = { PATH: '/usr/bin:/bin', HOME: '/' };
export const tail = (r) => r.stderr.trim().split('\n').slice(-3).join(' / ');
