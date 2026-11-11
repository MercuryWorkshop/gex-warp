// Processes a fork parted, each making different code at one address (as a
// loader that puts a library where the address space has room does in a
// parent and its child): each runs its own, in every order -- the child
// first, the parent first, two children taking turns, a child forked after
// its sibling made its code, a function replaced while a relative still
// calls the old one, and either side with a second thread. A fork's child
// shares its parent's translations until one of them changes its code,
// and must never run a translation of the other's.
import { native, trim, tail } from '../lib/native.mjs';

export const meta = { hosts: ['node'], timeout: 300 };

const WANT = `child first: 11
then its parent: 22
parent first: 33
then a child forked before: 44
the parent again: 33
one child: 55
the other: 66
one child again: 55
the other again: 66
an older child: 77
a younger child: 88
the older child again: 77
before the fork: 99
the child calls it: 99
the parent's new one: 100
the child's is the old one still: 99
the child's new one: 111
the parent's is its own still: 100
the child of two threads: 122
the parent of two threads: 133
its thread, after: 133
the child's thread's code: 144
its parent's: 155
the child's, after: 144`;

export default async function (t) {
  const n = native(t);
  const d = n.dir('fork-code');
  // With the program's flush of the instruction cache and without it (an
  // AArch64 `ic ivau` retires the page, which alone used to hide this
  // there).
  for (const [name, flags] of [['flushed', []], ['unflushed', ['-DNO_FLUSH']]]) {
    const prog = `${d}/code-${name}`;
    await n.cc(['-O1', '-static', ...flags, '-o', prog, n.src('fork-code/code.c'), '-lpthread']);
    const r = await n.gexc(['run', prog], { timeout: 120000 });
    t.eq(`native, ${name}`, trim(r.stdout), WANT, tail(r.stderr));
  }
  n.clean();
}
