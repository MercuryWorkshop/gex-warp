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
// And a task's own port sent to another task (`machipc task`), as a client
// tells securityd or WindowServer who it is: the receiver's name for it is
// not its own task's, pid_for_task says whose task it is (securityd refuses
// a client whose task port and audit token disagree), and what is asked of
// it is not answered as the receiver's own would be. And mach_port_destroy
// by its trap, which ends every right a name is. That part is not asked
// of the Mac: its answers are 10.14's, and a Mac of today may not let a
// task's control port leave its task, or have that trap.
//
// And send-once rights that die unused (`machipc once`), against the Mac:
// one the receiver took and never answered, one still queued on its port
// when the receiver ends -- each reply port is told (MACH_NOTIFY_SEND_ONCE),
// which is how an XPC client's wait for a reply ends when its server ends
// without one.
//
// And a receive right that asked to be given back when its holder ends
// (`machipc destroyed`), against the Mac, as an XPC client's connection
// port does: it comes back (MACH_NOTIFY_PORT_DESTROYED) under the name of
// the send right the parent kept, in XNU's 40 bytes (MIG's notify servers
// check the size: libxpc's refused gex's 48 and left fontd waiting for
// good), its queued requests with it; destroyed, the one still queued tells
// its reply port. The dead-name and no-senders notices' sizes are said too.
//
// Under gexc only: on the page each process has a kernel of its own, and a
// port does not yet leave its task there.
import { darwin } from '../lib/darwin.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 300 };

export default async function (t) {
  const d = darwin(t);
  const prog = await d.build('machipc');
  await d.compare('machipc', prog, [], { timeout: 60000 });
  await d.compare('machipc once', prog, ['once'], { timeout: 60000 });
  await d.compare('machipc destroyed', prog, ['destroyed'], { timeout: 60000 });
  const r = await d.gex(prog, ['task'], { timeout: 60000 });
  const lines = r.stdout.split('\n');
  const has = (line, what) => t.check(what, lines.includes(line), `exit ${r.code}: ${JSON.stringify(r.stdout)} / ${r.stderr.trim().split('\n').slice(-2).join(' / ')}`);
  has('received: 0, id 500, 1 descriptors', 'a task port: the child sends its own');
  has('the child\'s task port: disposition 17, a name of its own', 'a task port: it arrives under a name that is not the receiver\'s task\'s');
  has('pid_for_task of the parent\'s: 0, the parent', 'pid_for_task: the caller\'s own task is the caller');
  has('pid_for_task of the child\'s: 0, the child', 'pid_for_task: the child\'s task port is the child');
  has('pid_for_task of a port that is no task: refused', 'pid_for_task: a port that is no task is refused');
  // (KERN_FAILURE: gex does not yet ask anything of another task.)
  has('the child\'s bootstrap port, asked of its task: refused, 5', 'a task port: what is asked of the child\'s is not answered as the parent\'s own');
  has('the child: waited for, status 0', 'a task port: the child ends as it should');
  // mach_port_destroy, the trap (the Security framework ends a thread's
  // reply port to securityd with it).
  has('a receive right and a send right: type 30000', 'mach_port_destroy: a name with a receive right and a send right');
  has('destroyed: 0, its type now 0', 'mach_port_destroy: both go, and the name');
  has('destroyed again: 15', 'mach_port_destroy: no such name then');
  has('the name for the child\'s task port destroyed: 0, its type now 0', 'mach_port_destroy: a send right to another task\'s port');
  d.n.clean();
}
