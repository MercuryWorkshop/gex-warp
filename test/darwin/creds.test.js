// Who a process is, for a user who is not root (fixtures/darwin/creds.c,
// crates/gex-kernel/src/syscalls.rs `Creds`, darwin/bsd.rs): the ids and
// the groups beside them as getgroups lists them (the host user's own, the
// effective group first), what an unprivileged process is refused
// (setgroups, setuid and its kin, setlogin), the ids it may set to its own,
// issetugid and the saved ids, and chown and chmod on its own file: to a
// group it is in beside its own, not to another or to another user, the
// set-user-ID bit gone with the group given. The same lines on this Mac and
// under gexc. Both processors. What only root can do is setuid.test.js's,
// under gexc alone.
import { darwin } from '../lib/darwin.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

export default async function (t) {
  const d = darwin(t);
  const prog = await d.build('creds');
  await d.compare('creds', prog);
}
