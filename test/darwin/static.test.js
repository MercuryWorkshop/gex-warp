// The Darwin personality under gexc: static Mach-O programs that make XNU's
// system calls and Mach traps directly (fixtures/darwin, over its own small
// C library), each checked against the same source run on the Mac itself
// (lib/darwin.mjs), under gexc and on the page's runtime. What a new
// process is handed on its stack; files and
// directories in Darwin's layouts and error numbers; fork, wait4, kill and
// execve; mmap on Darwin's page and Mach's virtual memory traps; the clocks
// and the calls that wait; signals, delivered through XNU's frame to a
// handler behind its trampoline; and threads, made as libpthread asks for
// them (on the Mac, by its libpthread), waiting on ulock.
import { darwin } from '../lib/darwin.mjs';

export const meta = { hosts: ['node'], arches: ['a64'], timeout: 600 };

export default async function (t) {
  const d = darwin(t);
  await d.compare('hello', await d.build('hello'), ['one', 'two words'], { page: true });
  // `files` says what the filesystem under it says (an empty directory's
  // size, a name's longest length): the Mac's own under gexc, and so not
  // the page's in-memory one.
  await d.compare('files', await d.build('files'));
  for (const name of ['proc', 'mem', 'time', 'sig', 'thr']) await d.compare(name, await d.build(name), [], { page: true });
  d.n.clean();
}
