// IOKit's registry through its MIG interface (fixtures/darwin/iokit.c),
// against the Mac's: the master port, the binary matching dictionary, a
// service found and what the registry says of it, an entry by path and
// nothing for a path to nothing, a service with no user client, a routine
// there is none of. What gex's registry has (darwin/iokit in the kernel)
// is a machine of its own; only what every Mac's has is asked.
import { darwin } from '../lib/darwin.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 300 };

export default async function (t) {
  const d = darwin(t);
  await d.compare('iokit', await d.build('iokit'), [], { timeout: 60000 });
  d.n.clean();
}
