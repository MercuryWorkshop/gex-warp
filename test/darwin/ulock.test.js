// ulock, as os_unfair_lock and libdispatch ask it (fixtures/darwin/ulock.c),
// against the Mac: a wait on a word that has changed, and a wake for
// nobody; an unfair lock whose owner (the thread its word names) has
// ended, one whose word names no thread, one the caller holds itself --
// each answered at once, as XNU looks the owner up, not slept on; and two
// threads taking one unfair lock by turns many times, with no wait left
// hanging (a lost wake).
import { darwin } from '../lib/darwin.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 300 };

export default async function (t) {
  const d = darwin(t);
  await d.compare('ulock', await d.build('ulock'), [], { timeout: 120000 });
  d.n.clean();
}
