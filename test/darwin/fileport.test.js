// File ports (fixtures/darwin/fileport.c), against the Mac: an open file as
// a send right (fileport_makeport) and a descriptor for it again
// (fileport_makefd): closed on exec, the file's offset shared, the file
// outliving the descriptor it came from, and what is refused. libxpc sends
// a descriptor in a message so (CVMServer is handed one by an OpenCL build).
import { darwin } from '../lib/darwin.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 120 };

export default async function (t) {
  const d = darwin(t);
  await d.compare('fileport', await d.build('fileport'));
  d.n.clean();
}
