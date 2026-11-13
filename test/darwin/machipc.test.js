// Mach between two tasks (fixtures/darwin/machipc.c), against the Mac: a
// port made the parent's bootstrap port (task_set_special_port) and found
// by its forked child as its own (task_get_special_port); a message from
// the child through it, sent with mach_msg_trap as 10.14's libSystem sends
// (the Mac's build sends it with mach_msg2: a Mac of today ends a program
// that uses the older trap), carrying a send right to a port of the
// child's, memory out of line and a send-once right to reply by, received
// with an audit trailer that names the child; the reply by that send-once
// right, with a send right to a second port the child then sends to; and
// the child's end, which tells the parent that its name for the child's
// port is a dead name and that its second port has no senders left.
//
// Under gexc only: on the page each process has a kernel of its own, and a
// port does not yet leave its task there.
import { darwin } from '../lib/darwin.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 300 };

export default async function (t) {
  const d = darwin(t);
  await d.compare('machipc', await d.build('machipc'), [], { timeout: 60000 });
  d.n.clean();
}
