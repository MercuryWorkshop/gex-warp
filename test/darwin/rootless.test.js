// What System Integrity Protection keeps (fixtures/darwin/rootless.c),
// against the Mac: a sandbox check of writing a file the system keeps is
// denied, for launchd and for anybody, which libsystem_sandbox's
// rootless_check_trusted takes for a trusted file (Mojave's CVMServer loads
// OpenCL's CPU compiler only from one); anywhere else it is allowed.
import { darwin } from '../lib/darwin.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 120 };

export default async function (t) {
  const d = darwin(t);
  await d.compare('rootless', await d.build('rootless'));
  d.n.clean();
}
