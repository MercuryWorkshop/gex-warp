//! The changes a process made to its mount table, in the order it made them.
//!
//! A process's table is its own (`Vfs`), copied at a fork. Where a fork's
//! child is a process of another memory with a kernel built anew (the
//! page's: each process is a Worker), what it has of its parent's table is
//! what is told it: the system's own mounts it builds itself, and these
//! changes made again on top, in the order they were made -- the binds, the
//! `proc` and `devtmpfs` mounts, a `chroot`, a `pivot_root`. Each names
//! paths as the namespace had them when it was made (absolute, links
//! followed), so made again from the same start it comes to the same table.
//!
//! What is not here: what a host mounted ([`super::VfsView::mount`]: it
//! mounts the same in every process it builds), and what an entered tree
//! (a Mac program's root, [`super::Namespace::enter`]) is, which a child is
//! told by its own record.

/// One change to a mount table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MountOp {
    /// The process has a mount namespace of its own from here.
    Own,
    /// `chroot` to this directory.
    Chroot(String),
    /// Enter a tree ([`super::Namespace::enter`]): its directory, what is
    /// shared into it, and whether it is sealed.
    Enter { dir: String, shared: Vec<(String, String)>, sealed: bool },
    /// `mount --bind` (`MS_REC` when `recursive`).
    Bind { src: String, dst: String, recursive: bool },
    /// A remount of the mount at this place read-only.
    ReadOnly(String),
    /// `pivot_root`.
    Pivot { new_root: String, put_old: String },
    /// `umount` of the mount at exactly this place.
    Unmount(String),
    /// `umount2(MNT_DETACH)` of this place and what is mounted under it.
    UnmountTree(String),
    /// A `tmpfs` mounted here.
    Tmpfs(String),
    /// An empty filesystem mounted here where nothing was (`sysfs`,
    /// `cgroup`: what a program probes for and finds bare).
    Bare(String),
    /// The system's own `/proc` (`ty` "proc") or `/dev` ("devtmpfs"), there
    /// is one of each, mounted here too.
    System { at: String, ty: String },
}

const OP: char = '\x03';
const FIELD: char = '\x04';
const PAIR: char = '\x05';
const SIDE: char = '\x06';

/// The journal as text, to be carried to a process built anew. Paths have
/// no NUL and (here) none of the separators.
pub fn encode(ops: &[MountOp]) -> String {
    let mut out = String::new();
    for (i, op) in ops.iter().enumerate() {
        if i > 0 {
            out.push(OP);
        }
        let f = |out: &mut String, fields: &[&str]| out.push_str(&fields.join(&FIELD.to_string()));
        match op {
            MountOp::Own => f(&mut out, &["own"]),
            MountOp::Chroot(d) => f(&mut out, &["chroot", d]),
            MountOp::Enter { dir, shared, sealed } => {
                let shared: Vec<String> = shared.iter().map(|(a, b)| format!("{a}{SIDE}{b}")).collect();
                f(&mut out, &["enter", dir, if *sealed { "1" } else { "0" }, &shared.join(&PAIR.to_string())]);
            }
            MountOp::Bind { src, dst, recursive } => f(&mut out, &["bind", src, dst, if *recursive { "1" } else { "0" }]),
            MountOp::ReadOnly(a) => f(&mut out, &["ro", a]),
            MountOp::Pivot { new_root, put_old } => f(&mut out, &["pivot", new_root, put_old]),
            MountOp::Unmount(a) => f(&mut out, &["umount", a]),
            MountOp::UnmountTree(a) => f(&mut out, &["umount-tree", a]),
            MountOp::Tmpfs(a) => f(&mut out, &["tmpfs", a]),
            MountOp::Bare(a) => f(&mut out, &["bare", a]),
            MountOp::System { at, ty } => f(&mut out, &["system", at, ty]),
        }
    }
    out
}

/// [`encode`]'s text back; what is not an operation is left out.
pub fn decode(text: &str) -> Vec<MountOp> {
    text.split(OP)
        .filter_map(|op| {
            let f: Vec<&str> = op.split(FIELD).collect();
            let s = |i: usize| f.get(i).map(|s| s.to_string());
            Some(match *f.first()? {
                "own" => MountOp::Own,
                "chroot" => MountOp::Chroot(s(1)?),
                "enter" => MountOp::Enter {
                    dir: s(1)?,
                    sealed: f.get(2) == Some(&"1"),
                    shared: f.get(3)?.split(PAIR).filter_map(|p| p.split_once(SIDE)).map(|(a, b)| (a.to_string(), b.to_string())).collect(),
                },
                "bind" => MountOp::Bind { src: s(1)?, dst: s(2)?, recursive: f.get(3) == Some(&"1") },
                "ro" => MountOp::ReadOnly(s(1)?),
                "pivot" => MountOp::Pivot { new_root: s(1)?, put_old: s(2)? },
                "umount" => MountOp::Unmount(s(1)?),
                "umount-tree" => MountOp::UnmountTree(s(1)?),
                "tmpfs" => MountOp::Tmpfs(s(1)?),
                "bare" => MountOp::Bare(s(1)?),
                "system" => MountOp::System { at: s(1)?, ty: s(2)? },
                _ => return None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_journal_goes_through_its_text() {
        let ops = vec![
            MountOp::Own,
            MountOp::Tmpfs("/mnt/t".into()),
            MountOp::Bind { src: "/dev".into(), dst: "/opt/debian/dev".into(), recursive: true },
            MountOp::Bind { src: "/proc".into(), dst: "/opt/debian/proc".into(), recursive: false },
            MountOp::ReadOnly("/opt/debian/dev".into()),
            MountOp::Pivot { new_root: "/n".into(), put_old: "/n/old".into() },
            MountOp::Unmount("/mnt/t".into()),
            MountOp::UnmountTree("/x".into()),
            MountOp::Bare("/sys".into()),
            MountOp::System { at: "/d".into(), ty: "devtmpfs".into() },
            MountOp::Enter { dir: "/opt/mac".into(), shared: vec![("/dev".into(), "/dev".into()), ("/tmp".into(), "/private/tmp".into())], sealed: true },
            MountOp::Enter { dir: "/opt/mac".into(), shared: vec![], sealed: false },
            MountOp::Chroot("/opt/debian".into()),
        ];
        assert_eq!(decode(&encode(&ops)), ops);
        assert_eq!(decode(""), vec![]);
    }
}
