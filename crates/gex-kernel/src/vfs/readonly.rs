//! A filesystem that is another one, not to be written: the system layer
//! of a bottle ([`crate::darwin::bottle`]), which every bottle of it reads
//! and none changes, as a Mac's sealed system volume is under its data
//! volume.
//!
//! Everything that reads is the inner filesystem's. What would change it
//! is refused as a read-only mount refuses it: `EROFS`, after the errors
//! that come first on any filesystem (a name that is not there, one that
//! is already), so that `mkdir -p` of a directory the layer has still
//! finds it made.

use super::{FileHandle, FileSystem, Metadata, Walked, XattrOp};
use crate::errno::Errno;
use crate::fs::oflags;
use std::sync::Arc;

pub struct ReadOnly {
    inner: Arc<dyn FileSystem>,
}

impl ReadOnly {
    pub fn new(inner: Arc<dyn FileSystem>) -> ReadOnly {
        ReadOnly { inner }
    }

    /// `EROFS` for a name that is there, the inner filesystem's own error
    /// (`ENOENT`, `ENOTDIR`) for one that is not.
    fn there(&self, path: &str) -> Errno {
        match self.inner.metadata_nofollow(path) {
            Ok(_) => Errno::EROFS,
            Err(e) => e,
        }
    }

    /// `EEXIST` for a name that is there, `EROFS` for one to be made.
    fn made(&self, path: &str) -> Errno {
        match self.inner.metadata_nofollow(path) {
            Ok(_) => Errno::EEXIST,
            Err(_) => Errno::EROFS,
        }
    }

    fn refuse_open(&self, path: &str, flags: u64) -> Option<Errno> {
        let there = self.inner.metadata(path);
        if flags & oflags::O_CREAT != 0 && there.is_err() {
            return Some(Errno::EROFS);
        }
        if super::writes(flags) || flags & oflags::O_TRUNC != 0 {
            return Some(match there {
                // (A directory opened to be written is refused as one.)
                Ok(md) if md.file_type == super::FileType::Directory => Errno::EISDIR,
                Ok(_) => Errno::EROFS,
                Err(e) => e,
            });
        }
        None
    }
}

impl FileSystem for ReadOnly {
    fn fs_type(&self) -> &'static str {
        self.inner.fs_type()
    }
    fn links_are_names(&self) -> bool {
        self.inner.links_are_names()
    }
    fn open(&self, path: &str, flags: u64, mode: u32) -> Result<Arc<dyn FileHandle>, Errno> {
        match self.refuse_open(path, flags) {
            Some(e) => Err(e),
            None => self.inner.open(path, flags & !oflags::O_CREAT, mode),
        }
    }
    fn open_mode(&self, path: &str, flags: u64, mode: u32) -> Result<Arc<dyn FileHandle>, Errno> {
        match self.refuse_open(path, flags) {
            Some(e) => Err(e),
            None => self.inner.open_mode(path, flags & !oflags::O_CREAT, mode),
        }
    }
    fn metadata(&self, path: &str) -> Result<Metadata, Errno> {
        self.inner.metadata(path)
    }
    fn metadata_nofollow(&self, path: &str) -> Result<Metadata, Errno> {
        self.inner.metadata_nofollow(path)
    }
    fn access(&self, path: &str, mode: u32) -> Result<(), Errno> {
        if mode & 2 != 0 {
            return Err(self.there(path));
        }
        self.inner.access(path, mode)
    }
    fn read_link(&self, path: &str) -> Result<String, Errno> {
        self.inner.read_link(path)
    }
    fn walk(&self, abs: &str, follow_final: bool, mounts: &str) -> Result<Walked, Errno> {
        self.inner.walk(abs, follow_final, mounts)
    }
    fn walk_metadata(&self, abs: &str, mounts: &str) -> Result<Option<Metadata>, Errno> {
        self.inner.walk_metadata(abs, mounts)
    }
    fn unlink(&self, path: &str) -> Result<(), Errno> {
        Err(self.there(path))
    }
    fn mkdir(&self, path: &str, _mode: u32) -> Result<(), Errno> {
        Err(self.made(path))
    }
    fn rmdir(&self, path: &str) -> Result<(), Errno> {
        Err(self.there(path))
    }
    fn rename(&self, from: &str, _to: &str) -> Result<(), Errno> {
        Err(self.there(from))
    }
    fn rename2(&self, from: &str, _to: &str, _noreplace: bool) -> Result<(), Errno> {
        Err(self.there(from))
    }
    fn chmod(&self, path: &str, _mode: u32) -> Result<(), Errno> {
        Err(self.there(path))
    }
    fn chmod_link(&self, path: &str, _mode: u32) -> Result<(), Errno> {
        Err(self.there(path))
    }
    fn set_mtime(&self, path: &str, _secs: i64) -> Result<(), Errno> {
        Err(self.there(path))
    }
    fn set_times(&self, path: &str, _atime: Option<(i64, u32)>, _mtime: Option<(i64, u32)>) -> Result<(), Errno> {
        Err(self.there(path))
    }
    fn link(&self, from: &str, _to: &str) -> Result<(), Errno> {
        Err(self.there(from))
    }
    fn symlink(&self, _target: &str, path: &str) -> Result<(), Errno> {
        Err(self.made(path))
    }
    fn mksocket(&self, path: &str) -> Result<(), Errno> {
        Err(self.made(path))
    }
    fn mkfifo(&self, path: &str, _mode: u32) -> Result<(), Errno> {
        Err(self.made(path))
    }
    fn watch_key(&self, path: &str) -> Result<(Option<crate::inotify::Key>, bool), Errno> {
        self.inner.watch_key(path)
    }
    fn watched_remotely(&self) -> bool {
        self.inner.watched_remotely()
    }
    fn xattr(&self, path: &str, op: XattrOp<'_>) -> Result<usize, Errno> {
        if matches!(op, XattrOp::Set { .. } | XattrOp::Remove { .. }) {
            return Err(self.there(path));
        }
        self.inner.xattr(path, op)
    }
}
