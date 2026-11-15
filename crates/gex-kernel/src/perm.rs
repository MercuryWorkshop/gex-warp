//! Who may use a file: the owner, group and other bits of its mode, a
//! process's supplementary groups, and the capabilities that override them.
//!
//! This is checked only where gex knows a file's owner
//! ([`Metadata::owner`]: tmpfs, and a host directory with its owners kept,
//! [`crate::vfs::hostfs::OWNER_XATTR`]) and only for a process that is no
//! longer the machine's first user ([`Creds::is_base`]). The first user's own
//! permissions are what the host checks already, as they always were, and
//! nothing here is added to them; a filesystem that keeps no owners (the
//! devices, `/proc`, the page's store) is not checked at all.
//!
//! Not checked: the search permission of the directories a path passes
//! through, ACLs, immutable flags, and who may set a file's times.

use crate::errno::Errno;
use crate::syscalls::{cap, Creds, PathIo};
use crate::vfs::{FileType, Metadata};

/// `R_OK`, `W_OK` and `X_OK`.
pub const R: u32 = 4;
pub const W: u32 = 2;
pub const X: u32 = 1;

pub const S_ISUID: u32 = 0o4000;
pub const S_ISGID: u32 = 0o2000;
pub const S_ISVTX: u32 = 0o1000;

impl Creds {
    /// Whether the process may use the file `md` as `want` (a mix of [`R`],
    /// [`W`] and [`X`]) says: by its effective ids, or its real ones (`real`:
    /// what `access` asks). A file whose owner is not known may be used.
    pub fn may(&self, md: &Metadata, want: u32, real: bool) -> bool {
        if md.owner.is_none() {
            return true;
        }
        let (owner, group) = self.owner_of(md.owner);
        let is_dir = md.file_type == FileType::Directory;
        let (uid, gid) = if real { (self.ruid(), self.rgid()) } else { (self.uid(), self.gid()) };
        // (By the real ids, as `access` asks, only a real root overrides:
        // a set-user-ID program's capabilities are not its caller's.)
        let overrides = if real { uid == 0 } else { self.capable(cap::DAC_OVERRIDE) };
        if overrides {
            // Nobody's execute bit is no one's to override, but a directory
            // is always searched.
            return want & X == 0 || is_dir || md.mode & 0o111 != 0;
        }
        if !real && self.capable(cap::DAC_READ_SEARCH) && want & W == 0 && (want & X == 0 || is_dir) {
            return true;
        }
        let class = if uid == owner {
            md.mode >> 6
        } else if gid == group || self.groups().contains(&group) {
            md.mode >> 3
        } else {
            md.mode
        } & 7;
        class & want == want
    }

    /// Whether the process owns the file, or may act as its owner
    /// (`CAP_FOWNER`): what `chmod` and the sticky bit ask.
    pub fn owns(&self, md: &Metadata) -> bool {
        md.owner.is_none() || self.uid() == self.owner_of(md.owner).0 || self.capable(cap::FOWNER)
    }
}

impl PathIo<'_> {
    /// Whether the process's permissions are ours to check: it is not the
    /// machine's first user, whose own the host checks.
    pub fn enforcing(&self) -> bool {
        !self.creds.is_base()
    }

    /// Whether the process may use what `p` names (links followed unless
    /// `!follow`) as `want` says. A name that is not there is left for the
    /// call itself to say.
    pub fn check_path(&self, p: &str, want: u32, follow: bool) -> Result<(), Errno> {
        if !self.enforcing() {
            return Ok(());
        }
        let md = if follow { self.ns.metadata(p) } else { self.ns.metadata_nofollow(p) };
        match md {
            Ok(md) if !self.creds.may(&md, want, false) => Err(Errno::EACCES),
            _ => Ok(()),
        }
    }

    /// Whether the process may make or remove a name in the directory `p`
    /// is in: write and search permission there.
    pub fn check_parent(&self, p: &str) -> Result<(), Errno> {
        if !self.enforcing() {
            return Ok(());
        }
        self.check_path(&crate::vfs::parent_of(p), W | X, true)
    }

    /// [`PathIo::check_parent`] for taking away the name `p`, which a
    /// directory with the sticky bit lets only the file's owner or the
    /// directory's do.
    pub fn check_remove(&self, p: &str) -> Result<(), Errno> {
        if !self.enforcing() {
            return Ok(());
        }
        // (A name that is not there is `ENOENT` whoever asks: the call says.)
        if self.ns.metadata_nofollow(p).is_err() {
            return Ok(());
        }
        self.check_parent(p)?;
        let Ok(dir) = self.ns.metadata(&crate::vfs::parent_of(p)) else { return Ok(()) };
        if dir.owner.is_some() && dir.mode & S_ISVTX != 0 {
            if let Ok(file) = self.ns.metadata_nofollow(p) {
                let me = self.creds.uid();
                let theirs = |md: &Metadata| self.creds.owner_of(md.owner).0 == me;
                if file.owner.is_some() && !theirs(&file) && !theirs(&dir) && !self.creds.capable(cap::FOWNER) {
                    return Err(Errno::EPERM);
                }
            }
        }
        Ok(())
    }

    /// The owner and group a name just made at `p` takes: the process's user,
    /// and a group by the system's rule: the directory's on a Mac, the
    /// process's own on Linux unless the directory has the set-group-ID bit.
    pub fn new_owner(&self, p: &str) -> (u32, u32) {
        let dir = self.ns.metadata(&crate::vfs::parent_of(p)).ok();
        let dir_gid = dir.as_ref().map(|d| self.creds.owner_of(d.owner).1);
        let inherits = crate::darwin::on() || dir.as_ref().is_some_and(|d| d.mode & S_ISGID != 0);
        (self.creds.uid(), if inherits { dir_gid.unwrap_or(self.creds.gid()) } else { self.creds.gid() })
    }

    /// The name `p` was just made: it is the process's, where owners are
    /// kept and the process is not the first user (whose files are the
    /// first user's already).
    pub fn record_new(&self, p: &str) {
        if !self.enforcing() || !self.ns.keeps_owners(p) {
            return;
        }
        let (uid, gid) = self.new_owner(p);
        let _ = self.ns.chown(p, uid, gid);
    }
}
