//! Audit sessions, as XNU has them (`bsd/security/audit/audit_session.c`):
//! what a Mac calls a login session where the kernel is concerned. Every
//! process is in one, by its credential; a fork's child and the next
//! program are in their parent's. A session has an id (`au_asid_t`), the
//! audit user (who logged in: all ones until somebody did), a terminal, and
//! flags that say what the session may reach: the window server
//! (`AU_SESSION_FLAG_HAS_GRAPHIC_ACCESS`), a terminal, the console, and
//! whether its user has authenticated. launchd makes one for the system
//! and one for each login, loginwindow's among them, and the security
//! server and the window server ask which one a client is in (its audit
//! token carries the id) and what its flags are.
//!
//! - `getaudit_addr` and `setaudit_addr` read and set the caller's, as an
//!   `auditinfo_addr_t`. Setting is the superuser's. A session id of
//!   `AU_ASSIGN_ASID` (or none, 0) makes a new session with the next id
//!   from 100000 and puts the caller in it, the id written back; an id no
//!   session has makes that session; an id one has moves the caller into
//!   it and changes it, within what may change: the audit user and the
//!   terminal only while they are the default, and a flag only as the four
//!   masks allow (`audit.session.superuser_set_sflags_mask` and the three
//!   beside it, `sysctl`s the superuser sets; all zero until then, as
//!   XNU's are until launchd sets them).
//! - `getauid` and `setauid`: the session's audit user. Setting it in no
//!   session makes one.
//! - `auditon`: `A_GETSINFO_ADDR`, any session's record by its id (its
//!   masks all ones to anyone but the superuser), and `A_GETSFLAGS` and
//!   `A_SETSFLAGS`, the caller's session's flags (set by a member as the
//!   member masks allow).
//! - `audit_session_self` is a send right to the port that names the
//!   caller's session, `audit_session_port` to any session's (another's is
//!   the superuser's to ask for), and `audit_session_join` puts the caller
//!   in the session a port names: holding the port is the right to join.
//!   The port is the kernel's ([`Kind::AuditSession`]) and lasts while
//!   somebody has a right to it; the next asked for is a new one.
//!
//! XNU's own first process is in a default session, id 0, which is none
//! (it has no port, and nothing can be asked of it by its id), and launchd
//! makes the system's before it starts anything, so no program a Mac runs
//! has seen the default one; LaunchServices ends a process it finds there.
//! So the machine's first process here is in a session made for it as
//! launchd's is ([`session`]): the first id, 100000, no audit user, and
//! the flag that says it is the initial one.
//!
//! Not kept: who is in a session (one lasts as long as the machine does,
//! where XNU's goes with its last process), the per-credential event masks
//! apart from the session's record, and the changes `/dev/auditsessions`
//! would tell of (nothing is written there). The sessions are this
//! kernel's: natively every process's; on the page each process has a
//! kernel of its own, which knows the session it was born in as it was
//! then ([`adopt`]) and no change made in another process since.

use super::port::{Kind, Port};
use super::*;
use std::collections::HashMap;
use std::sync::{Mutex, Weak};

/// `sizeof(auditinfo_addr_t)`, and where its fields are: the audit user,
/// the two event masks, the terminal (a port, an address type and four
/// words of address), the session's id, and the flags.
pub const INFO: usize = 48;
const AUID: usize = 0;
const MASK: usize = 4;
const TERMID: usize = 12;
const TERM_TYPE: usize = 16;
const ASID: usize = 36;
const FLAGS: usize = 40;

/// `AU_DEFAUDITID`: no audit user.
pub const AU_DEFAUDITID: u32 = u32::MAX;
/// `AU_ASSIGN_ASID`: the kernel is to choose the id.
const AU_ASSIGN_ASID: u32 = u32::MAX;
/// The ids the kernel chooses from: above any process's number.
const ASSIGNED_ASID_MIN: u32 = 100_000;
const ASSIGNED_ASID_MAX: u32 = u32::MAX - 1;
/// The terminal address types there are.
const AU_IPV4: u32 = 4;
const AU_IPV6: u32 = 16;
/// `AU_SESSION_FLAG_IS_INITIAL`: the session the system began in.
const IS_INITIAL: u64 = 0x1;
/// `auditon`'s commands answered here.
pub(super) const A_GETSINFO_ADDR: u64 = 32;
pub(super) const A_GETSFLAGS: u64 = 39;
pub(super) const A_SETSFLAGS: u64 = 40;

/// The `sysctl`s that say which flags may be set and cleared, by the
/// superuser and by a session's member, in the order [`Table::masks`]
/// keeps them.
pub(super) const MASKS: [&str; 4] =
    ["audit.session.superuser_set_sflags_mask", "audit.session.superuser_clear_sflags_mask", "audit.session.member_set_sflags_mask", "audit.session.member_clear_sflags_mask"];

struct Session {
    info: [u8; INFO],
    /// The port that names it, while a right to one is held.
    port: Weak<Port>,
}

#[derive(Default)]
struct Table {
    sessions: HashMap<u32, Session>,
    /// The last id chosen.
    last: u32,
    masks: [u64; 4],
}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Table) -> R) -> R {
    let mut t = TABLE.lock().unwrap_or_else(|e| e.into_inner());
    f(t.get_or_insert_with(Table::default))
}

fn get32(b: &[u8; INFO], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn flags_of(b: &[u8; INFO]) -> u64 {
    u64::from_le_bytes(b[FLAGS..].try_into().unwrap())
}

/// The default session's record: no audit user, an IPv4 terminal of no
/// address, id 0, no flags.
fn default_info() -> [u8; INFO] {
    let mut b = [0u8; INFO];
    b[AUID..AUID + 4].copy_from_slice(&AU_DEFAUDITID.to_le_bytes());
    b[TERM_TYPE..TERM_TYPE + 4].copy_from_slice(&AU_IPV4.to_le_bytes());
    b
}

/// The session a credential whose id is `asid` is in: that one, or for a
/// credential that was never put in one (0) the machine's first, which is
/// made when it is first asked after.
pub fn session(asid: u32) -> u32 {
    if asid != 0 {
        return asid;
    }
    with(|t| {
        t.sessions.entry(ASSIGNED_ASID_MIN).or_insert_with(|| {
            let mut info = default_info();
            info[ASID..ASID + 4].copy_from_slice(&ASSIGNED_ASID_MIN.to_le_bytes());
            info[FLAGS..].copy_from_slice(&IS_INITIAL.to_le_bytes());
            Session { info, port: Weak::new() }
        });
        t.last = t.last.max(ASSIGNED_ASID_MIN);
    });
    ASSIGNED_ASID_MIN
}

/// Session `asid`'s record; the default session's for an id none has.
pub fn record(asid: u32) -> [u8; INFO] {
    with(|t| t.sessions.get(&asid).map(|s| s.info)).unwrap_or_else(default_info)
}

/// Session `asid`'s audit user.
pub fn auid(asid: u32) -> u32 {
    get32(&record(asid), AUID)
}

/// The session a saved credential was in, by the record saved with it:
/// known from now on if it was not (a process of a kernel that never heard
/// of it: a page's child). Its id.
pub fn adopt(info: &[u8; INFO]) -> u32 {
    let asid = get32(info, ASID);
    if asid != 0 {
        with(|t| {
            t.sessions.entry(asid).or_insert(Session { info: *info, port: Weak::new() });
            if asid >= ASSIGNED_ASID_MIN {
                t.last = t.last.max(asid);
            }
        });
    }
    asid
}

impl Table {
    /// The next id no session has.
    fn assign(&mut self) -> u32 {
        loop {
            self.last = if self.last < ASSIGNED_ASID_MIN || self.last >= ASSIGNED_ASID_MAX { ASSIGNED_ASID_MIN } else { self.last + 1 };
            if !self.sessions.contains_key(&self.last) {
                return self.last;
            }
        }
    }

    /// Whether a session that is `old` may be made `new` by this caller
    /// (XNU's `audit_session_update_check`).
    fn may_change(&self, old: &[u8; INFO], new: &[u8; INFO], superuser: bool) -> bool {
        if get32(old, AUID) != AU_DEFAUDITID && get32(old, AUID) != get32(new, AUID) {
            return false;
        }
        let unset = get32(old, TERM_TYPE) == AU_IPV4 && get32(old, TERMID) == 0 && get32(old, TERM_TYPE + 4) == 0;
        if !unset && old[TERMID..ASID] != new[TERMID..ASID] {
            return false;
        }
        let (was, now) = (flags_of(old), flags_of(new));
        let [su_set, su_clear, member_set, member_clear] = self.masks;
        let allowed = |n: u64, su: u64, member: u64| n == 0 || (superuser && n & !su == 0) || n & !member == 0;
        allowed(!was & now, su_set, member_set) && allowed(!now & was, su_clear, member_clear)
    }

    /// The port that names session `asid`, made if nobody holds one.
    fn port(&mut self, asid: u32) -> Option<Arc<Port>> {
        let s = self.sessions.get_mut(&asid)?;
        Some(s.port.upgrade().unwrap_or_else(|| {
            let p = Port::new(Kind::AuditSession(asid));
            s.port = Arc::downgrade(&p);
            p
        }))
    }
}

impl Kernel {
    /// The caller's audit token, as XNU makes a task's
    /// (`set_security_token`): the audit user, the effective user and
    /// group, the real ones, the process, its audit session, and which use
    /// of the process's number this is.
    pub(super) fn darwin_audit_token(&self) -> [u32; 8] {
        [self.creds.auid(), self.creds.uid(), self.creds.gid(), self.creds.ruid(), self.creds.rgid(), self.pid as u32, self.creds.asid(), 1]
    }

    /// The caller is put in the session `new` describes, which is made or
    /// changed to be so (XNU's `audit_session_setaia`). Its id.
    fn darwin_audit_set(&mut self, mut new: [u8; INFO]) -> Result<u32, Errno> {
        let superuser = self.creds.uid() == 0;
        let asid = with(|t| {
            let mut asid = get32(&new, ASID);
            if let Some(old) = t.sessions.get(&asid) {
                if !t.may_change(&old.info, &new, superuser) {
                    return Err(Errno::EINVAL);
                }
            }
            if asid == AU_ASSIGN_ASID {
                asid = t.assign();
                new[ASID..ASID + 4].copy_from_slice(&asid.to_le_bytes());
            }
            match t.sessions.get_mut(&asid) {
                // (One that is there is changed only when its audit user,
                // terminal or flags are: its masks with them.)
                Some(s) => {
                    if s.info[AUID..AUID + 4] != new[AUID..AUID + 4] || s.info[TERMID..ASID] != new[TERMID..ASID] || s.info[FLAGS..] != new[FLAGS..] {
                        s.info = new;
                    }
                }
                None => {
                    t.sessions.insert(asid, Session { info: new, port: Weak::new() });
                }
            }
            Ok(asid)
        })?;
        self.creds.set_asid(asid);
        self.procs.set_creds(self.pid, &self.creds.save_within(256));
        Ok(asid)
    }

    /// `getaudit_addr(auditinfo_addr*, length)`: the caller's session.
    pub(super) fn darwin_getaudit_addr(&mut self, m: &mut GuestMem<'_>, x: [u64; 8]) -> SysResult {
        if x[1] < INFO as u64 {
            return Err(Errno::EOVERFLOW);
        }
        m.write(x[0], &record(self.creds.asid()))?;
        Ok(0)
    }

    /// `setaudit_addr(auditinfo_addr*, length)`: the superuser's.
    pub(super) fn darwin_setaudit_addr(&mut self, m: &mut GuestMem<'_>, x: [u64; 8]) -> SysResult {
        let mut new = [0u8; INFO];
        let n = (x[1] as usize).min(INFO);
        new[..n].copy_from_slice(m.read(x[0], n)?);
        if !matches!(get32(&new, TERM_TYPE), AU_IPV4 | AU_IPV6) {
            return Err(Errno::EINVAL);
        }
        if self.creds.uid() != 0 {
            return Err(Errno::EPERM);
        }
        // (No session's id asks for a new one, as `AU_ASSIGN_ASID` does.)
        let assign = matches!(get32(&new, ASID), 0 | AU_ASSIGN_ASID);
        if assign {
            new[ASID..ASID + 4].copy_from_slice(&AU_ASSIGN_ASID.to_le_bytes());
        }
        let asid = self.darwin_audit_set(new)?;
        if assign && x[1] >= INFO as u64 {
            m.write(x[0], &record(asid))?;
        }
        Ok(0)
    }

    /// `setauid(au_id_t*)`: the session's audit user, the superuser's to
    /// set; in no session, a new one's.
    pub(super) fn darwin_setauid(&mut self, m: &mut GuestMem<'_>, x: [u64; 8]) -> SysResult {
        let id = m.read_u32(x[0])?;
        if self.creds.uid() != 0 {
            return Err(Errno::EPERM);
        }
        let mut new = record(self.creds.asid());
        if get32(&new, ASID) == 0 {
            new[ASID..ASID + 4].copy_from_slice(&AU_ASSIGN_ASID.to_le_bytes());
        }
        new[AUID..AUID + 4].copy_from_slice(&id.to_le_bytes());
        self.darwin_audit_set(new)?;
        Ok(0)
    }

    /// `auditon(command, data, length)`, the commands that are a session's.
    pub(super) fn darwin_auditon(&mut self, m: &mut GuestMem<'_>, x: [u64; 8]) -> SysResult {
        match x[0] {
            // What a session is, by the id the caller puts in the record.
            A_GETSINFO_ADDR => {
                if x[2] != INFO as u64 {
                    return Err(Errno::EINVAL);
                }
                let asid = m.read_u32(x[1] + ASID as u64)?;
                let mut info = with(|t| t.sessions.get(&asid).map(|s| s.info)).ok_or(Errno::EINVAL)?;
                if self.creds.uid() != 0 {
                    info[MASK..TERMID].fill(0xff);
                }
                m.write(x[1], &info)?;
                Ok(0)
            }
            A_GETSFLAGS | A_SETSFLAGS if x[2] != 8 => Err(Errno::EINVAL),
            A_GETSFLAGS => m.write_u64(x[1], flags_of(&record(self.creds.asid()))).map(|_| 0),
            A_SETSFLAGS => {
                let flags = m.read_u64(x[1])?;
                let mut new = record(self.creds.asid());
                new[FLAGS..].copy_from_slice(&flags.to_le_bytes());
                self.darwin_audit_set(new)?;
                Ok(0)
            }
            _ => Err(Errno::ENOSYS),
        }
    }

    /// A send right to session `asid`'s port, given the caller: its name.
    fn darwin_audit_port(&mut self, asid: u32) -> Result<u32, Errno> {
        let port = with(|t| t.port(asid)).ok_or(Errno::EINVAL)?;
        let space = self.darwin_space();
        let name = space.lock().unwrap_or_else(|e| e.into_inner()).give_send(port);
        Ok(name)
    }

    /// `audit_session_self()`: the caller's session's port.
    pub(super) fn darwin_audit_session_self(&mut self) -> SysResult {
        self.darwin_audit_port(self.creds.asid()).map(u64::from)
    }

    /// `audit_session_port(asid, name*)`: a session's port; the caller's
    /// own (by its id, or -1) or, for the superuser, any.
    pub(super) fn darwin_audit_session_port(&mut self, m: &mut GuestMem<'_>, x: [u64; 8]) -> SysResult {
        let (asid, mine) = (x[0] as u32, self.creds.asid());
        let asid = if asid == u32::MAX || asid == mine {
            mine
        } else if self.creds.uid() == 0 {
            asid
        } else {
            return Err(Errno::EPERM);
        };
        let name = self.darwin_audit_port(asid)?;
        m.write_u32(x[1], name)?;
        Ok(0)
    }

    /// `audit_session_join(port)`: the caller is in the session the port
    /// names from now on. Its id.
    pub(super) fn darwin_audit_session_join(&mut self, x: [u64; 8]) -> SysResult {
        let space = self.darwin_space();
        let port = space.lock().unwrap_or_else(|e| e.into_inner()).port_named(self.pid, x[0] as u32);
        let Some(Kind::AuditSession(asid)) = port.map(|p| p.kind) else { return Err(Errno::EINVAL) };
        if !with(|t| t.sessions.contains_key(&asid)) {
            return Err(Errno::EINVAL);
        }
        if asid != self.creds.asid() {
            self.creds.set_asid(asid);
            self.procs.set_creds(self.pid, &self.creds.save_within(256));
        }
        Ok(asid as u64)
    }

    /// One of the four flag masks ([`MASKS`]), by its place.
    pub(super) fn darwin_audit_mask(&self, which: usize) -> u64 {
        with(|t| t.masks[which])
    }

    /// The superuser sets one.
    pub(super) fn darwin_audit_set_mask(&mut self, which: usize, mask: u64) -> Result<(), Errno> {
        if self.creds.uid() != 0 {
            return Err(Errno::EPERM);
        }
        with(|t| t.masks[which] = mask);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(auid: u32, asid: u32, flags: u64) -> [u8; INFO] {
        let mut b = default_info();
        b[AUID..AUID + 4].copy_from_slice(&auid.to_le_bytes());
        b[ASID..ASID + 4].copy_from_slice(&asid.to_le_bytes());
        b[FLAGS..].copy_from_slice(&flags.to_le_bytes());
        b
    }

    #[test]
    fn a_session_changes_only_as_xnu_lets_it() {
        let mut t = Table::default();
        let old = info(AU_DEFAUDITID, 100_001, 0x30);
        // The audit user is set once.
        assert!(t.may_change(&old, &info(501, 100_001, 0x30), true));
        assert!(!t.may_change(&info(501, 100_001, 0x30), &info(502, 100_001, 0x30), true));
        // A flag is set or cleared only as the masks allow, by whom.
        assert!(!t.may_change(&old, &info(AU_DEFAUDITID, 100_001, 0x4030), true));
        t.masks = [0x4000, 0, 0, 0x4000];
        assert!(t.may_change(&old, &info(AU_DEFAUDITID, 100_001, 0x4030), true));
        assert!(!t.may_change(&old, &info(AU_DEFAUDITID, 100_001, 0x4030), false));
        assert!(t.may_change(&info(501, 100_001, 0x4030), &info(501, 100_001, 0x30), false));
        assert!(!t.may_change(&info(501, 100_001, 0x4030), &info(501, 100_001, 0x4010), true));
        // A terminal is set once.
        let mut term = old;
        term[TERMID..TERMID + 4].copy_from_slice(&7u32.to_le_bytes());
        assert!(t.may_change(&old, &term, false));
        assert!(!t.may_change(&term, &old, true));
    }

    #[test]
    fn ids_are_chosen_from_100000_and_skip_those_taken() {
        let mut t = Table::default();
        assert_eq!(t.assign(), ASSIGNED_ASID_MIN);
        t.sessions.insert(ASSIGNED_ASID_MIN + 1, Session { info: default_info(), port: Weak::new() });
        assert_eq!(t.assign(), ASSIGNED_ASID_MIN + 2);
        t.last = ASSIGNED_ASID_MAX;
        assert_eq!(t.assign(), ASSIGNED_ASID_MIN);
    }

    #[test]
    fn a_session_is_known_by_its_record_and_has_one_port() {
        let asid = adopt(&info(501, 4_000_000_000, 0x10));
        assert_eq!(asid, 4_000_000_000);
        assert_eq!(auid(asid), 501);
        assert_eq!(flags_of(&record(asid)), 0x10);
        // (One that is known is not replaced.)
        adopt(&info(502, 4_000_000_000, 0));
        assert_eq!(auid(asid), 501);
        let a = with(|t| t.port(asid)).unwrap();
        let b = with(|t| t.port(asid)).unwrap();
        assert!(Arc::ptr_eq(&a, &b) && a.kind == Kind::AuditSession(asid));
        // The default session: no audit user, and no port.
        assert_eq!(auid(0), AU_DEFAUDITID);
        assert_eq!(adopt(&default_info()), 0);
        assert!(with(|t| t.port(0)).is_none());
        // A credential never put in a session is in the machine's first.
        let first = session(0);
        assert_eq!((first, session(first), auid(first), flags_of(&record(first))), (ASSIGNED_ASID_MIN, ASSIGNED_ASID_MIN, AU_DEFAUDITID, IS_INITIAL));
        assert!(with(|t| t.port(first)).is_some() && with(|t| t.assign()) > first);
    }
}
