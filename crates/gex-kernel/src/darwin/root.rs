//! A Mac program's root, in a system that is not a Mac's.
//!
//! A Mac program expects a Mac's tree under `/`: its dynamic linker at
//! `/usr/lib/dyld`, its libraries, its `/etc`. In a system whose tree is
//! Linux's that tree is a directory somewhere, as a Windows program's is a
//! Wine prefix; and a Mac program run there is run in it. When a process
//! that is not in such a root is about to become a Mach-O program
//! ([`Kernel::exec_root`], at its `execve`), the kernel finds the root --
//! the one the environment names (`GEX_DARWIN_ROOT`), else the nearest
//! directory above the program that has a `usr/lib/dyld` -- and enters it
//! ([`crate::vfs::Namespace::enter`]): a `chroot` that can be left. What
//! the two systems share is in the root too ([`SHARED`]): the devices, the
//! temporary files, and the whole of the system the root is in, as a
//! volume. A process in the root that becomes a program which is no Mac's
//! (an ELF one) leaves it, for the tree it was entered from.
//!
//! So a Linux shell runs `/opt/mac/bin/ls` and a Mac shell runs
//! `/Volumes/Linux/usr/bin/ls`, each in the other's terminal, and a file one
//! writes in `/tmp` or under the working directory the other reads.
//!
//! A system whose own tree is a Mac's needs none of this, and nothing is
//! entered there: its root has the linker.
//!
//! The root may be a bottle's ([`super::bottle`]): the environment names
//! the bottle (`GEX_BOTTLE`, its directory), whose configuration names its
//! system layer. That is the tree entered, sealed, with the bottle's own
//! directories bound over it and written through, and the system outside
//! shared in or not as the bottle says.
use super::*;

/// What of the system a Mac program's root has in it too: (where in the
/// root, what of the system).
pub const SHARED: [(&str, &str); 3] = [("/dev", "/dev"), ("/private/tmp", "/tmp"), ("/Volumes/Linux", "/")];

impl Kernel {
    /// The program at `path` is about to be run in this process's place,
    /// with `envp` its environment: the root it is to run in is entered or
    /// left, as the kind of program it is says, and the program's name
    /// there is answered (`path` itself, when nothing changes).
    pub fn exec_root(&mut self, path: &str, envp: &[String]) -> String {
        let head = self.read_head(path, 64).unwrap_or_default();
        if gex_ld::macho::is_macho(&head) && !self.vfs.entered() {
            let named = self.vfs.canonical(path);
            let entered = match self.darwin_bottle_of(path, envp) {
                Some(layout) => self.enter_bottle(&layout),
                None => {
                    let Some(root) = self.darwin_root_of(path, envp) else { return path.to_string() };
                    self.enter_root(&root)
                }
            };
            if entered.is_err() {
                return path.to_string();
            }
            self.vfs.inner_name(&named).unwrap_or(named)
        } else if head.starts_with(b"\x7fELF") && self.vfs.entered() {
            let named = self.vfs.outer_name(path).unwrap_or_else(|| path.to_string());
            if self.vfs.leave() {
                let cwd = self.vfs.cwd.clone();
                let _ = self.set_cwd(&cwd);
            }
            named
        } else {
            path.to_string()
        }
    }

    /// Enter the tree at `root` as a Mac program's root: [`SHARED`] with
    /// it, and the working directory by its name there.
    pub fn enter_root(&mut self, root: &str) -> Result<(), Errno> {
        self.vfs.enter(root, &SHARED)?;
        let cwd = self.vfs.cwd.clone();
        let _ = self.set_cwd(&cwd);
        Ok(())
    }

    /// Enter a tree as it was entered in another process's namespace
    /// ([`crate::vfs::Namespace::entry`]): a fork's child, on a host where
    /// each process has a kernel of its own.
    pub fn enter_root_as(&mut self, root: &str, shared: &[(String, String)], sealed: bool) -> Result<(), Errno> {
        let shared: Vec<(&str, &str)> = shared.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        self.vfs.enter_sealed(root, &shared, sealed)?;
        let cwd = self.vfs.cwd.clone();
        let _ = self.set_cwd(&cwd);
        Ok(())
    }

    /// Enter a bottle: its system layer, sealed, with [`SHARED`] (the
    /// system outside as a volume only if the bottle shares it) and the
    /// bottle's own directories in it.
    pub fn enter_bottle(&mut self, layout: &bottle::Layout) -> Result<(), Errno> {
        let mut shared: Vec<(String, String)> = SHARED.iter().filter(|(_, out)| *out != "/" || layout.shares_system).map(|(a, b)| (a.to_string(), b.to_string())).collect();
        shared.extend(layout.binds.iter().cloned());
        self.enter_root_as(&layout.system, &shared, true)
    }

    /// The bottle a Mac program at `path` is to run in, if its environment
    /// names one ([`bottle::NAMED`]) that is one: how it is laid out. Its
    /// system layer is what its configuration names, else what the
    /// environment or the program's place does ([`Kernel::darwin_root_of`]).
    fn darwin_bottle_of(&self, path: &str, envp: &[String]) -> Option<bottle::Layout> {
        let named = envp.iter().find_map(|e| e.strip_prefix(bottle::NAMED).and_then(|v| v.strip_prefix('='))).filter(|d| !d.is_empty())?;
        let dir = self.vfs.canonical(named);
        let Some(text) = self.read_file(&format!("{dir}{}", bottle::CONF)) else {
            eprintln!("[gex:{}] {}={named}: no bottle there (it has no {}); the program is run without one", self.pid, bottle::NAMED, &bottle::CONF[1..]);
            return None;
        };
        let conf = bottle::Conf::parse(&String::from_utf8_lossy(&text));
        let fallback = self.darwin_root_of(path, envp);
        let system = conf.get("system").map(|s| self.vfs.canonical(s)).or(fallback)?;
        let passwd = self.read_file(&format!("{system}/private/etc/passwd")).unwrap_or_default();
        let there = |p: &str| self.vfs.metadata(p).is_ok_and(|md| md.file_type == crate::vfs::FileType::Directory);
        let mut layout = bottle::Layout::of(&conf, &dir, Some(&system), &String::from_utf8_lossy(&passwd), &there)?;
        layout.system = system;
        Some(layout)
    }

    /// The root a Mac program at `path` runs in: what its environment
    /// names, else the nearest directory above it with a dynamic linker in
    /// it. `None`: the system's own root (which then is a Mac's, or the
    /// program is one that needs none).
    fn darwin_root_of(&self, path: &str, envp: &[String]) -> Option<String> {
        if let Some(named) = envp.iter().find_map(|e| e.strip_prefix("GEX_DARWIN_ROOT=")).filter(|r| !r.is_empty()) {
            let dir = self.vfs.canonical(named);
            return (dir != "/").then_some(dir);
        }
        let mut dir = self.vfs.canonical(path);
        loop {
            let up = dir.rfind('/')?;
            if up == 0 {
                return None;
            }
            dir.truncate(up);
            if self.vfs.metadata(&format!("{dir}/usr/lib/dyld")).is_ok() {
                return Some(dir);
            }
        }
    }
}
