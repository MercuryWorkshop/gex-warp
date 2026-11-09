// The Darwin personality under gexc: static Mach-O programs that make XNU's
// system calls and Mach traps directly (fixtures/darwin, over its own small
// C library), each checked against the same source run on the Mac itself
// (lib/darwin.mjs). What a new process is handed on its stack, then files
// and directories in Darwin's layouts and error numbers.
import { darwin } from '../lib/darwin.mjs';

export const meta = { hosts: ['node'], arches: ['a64'], timeout: 600 };

export default async function (t) {
  const d = darwin(t);
  await d.compare('hello', await d.build('hello'), ['one', 'two words']);
  await d.compare('files', await d.build('files'));
  d.n.clean();
}
