//! What Darwin says of a file beyond `stat`: the attribute lists of
//! `getattrlist`, and the "extended" calls that carry a file's security
//! (its access list) beside its mode.
//!
//! `getattrlist` is asked for attributes by bit, in five groups, and
//! answers them packed in bit order: a length, then each attribute at a
//! four-byte boundary, and after them all the data of the ones whose size
//! varies (a name, a path), which the fixed part refers to by offset
//! (`vfs_attrlist.c`). The kernel's files have what `stat` has, and the
//! rest is answered as APFS answers for a file with nothing special about
//! it: no Finder information, no access list, no resource fork.
//!
//! The kernel keeps no access lists, so a file never has one
//! (`fstat64_extended` says so), and giving it one is refused as a volume
//! that keeps none refuses it.
//!
//! Checked against a Mac by `test/fixtures/darwin/attr.c`.
use super::{abi, nr as d};
use crate::errno::Errno;
use crate::fs::Stat;
use crate::syscalls::nr;
use crate::vfs::FileType;
use crate::syscalls::{Kernel, SysResult};
use crate::GuestMem;

/// `ATTR_BIT_MAP_COUNT`: how many groups an `attrlist` has.
const GROUPS: u16 = 5;

/// The common attributes, by bit.
mod cmn {
    pub const NAME: u32 = 1 << 0;
    pub const DEVID: u32 = 1 << 1;
    pub const FSID: u32 = 1 << 2;
    pub const OBJTYPE: u32 = 1 << 3;
    pub const OBJTAG: u32 = 1 << 4;
    pub const OBJID: u32 = 1 << 5;
    pub const OBJPERMANENTID: u32 = 1 << 6;
    pub const PAROBJID: u32 = 1 << 7;
    pub const SCRIPT: u32 = 1 << 8;
    pub const CRTIME: u32 = 1 << 9;
    pub const MODTIME: u32 = 1 << 10;
    pub const CHGTIME: u32 = 1 << 11;
    pub const ACCTIME: u32 = 1 << 12;
    pub const BKUPTIME: u32 = 1 << 13;
    pub const FNDRINFO: u32 = 1 << 14;
    pub const OWNERID: u32 = 1 << 15;
    pub const GRPID: u32 = 1 << 16;
    pub const ACCESSMASK: u32 = 1 << 17;
    pub const FLAGS: u32 = 1 << 18;
    pub const GEN_COUNT: u32 = 1 << 19;
    pub const DOCUMENT_ID: u32 = 1 << 20;
    pub const USERACCESS: u32 = 1 << 21;
    pub const EXTENDED_SECURITY: u32 = 1 << 22;
    pub const UUID: u32 = 1 << 23;
    pub const GRPUUID: u32 = 1 << 24;
    pub const FILEID: u32 = 1 << 25;
    pub const PARENTID: u32 = 1 << 26;
    pub const FULLPATH: u32 = 1 << 27;
    pub const ADDEDTIME: u32 = 1 << 28;
    pub const ERROR: u32 = 1 << 29;
    pub const DATA_PROTECT_FLAGS: u32 = 1 << 30;
    pub const RETURNED_ATTRS: u32 = 1 << 31;
}

/// A volume's, which are asked for with `INFO` among them.
mod vol {
    pub const FSTYPE: u32 = 1 << 0;
    pub const SIGNATURE: u32 = 1 << 1;
    pub const SIZE: u32 = 1 << 2;
    pub const SPACEFREE: u32 = 1 << 3;
    pub const SPACEAVAIL: u32 = 1 << 4;
    pub const MINALLOCATION: u32 = 1 << 5;
    pub const ALLOCATIONCLUMP: u32 = 1 << 6;
    pub const IOBLOCKSIZE: u32 = 1 << 7;
    pub const OBJCOUNT: u32 = 1 << 8;
    pub const FILECOUNT: u32 = 1 << 9;
    pub const DIRCOUNT: u32 = 1 << 10;
    pub const MAXOBJCOUNT: u32 = 1 << 11;
    pub const MOUNTPOINT: u32 = 1 << 12;
    pub const NAME: u32 = 1 << 13;
    pub const MOUNTFLAGS: u32 = 1 << 14;
    pub const MOUNTEDDEVICE: u32 = 1 << 15;
    pub const ENCODINGSUSED: u32 = 1 << 16;
    pub const CAPABILITIES: u32 = 1 << 17;
    pub const UUID: u32 = 1 << 18;
    pub const FSTYPENAME: u32 = 1 << 20;
    pub const FSSUBTYPE: u32 = 1 << 21;
    pub const SPACEUSED: u32 = 1 << 23;
    pub const ATTRIBUTES: u32 = 1 << 30;
    pub const INFO: u32 = 1 << 31;
    pub const VALID: u32 = 0xc0b7_ffff;
}

/// A directory's.
mod dir {
    pub const LINKCOUNT: u32 = 1 << 0;
    pub const ENTRYCOUNT: u32 = 1 << 1;
    pub const MOUNTSTATUS: u32 = 1 << 2;
    pub const ALLOCSIZE: u32 = 1 << 3;
    pub const IOBLOCKSIZE: u32 = 1 << 4;
    pub const DATALENGTH: u32 = 1 << 5;
    pub const VALID: u32 = 0x3f;
}

/// A file's (whatever is no directory). The gaps are attributes of HFS's
/// that no call answers any more.
mod file {
    pub const LINKCOUNT: u32 = 1 << 0;
    pub const TOTALSIZE: u32 = 1 << 1;
    pub const ALLOCSIZE: u32 = 1 << 2;
    pub const IOBLOCKSIZE: u32 = 1 << 3;
    pub const CLUMPSIZE: u32 = 1 << 4;
    pub const DEVTYPE: u32 = 1 << 5;
    pub const DATALENGTH: u32 = 1 << 9;
    pub const DATAALLOCSIZE: u32 = 1 << 10;
    pub const RSRCLENGTH: u32 = 1 << 12;
    pub const RSRCALLOCSIZE: u32 = 1 << 13;
    pub const VALID: u32 = 0x363f;
}

/// The fifth group, read as more common attributes when the options say so
/// (`FSOPT_ATTR_CMN_EXTENDED`); a fork has none otherwise.
mod ext {
    pub const RELPATH: u32 = 1 << 2;
    pub const PRIVATESIZE: u32 = 1 << 3;
    pub const LINKID: u32 = 1 << 4;
    pub const NOFIRMLINKPATH: u32 = 1 << 5;
    pub const REALDEVID: u32 = 1 << 6;
    pub const REALFSID: u32 = 1 << 7;
    pub const CLONEID: u32 = 1 << 8;
    pub const EXT_FLAGS: u32 = 1 << 9;
    pub const RECURSIVE_GENCOUNT: u32 = 1 << 10;
    pub const VALID: u32 = 0x7fc;
}

/// The options: the last link of the path is the file, an attribute there
/// is none of is packed all the same, and the fifth group is `ext`.
const FSOPT_NOFOLLOW: u64 = 1;
const FSOPT_PACK_INVAL_ATTRS: u64 = 8;
const FSOPT_ATTR_CMN_EXTENDED: u64 = 0x20;

/// `VT_APFS`, the kind of filesystem a file's node is of.
const VT_APFS: u32 = 25;
/// The volume's name, which is its root's.
const VOLUME: &str = "Macintosh HD";
/// `kTextEncodingMacUnicode`, the script of every name on APFS.
const SCRIPT: u32 = 0x7e;

/// An answer being packed: the fixed part, the data that follows it, and
/// where in the first each reference to the second is.
#[derive(Default)]
struct Pack {
    fixed: Vec<u8>,
    var: Vec<u8>,
    refs: Vec<(usize, usize)>,
}

impl Pack {
    fn bytes(&mut self, b: &[u8]) {
        self.fixed.extend_from_slice(b);
        self.fixed.resize(self.fixed.len().next_multiple_of(4), 0);
    }
    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    /// A `timespec`.
    fn time(&mut self, s: i64, ns: u32) {
        self.u64(s as u64);
        self.u64(ns as u64);
    }
    /// Data of a size of its own: an `attrreference` here (how far on it
    /// is, and how long), the data after everything fixed.
    fn var(&mut self, data: &[u8]) {
        self.refs.push((self.fixed.len(), self.var.len()));
        self.u32(0);
        self.u32(data.len() as u32);
        self.var.extend_from_slice(data);
        self.var.resize(self.var.len().next_multiple_of(4), 0);
    }
    /// A string, with the zero that ends it.
    fn string(&mut self, s: &str) {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        self.var(&b);
    }
    /// The whole answer, its references pointed at their data.
    fn finish(mut self) -> Vec<u8> {
        let fixed = self.fixed.len();
        for (at, data) in self.refs {
            let off = (fixed + data - at) as u32;
            self.fixed[at..at + 4].copy_from_slice(&off.to_le_bytes());
        }
        self.fixed.extend_from_slice(&self.var);
        self.fixed
    }
}

/// `VREG`, `VDIR` and the rest: a node's kind.
fn objtype(t: FileType) -> u32 {
    match t {
        FileType::Regular => 1,
        FileType::Directory => 2,
        FileType::CharDevice => 4,
        FileType::Symlink => 5,
        FileType::Socket => 6,
        FileType::Fifo => 7,
    }
}

/// The directory a path is in, and its name there.
fn split(path: &str) -> (&str, &str) {
    let p = if path.len() > 1 { path.trim_end_matches('/') } else { path };
    match p.rfind('/') {
        Some(0) if p.len() == 1 => ("/", "/"),
        Some(0) => ("/", &p[1..]),
        Some(i) => (&p[..i], &p[i + 1..]),
        None => ("/", p),
    }
}

impl Kernel {
    /// `getattrlist`, `fgetattrlist` and `getattrlistat`: the file is
    /// `path` under `dirfd`, or with no path the descriptor itself.
    pub(super) fn darwin_getattrlist(&mut self, m: &mut GuestMem<'_>, dirfd: i32, path: u64, list: u64, buf: u64, size: u64, options: u64) -> SysResult {
        let head = m.read(list, 24)?.to_vec();
        let word = |at: usize| u32::from_le_bytes(head[at..at + 4].try_into().unwrap());
        if u16::from_le_bytes([head[0], head[1]]) != GROUPS {
            return Err(Errno::EINVAL);
        }
        let (common, volume, dirs, files, fork) = (word(4), word(8), word(12), word(16), word(20));
        let extended = options & FSOPT_ATTR_CMN_EXTENDED != 0;
        if dirs & !dir::VALID != 0 || files & !file::VALID != 0 || fork & !(if extended { ext::VALID } else { 0 }) != 0 {
            return Err(Errno::EINVAL);
        }
        // A volume is asked about by itself.
        if volume != 0 && (volume & !vol::VALID != 0 || dirs | files | fork != 0) {
            return Err(Errno::EINVAL);
        }
        let (md, full) = if path == 0 {
            let f = self.fds.get(dirfd)?;
            (f.handle.metadata()?, f.path.clone())
        } else {
            let p = self.path_io().at_path(m, dirfd, path)?;
            if options & FSOPT_NOFOLLOW != 0 {
                (self.vfs.metadata_nofollow(&p)?, p)
            } else {
                (self.vfs.metadata(&p)?, self.vfs.canonical(&p))
            }
        };
        // (A volume's own attributes are its root's.)
        let (md, full) = if volume != 0 { (self.vfs.metadata("/")?, "/".to_string()) } else { (md, full) };
        let full = if full.starts_with('/') { full } else { "/".to_string() };
        let out = self.darwin_pack_attrs(&md, &full, [common, volume, dirs, files, fork], options, false);
        m.write(buf, &out[..out.len().min(size as usize)])?;
        Ok(0)
    }

    /// One file's attributes, packed: its length (of all of it, however
    /// much there turns out to be room for), then what was asked for. One
    /// of a directory's entries read in bulk (`bulk`) is a multiple of
    /// eight bytes long.
    fn darwin_pack_attrs(&self, md: &crate::vfs::Metadata, full: &str, want: [u32; 5], options: u64, bulk: bool) -> Vec<u8> {
        let [common, volume, dirs, files, fork] = want;
        let st = self.path_io().stat_owned(md);
        let (parent, name) = split(full);
        let parent_ino = self.vfs.metadata(parent).map_or(2, |p| p.ino);
        let is_dir = md.file_type == FileType::Directory;
        let fsid = |p: &mut Pack| {
            p.u32(abi::ROOT_DEV);
            p.u32(0x1a);
        };

        let mut p = Pack::default();
        // The length, written last; then which attributes are answered,
        // likewise, for whoever asks to be told.
        p.u32(0);
        let returned_at = p.fixed.len();
        // (An error is a directory entry's, read in bulk: here it is none,
        // first of all, unless the caller is told what it got instead.)
        let mut common = common;
        let mut unsaid = false;
        if common & cmn::RETURNED_ATTRS != 0 {
            p.bytes(&[0; 20]);
            if common & cmn::ERROR != 0 && options & FSOPT_PACK_INVAL_ATTRS == 0 {
                common &= !cmn::ERROR;
                unsaid = true;
            }
        }
        if common & cmn::ERROR != 0 {
            p.u32(0);
        }
        for bit in (0..31).map(|i| 1u32 << i).filter(|b| common & b != 0 && *b != cmn::ERROR) {
            match bit {
                cmn::NAME => p.string(if full == "/" { VOLUME } else { name }),
                cmn::DEVID => p.u32(abi::ROOT_DEV),
                cmn::FSID => fsid(&mut p),
                cmn::OBJTYPE => p.u32(objtype(md.file_type)),
                cmn::OBJTAG => p.u32(VT_APFS),
                // An object's id is its number and a generation.
                cmn::OBJID | cmn::OBJPERMANENTID => p.u64(st.ino & 0xffff_ffff),
                cmn::PAROBJID => p.u64(parent_ino & 0xffff_ffff),
                cmn::SCRIPT => p.u32(SCRIPT),
                // (No time of birth is kept: a file was made when it was
                // last changed, as `stat` says too.)
                cmn::CRTIME | cmn::MODTIME | cmn::ADDEDTIME => p.time(st.mtime, st.mtime_ns),
                cmn::CHGTIME => p.time(st.ctime, st.ctime_ns),
                cmn::ACCTIME => p.time(st.atime, st.atime_ns),
                cmn::BKUPTIME => p.time(0, 0),
                cmn::FNDRINFO => p.bytes(&[0; 32]),
                cmn::OWNERID => p.u32(st.uid),
                cmn::GRPID => p.u32(st.gid),
                // (The whole mode, its kind with its permissions.)
                cmn::ACCESSMASK => p.u32(st.mode),
                cmn::FLAGS | cmn::GEN_COUNT | cmn::DOCUMENT_ID | cmn::DATA_PROTECT_FLAGS => p.u32(0),
                cmn::USERACCESS => p.u32(self.darwin_user_access(&st)),
                // No access list: a reference to nothing.
                cmn::EXTENDED_SECURITY => p.var(&[]),
                cmn::UUID | cmn::GRPUUID => p.bytes(&[0; 16]),
                cmn::FILEID => p.u64(st.ino),
                cmn::PARENTID => p.u64(parent_ino),
                cmn::FULLPATH => p.string(full),
                _ => unreachable!("every bit below the last is an attribute"),
            }
        }
        if volume != 0 {
            self.darwin_volume_attrs(&mut p, volume);
        }
        let (mut dirs, mut files) = (dirs, files);
        if is_dir {
            files = 0;
            for bit in (0..6).map(|i| 1u32 << i).filter(|b| dirs & b != 0) {
                match bit {
                    dir::LINKCOUNT => p.u32(st.nlink),
                    dir::ENTRYCOUNT => p.u32(self.darwin_entry_count(full)),
                    dir::MOUNTSTATUS => p.u32((full == "/") as u32),
                    dir::ALLOCSIZE => p.u64(st.blocks as u64 * 512),
                    dir::IOBLOCKSIZE => p.u32(st.blksize as u32),
                    _ => p.u64(st.size as u64),
                }
            }
        } else {
            dirs = 0;
            for bit in (0..14).map(|i| 1u32 << i).filter(|b| files & b != 0) {
                match bit {
                    file::LINKCOUNT => p.u32(st.nlink),
                    file::TOTALSIZE | file::DATALENGTH => p.u64(st.size as u64),
                    file::ALLOCSIZE | file::DATAALLOCSIZE => p.u64(st.blocks as u64 * 512),
                    file::IOBLOCKSIZE => p.u32(st.blksize as u32),
                    file::CLUMPSIZE => p.u32(0),
                    file::DEVTYPE => p.u32(abi::dev(st.rdev)),
                    // No resource fork.
                    _ => p.u64(0),
                }
            }
        }
        for bit in (2..11).map(|i| 1u32 << i).filter(|b| fork & b != 0) {
            match bit {
                ext::RELPATH => p.string(full.trim_start_matches('/')),
                ext::PRIVATESIZE => p.u64(st.size as u64),
                ext::LINKID => p.u64(st.ino),
                ext::NOFIRMLINKPATH => p.string(full),
                ext::REALDEVID => p.u32(abi::ROOT_DEV),
                ext::REALFSID => fsid(&mut p),
                // Cloned from nothing; no flags; never changed beneath.
                _ => p.u64(0),
            }
        }
        // (Room was made for the error that went unsaid, and is left.)
        if unsaid {
            p.u32(0);
        }
        if common & cmn::RETURNED_ATTRS != 0 {
            for (i, v) in [common, volume, dirs, files, fork].into_iter().enumerate() {
                p.fixed[returned_at + 4 * i..returned_at + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        let mut out = p.finish();
        if bulk {
            out.resize(out.len().next_multiple_of(8), 0);
        }
        let whole = out.len() as u32;
        out[0..4].copy_from_slice(&whole.to_le_bytes());
        out
    }

    /// `getattrlistbulk`: the attributes of a directory's entries, as many
    /// as fit, from where the descriptor has read to. The answer is how
    /// many; none at the end.
    pub(super) fn darwin_getattrlistbulk(&mut self, m: &mut GuestMem<'_>, fd: i32, list: u64, buf: u64, size: u64, options: u64) -> SysResult {
        let head = m.read(list, 24)?.to_vec();
        let word = |at: usize| u32::from_le_bytes(head[at..at + 4].try_into().unwrap());
        let (common, volume, dirs, files, fork) = (word(4), word(8), word(12), word(16), word(20));
        let extended = options & FSOPT_ATTR_CMN_EXTENDED != 0;
        // Each entry says what it has, since each may have something else;
        // and a directory's entries are no volumes.
        if u16::from_le_bytes([head[0], head[1]]) != GROUPS
            || common & cmn::RETURNED_ATTRS == 0
            || volume != 0
            || dirs & !dir::VALID != 0
            || files & !file::VALID != 0
            || fork & !(if extended { ext::VALID } else { 0 }) != 0
        {
            return Err(Errno::EINVAL);
        }
        let file = self.fds.get(fd)?;
        if file.handle.metadata()?.file_type != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }
        let dir = file.path.trim_end_matches('/').to_string();
        let (mut out, mut count, mut full_up) = (Vec::new(), 0u64, false);
        file.take_dir_entries(&|dir| self.vfs.mounts_in(dir), |e| {
            if e.name == "." || e.name == ".." {
                return true;
            }
            let path = format!("{dir}/{}", e.name);
            // (An entry gone since the directory was read is passed over.)
            let Ok(md) = self.vfs.metadata_nofollow(&path) else { return true };
            let entry = self.darwin_pack_attrs(&md, &path, [common, 0, dirs, files, fork], options, true);
            if out.len() + entry.len() > size as usize {
                full_up = true;
                return false;
            }
            out.extend_from_slice(&entry);
            count += 1;
            true
        })?;
        // No room for even one.
        if count == 0 && full_up {
            return Err(Errno::ERANGE);
        }
        m.write(buf, &out)?;
        Ok(count)
    }

    /// How many entries a directory has, itself and its parent aside.
    fn darwin_entry_count(&self, path: &str) -> u32 {
        let Ok(handle) = self.vfs.open(path, 0, 0) else { return 0 };
        handle.read_dir().map_or(0, |e| e.iter().filter(|e| e.name != "." && e.name != "..").count() as u32)
    }

    /// What the caller may do with a file, as `access` would say of each:
    /// read 4, write 2, run or search 1.
    fn darwin_user_access(&self, st: &Stat) -> u32 {
        // The superuser may read and write anything, and run what anyone
        // may; anybody else is the file's owner here (`stat_owned`).
        if self.creds.uid() == 0 {
            return 6 | (st.mode & 0o111 != 0 || st.mode & 0o170000 == 0o040000) as u32;
        }
        (st.mode >> 6) & 7
    }

    /// The attributes of the one volume there is: APFS, mounted on `/`,
    /// of the size `statfs` says.
    fn darwin_volume_attrs(&self, p: &mut Pack, volume: u32) {
        // (`statfs`'s numbers: 4096-byte blocks, 2^20 of them, half free.)
        const BLOCK: u64 = 4096;
        const SIZE: u64 = BLOCK << 20;
        const FREE: u64 = BLOCK << 19;
        // What the volume can do (VOL_CAP_FMT_*, VOL_CAP_INT_*): its
        // format keeps persistent ids, symbolic and hard links, sparse
        // files, case as written and told apart, files past 2 GiB, hidden
        // files and 64-bit ids, and answers `statfs` cheaply; of the
        // interfaces it has `getattrlist`, advisory locks, `access` by
        // attribute and extended attributes.
        const FORMAT: u32 = 0x1 | 0x2 | 0x4 | 0x40 | 0x100 | 0x200 | 0x400 | 0x800 | 0x1000 | 0x2_0000;
        const INTERFACES: u32 = 0x2 | 0x100 | 0x400 | 0x4000;
        for bit in (0..31).map(|i| 1u32 << i).filter(|b| volume & b != 0) {
            match bit {
                vol::FSTYPE | vol::FSSUBTYPE => p.u32(if bit == vol::FSTYPE { 0x1a } else { 0 }),
                // APFS's two letters, as HFS had 'H+'.
                vol::SIGNATURE => p.u32(0x4253),
                vol::SIZE => p.u64(SIZE),
                vol::SPACEFREE | vol::SPACEAVAIL => p.u64(FREE),
                vol::SPACEUSED => p.u64(SIZE - FREE),
                vol::MINALLOCATION | vol::ALLOCATIONCLUMP => p.u64(BLOCK),
                vol::IOBLOCKSIZE => p.u32(1 << 20),
                vol::OBJCOUNT | vol::FILECOUNT | vol::DIRCOUNT => p.u32(0),
                vol::MAXOBJCOUNT => p.u32(u32::MAX),
                vol::MOUNTPOINT => p.string("/"),
                vol::NAME => p.string(VOLUME),
                // MNT_LOCAL | MNT_ROOTFS | MNT_JOURNALED, as `statfs` says.
                vol::MOUNTFLAGS => p.u32(0x1000 | 0x4000 | 0x80_0000),
                vol::MOUNTEDDEVICE => p.string("/dev/disk1s1"),
                vol::ENCODINGSUSED => p.u64(1),
                vol::CAPABILITIES => {
                    // What it can do, then which of those answers mean
                    // anything: all that are given.
                    for v in [FORMAT, INTERFACES, 0, 0, FORMAT, INTERFACES, 0, 0] {
                        p.u32(v);
                    }
                }
                vol::UUID => p.bytes(&[0x67, 0x65, 0x78, 0x2d, 0x77, 0x61, 0x72, 0x70, 0x80, 0, 0, 0, 0, 0, 0, 1]),
                vol::FSTYPENAME => p.string("apfs"),
                vol::ATTRIBUTES => {
                    // The attributes this call answers, twice over: the
                    // valid ones, and the ones the volume keeps itself.
                    for _ in 0..2 {
                        for v in [!cmn::ERROR, vol::VALID, dir::VALID, file::VALID, 0] {
                            p.u32(v);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// `setattrlist`, `fsetattrlist` and `setattrlistat`: the attributes a
    /// caller may give a file, read from `buf` in the order of their bits.
    /// The file is `path` under `dirfd`, or with no path the descriptor.
    /// What is kept is what `stat` has, the times and the mode and the
    /// owner; the rest (Finder information, a backup time, flags) is taken
    /// and not kept, as `chflags` is.
    pub(super) fn darwin_setattrlist(&mut self, m: &mut GuestMem<'_>, dirfd: i32, path: u64, list: u64, buf: u64, size: u64, options: u64) -> SysResult {
        // `ATTR_CMN_SETMASK`, and the one file attribute that may be set
        // (a device's numbers); a volume may be renamed, which is not
        // something this one's owner may do.
        const SETTABLE: u32 = 0x51c7_ff00;
        let head = m.read(list, 24)?.to_vec();
        let word = |at: usize| u32::from_le_bytes(head[at..at + 4].try_into().unwrap());
        if u16::from_le_bytes([head[0], head[1]]) != GROUPS {
            return Err(Errno::EINVAL);
        }
        let (common, volume, dirs, files, fork) = (word(4), word(8), word(12), word(16), word(20));
        if common & !SETTABLE != 0 || dirs != 0 || files & !file::DEVTYPE != 0 || fork != 0 {
            return Err(Errno::EINVAL);
        }
        if volume != 0 {
            return Err(Errno::EPERM);
        }
        // The file must be there, whatever is asked of it.
        let full = if path == 0 {
            self.fds.get(dirfd)?.path.clone()
        } else {
            let p = self.path_io().at_path(m, dirfd, path)?;
            if options & FSOPT_NOFOLLOW != 0 { self.vfs.metadata_nofollow(&p)? } else { self.vfs.metadata(&p)? };
            p
        };
        let data = m.read(buf, size.min(4096) as usize)?.to_vec();
        let mut at = 0usize;
        let mut take = |n: usize| -> Result<&[u8], Errno> {
            let b = data.get(at..at + n).ok_or(Errno::EINVAL)?;
            at += n.next_multiple_of(4);
            Ok(b)
        };
        let (mut atime, mut mtime, mut uid, mut gid, mut mode) = (None, None, None, None, None);
        for bit in (0..31).map(|i| 1u32 << i).filter(|b| common & b != 0) {
            match bit {
                cmn::CRTIME | cmn::MODTIME | cmn::CHGTIME | cmn::ACCTIME | cmn::BKUPTIME | cmn::ADDEDTIME => {
                    let b = take(16)?;
                    let t = (i64::from_le_bytes(b[0..8].try_into().unwrap()), u64::from_le_bytes(b[8..16].try_into().unwrap()) as u32);
                    match bit {
                        cmn::MODTIME => mtime = Some(t),
                        cmn::ACCTIME => atime = Some(t),
                        _ => {}
                    }
                }
                cmn::FNDRINFO => {
                    take(32)?;
                }
                cmn::UUID | cmn::GRPUUID => {
                    take(16)?;
                }
                // A list to be given, by reference: none is kept.
                cmn::EXTENDED_SECURITY => {
                    take(8)?;
                }
                _ => {
                    let v = u32::from_le_bytes(take(4)?.try_into().unwrap());
                    match bit {
                        cmn::OWNERID => uid = Some(v),
                        cmn::GRPID => gid = Some(v),
                        cmn::ACCESSMASK => mode = Some(v),
                        _ => {}
                    }
                }
            }
        }
        if atime.is_some() || mtime.is_some() {
            if path == 0 {
                let file = self.fds.get(dirfd)?;
                self.path_io().set_times_of(&*file, atime, mtime)?;
            } else {
                self.vfs.set_times(&full, atime, mtime, options & FSOPT_NOFOLLOW == 0)?;
            }
        }
        const NOBODY: u64 = u32::MAX as u64;
        let (dirfd, nofollow) = (dirfd as i64 as u64, if options & FSOPT_NOFOLLOW != 0 { 0x100 } else { 0 });
        if uid.is_some() || gid.is_some() {
            let (u, g) = (uid.map_or(NOBODY, u64::from), gid.map_or(NOBODY, u64::from));
            if path == 0 { self.dispatch(m, nr::FCHOWN, [dirfd, u, g, 0, 0, 0]) } else { self.dispatch(m, nr::FCHOWNAT, [dirfd, path, u, g, nofollow, 0]) }.unwrap_or(Err(Errno::EINVAL))?;
        }
        if let Some(mode) = mode {
            let mode = mode as u64 & 0o7777;
            if path == 0 { self.dispatch(m, nr::FCHMOD, [dirfd, mode, 0, 0, 0, 0]) } else { self.dispatch(m, nr::FCHMODAT, [dirfd, path, mode, 0, 0, 0]) }.unwrap_or(Err(Errno::EINVAL))?;
        }
        Ok(0)
    }

    /// `stat64_extended` and its kin: `stat`, and the file's security
    /// beside it, which is a record that says there is no list.
    pub(super) fn darwin_stat_extended(&mut self, m: &mut GuestMem<'_>, code: u64, x: [u64; 8]) -> SysResult {
        const CWD: u64 = -100i64 as u64;
        let r = match code {
            d::FSTAT64_EXTENDED => self.dispatch(m, nr::FSTAT, [x[0], x[1], 0, 0, 0, 0]),
            d::LSTAT64_EXTENDED => self.dispatch(m, nr::NEWFSTATAT, [CWD, x[0], x[1], 0x100, 0, 0]),
            _ => self.dispatch(m, nr::NEWFSTATAT, [CWD, x[0], x[1], 0, 0, 0]),
        };
        r.unwrap_or(Err(Errno::EINVAL))?;
        if x[2] != 0 && x[3] != 0 {
            // A `kauth_filesec` with no list in it: its magic number, an
            // owner and a group known by no UUID, and -1 entries
            // (`KAUTH_FILESEC_NOACL`).
            let mut sec = [0u8; 44];
            sec[0..4].copy_from_slice(&0x012c_c16du32.to_le_bytes());
            sec[36..40].copy_from_slice(&u32::MAX.to_le_bytes());
            if m.read_u64(x[3])? >= sec.len() as u64 {
                m.write(x[2], &sec)?;
            }
            m.write_u64(x[3], sec.len() as u64)?;
        }
        Ok(0)
    }

    /// `chmod_extended` and `fchmod_extended`: an owner, a group, a mode
    /// and an access list at once, each or none. `path` is the file, or
    /// with none of it `fd` is.
    pub(super) fn darwin_chmod_extended(&mut self, m: &mut GuestMem<'_>, fd: Option<i32>, path: u64, uid: u32, gid: u32, mode: u32, security: u64) -> SysResult {
        const CWD: u64 = -100i64 as u64;
        // `KAUTH_UID_NONE`: not this one.
        const NONE: u32 = !0 - 100;
        // `KAUTH_FILESEC_NONE` is no list to set, and 1 a list to remove
        // (`_FILESEC_REMOVE_ACL`), which is done as soon as said; any
        // other is a list, which this volume keeps none of.
        if security > 1 {
            return Err(Errno::EOPNOTSUPP);
        }
        let own = |v: u32| if v == NONE { u32::MAX as u64 } else { v as u64 };
        if uid != NONE || gid != NONE {
            match fd {
                Some(fd) => self.dispatch(m, nr::FCHOWN, [fd as u64, own(uid), own(gid), 0, 0, 0]),
                None => self.dispatch(m, nr::FCHOWNAT, [CWD, path, own(uid), own(gid), 0, 0]),
            }
            .unwrap_or(Err(Errno::EINVAL))?;
        }
        if mode != u32::MAX {
            match fd {
                Some(fd) => self.dispatch(m, nr::FCHMOD, [fd as u64, mode as u64 & 0o7777, 0, 0, 0, 0]),
                None => self.dispatch(m, nr::FCHMODAT, [CWD, path, mode as u64 & 0o7777, 0, 0, 0]),
            }
            .unwrap_or(Err(Errno::EINVAL))?;
        }
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_packed_in_fours_with_its_data_after() {
        let mut p = Pack::default();
        p.u32(0);
        p.string("abc");
        p.bytes(&[1, 2, 3, 4, 5]);
        p.string("/a/long/path");
        p.u64(7);
        let out = p.finish();
        // Length; a reference; five bytes padded to eight; a reference;
        // eight: 36 fixed. Then "abc\0" and thirteen bytes padded to 16.
        assert_eq!(out.len(), 36 + 4 + 16);
        let r = |at: usize| (u32::from_le_bytes(out[at..at + 4].try_into().unwrap()) as usize, u32::from_le_bytes(out[at + 4..at + 8].try_into().unwrap()) as usize);
        let (off, len) = r(4);
        assert_eq!((&out[4 + off..4 + off + len], len), (&b"abc\0"[..], 4));
        let (off, len) = r(20);
        assert_eq!(&out[20 + off..20 + off + len], b"/a/long/path\0");
        assert_eq!(out[28..36], 7u64.to_le_bytes());
    }

    #[test]
    fn a_path_is_a_directory_and_a_name() {
        assert_eq!(split("/"), ("/", "/"));
        assert_eq!(split("/a"), ("/", "a"));
        assert_eq!(split("/a/b"), ("/a", "b"));
        assert_eq!(split("/a/b/"), ("/a", "b"));
    }
}
