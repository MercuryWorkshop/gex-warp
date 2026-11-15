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
// it is not answered as the receiver's own would be. That part is not
// asked of the Mac: its answers are 10.14's, and a Mac of today may not
// let a task's control port leave its task.
//
// Under gexc only: on the page each process has a kernel of its own, and a
// port does not yet leave its task there.
import { darwin } from '../lib/darwin.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 300 };

export default async function (t) {
  const d = darwin(t);
  const prog = await d.build('machipc');
  await d.compare('machipc', prog, [], { timeout: 60000 });
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
  d.n.clean();
}
