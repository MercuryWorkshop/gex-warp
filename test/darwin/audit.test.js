// Audit sessions under gexc (crates/gex-kernel/src/darwin/audit.rs,
// fixtures/darwin/audit.c), as XNU has them: what a Mac's login session is
// to the kernel, which launchd makes and loginwindow, the security server
// and the window server ask after. As root, since making one is the
// superuser's; the Mac cannot be asked without it.
//
// The first process is in the session the machine began in (id 100000, no
// audit user, the initial one: as launchd's is; XNU's default session, 0,
// which is none, is never seen). setaudit_addr makes one with the next id
// the kernel chooses and the caller is in it; its flags change only as the
// masks the superuser sets allow (audit.session.*_sflags_mask), its audit
// user is set once; a session has a port (audit_session_self,
// audit_session_port), and holding it is the right to join
// (audit_session_join); a fork's child and the program it runs are in
// their parent's, and a user in one is a member: what it may clear is the
// session's for everyone.
import { darwin } from '../lib/darwin.mjs';

export const meta = { hosts: ['node'], arches: ['a64', 'x86'], timeout: 600 };

export default async function (t) {
  const d = darwin(t);
  const prog = await d.build('audit');
  const r = await d.gex(prog, [], { gexc: ['--uid', '0'] });
  const lines = r.stdout.split('\n');
  const has = (line, what) => t.check(what, lines.includes(line), `exit ${r.code}: ${JSON.stringify(r.stdout)} / ${r.stderr.trim().split('\n').slice(-2).join(' / ')}`);
  const NONE = 4294967295;
  t.check('the program completes', r.code === 0 && !lines.some((l) => l.startsWith('FAIL')), `exit ${r.code}: ${lines.filter((l) => l.startsWith('FAIL')).join(' / ')} ${r.stderr.trim().split('\n').slice(-2).join(' / ')}`);

  has(`at the start: getaudit_addr 0: audit user ${NONE} session 100000 flags 0x1 terminal type 4`, 'the first process is in the machine\'s first session');
  has('ok the first session has a port', 'which has a port');
  has('flags 0x1', 'and is the initial one');
  has('setaudit_addr(no id) = 0', 'a session asked for by no id is a new one');
  has('assigned session 100001', 'with the next id');

  has('setaudit_addr(AU_ASSIGN_ASID, initial) = 0', 'the superuser makes a session');
  has('assigned session 100002', 'the kernel chooses its id, written back');
  has(`in the system's session: getaudit_addr 0: audit user ${NONE} session 100002 flags 0x1 terminal type 4`, 'and the caller is in it');
  has('setaudit_addr with no terminal type = -22', 'a terminal of no address type is refused');

  has('mask 0x0', 'no flag may change until the superuser says which');
  has('A_SETSFLAGS(authenticated) with no mask = -22', 'so none does');
  has('set the superuser\'s set mask = 0', 'the superuser sets a mask');
  has('set the member\'s clear mask = 0', 'and another');

  has('setaudit_addr(AU_ASSIGN_ASID, user 501, graphic and tty) = 0', 'a login\'s session is made');
  has('in the login\'s session: getaudit_addr 0: audit user 501 session 100003 flags 0x30 terminal type 4', 'with the next id, its user and its flags');
  has('audit user 501', 'getauid is the session\'s audit user');
  has('setauid(502), once it is set = -22', 'which is set once');
  has('A_SETSFLAGS(authenticated) by the superuser = 0', 'the superuser sets a flag its mask has');
  has('A_SETSFLAGS(console), which no mask has = -22', 'and no other');
  has('flags 0x4030', 'the flags are as set');

  has('ok audit_session_self is a port, the same one each time', 'a session has one port');
  has('audit_session_port(the system\'s) = 0', 'the superuser is given another session\'s port');
  has('ok the caller\'s own by -1 is the same port, the system\'s another', 'and its own by -1');
  has('audit_session_port(a session there is none of) = -22', 'a session there is none of has no port');
  has('audit_session_join(a name that is no port) = -22', 'a name that is no right joins nothing');
  has('audit_session_join(the task\'s own port) = -22', 'nor a port that is no session\'s');

  has('a fork\'s child: getaudit_addr 0: audit user 501 session 100003 flags 0x4030 terminal type 4', 'a fork\'s child is in its parent\'s session');
  has('audit_session_port(the system\'s) as a user = -1', 'a user is not given another session\'s port');
  has('setaudit_addr as a user = -1', 'nor may it set a session');
  has(`the system's: audit user ${NONE} flags 0x1 masks ffffffff ffffffff`, 'a user is told of a session by its id, without its masks');
  has('A_SETSFLAGS(not authenticated) by a member = 0', 'a member clears a flag the member mask has');
  has('A_SETSFLAGS(authenticated) by a member = -22', 'and cannot set it again');
  has('the program it runs: getaudit_addr 0: audit user 501 session 100003 flags 0x30 terminal type 4', 'the program a child runs is in the session too');
  has('the login\'s: audit user 501 flags 0x30 masks 0 0', 'what a member cleared is the session\'s, for everyone');
  has('A_GETSINFO_ADDR(a session there is none of) = -22', 'no session, no record');

  has('audit_session_join(the system\'s) = 100002', 'holding a session\'s port joins it');
  has(`joined to the system's: getaudit_addr 0: audit user ${NONE} session 100002 flags 0x1 terminal type 4`, 'and the caller is in that one');
  has('audit_session_join(the login\'s) = 100003', 'and back');
  has('joined to the login\'s again: getaudit_addr 0: audit user 501 session 100003 flags 0x30 terminal type 4', 'in the login\'s again');
  has('setaudit_addr(the system\'s id) = 0', 'a session named by its id is moved into');
  has(`moved to the system's: getaudit_addr 0: audit user ${NONE} session 100002 flags 0x1 terminal type 4`, 'as it is');
  d.n.clean();
}
