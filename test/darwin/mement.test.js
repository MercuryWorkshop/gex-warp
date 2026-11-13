// Named memory entries (fixtures/darwin/mement.c), against the Mac:
// mach_make_memory_entry_64 of memory a task has, of memory made for the
// entry (MAP_MEM_NAMED_CREATE) and of a range of another entry, each mapped
// with mach_vm_map and seen to be the same pages (or a copy, asked for one);
// what is refused, and why; and mach_vm_read. In one task, under gexc and on
// the page's runtime. Then between two tasks, under gexc: a forked child
// names a page of its own and memory made for it, sends the entries to its
// parent, and each sees the other's stores while both run (natively the
// tasks' mappings of an entry are one memory: crates/gex-kernel/src/darwin/mement.rs).
// On the page a port does not leave its task, so that half is gexc's only.
import { darwin } from '../lib/darwin.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 300 };

export default async function (t) {
  const d = darwin(t);
  const prog = await d.build('mement');
  await d.compare('mement', prog, [], { page: true, timeout: 60000 });
  await d.compare('mement fork', prog, ['fork'], { timeout: 60000 });
  d.n.clean();
}
