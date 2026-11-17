// Kernel control sockets (fixtures/darwin/kctl.c), against the Mac:
// socket(PF_SYSTEM, SOCK_DGRAM, SYSPROTO_CONTROL), the network statistics'
// control found by name (CTLIOCGINFO) and connected to by its id, and what
// is refused (a name or an id nobody registered, the ioctl of a file).
// symptomsd opens the statistics' at start and aborts without it. What the
// control answers is the kernel's unit tests' (crates/gex-kernel/src/darwin/kctl.rs):
// on the Mac it reports the machine's own sockets.
import { darwin } from '../lib/darwin.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 120 };

export default async function (t) {
  const d = darwin(t);
  await d.compare('kctl', await d.build('kctl'));
  d.n.clean();
}
