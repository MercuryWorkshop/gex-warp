// CoreMIDI of pyrus's, over the page's `midi` service (page/runtime
// Two processes that make virtual endpoints between them, on a page whose
// `midi` service is never asked (fixtures/darwin/midipeers.c, which prints on a
// Mac what midipeers.expected says).
import { bootMidi, ENV as env, tail } from './midi-helpers.js';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], images: ['darwin-root'], timeout: 900 };

export default async function (t) {
  const { eq, check, timeout } = t;
  // ---- two processes, and the service not provided
  {
    const { sys } = await bootMidi(t, null);
    const want = readFileSync(join(import.meta.dirname, '..', 'fixtures', 'darwin', 'midipeers.expected'), 'utf8').replace(/exit 0\n$/, '');
    const r = await timeout(sys.run(['/usr/local/bin/midipeers'], { env, cwd: '/' }), 400000, 'the pair of processes');
    eq('virtual endpoints between two processes print what a Mac prints', r.stdout, want, tail(r));
    await sys.shutdown();
  }
}
