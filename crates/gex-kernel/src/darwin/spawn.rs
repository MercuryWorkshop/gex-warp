//! `posix_spawn`: a new process running a program, in one call.
//!
//! XNU makes the child and has it take up the program without ever running
//! the caller's code in it. Here a new process is a copy of its parent,
//! which only the host can make and which then runs where its parent was;
//! so the call is a `fork`, and the copy is sent straight back in: its
//! return address is set ([`set_fork_child`]) to two calls the kernel keeps
//! in the task's memory beside the commpage ([`commpage::TEXT`], as XNU's
//! commpage has text of its own), which make this same call again with its
//! arguments untouched. The kernel knows the copy by where the call is
//! made from, does what the attributes and file actions ask, and the call
//! ends as an `execve`.
//! The parent, meanwhile, has the child's pid where it asked for it.
//!
//! What can be known to fail is found before the fork (the program is not
//! there, or may not be run) and is the call's error. What fails after it,
//! in the child, ends the child with status 127.

use super::nr as d;
use super::*;
use crate::syscalls::nr;

// `psa_flags`.
const SETPGROUP: u32 = 0x0002;
const SETSIGDEF: u32 = 0x0004;
const SETSIGMASK: u32 = 0x0008;
const SETEXEC: u32 = 0x0040;
const SETSID: u32 = 0x0400;
const CLOEXEC_DEFAULT: u32 = 0x4000;

/// The size of one file action (`_psfa_action_t`): its kind, a descriptor,
/// and room for a path.
const ACTION: u64 = 1040;
const AT_FDCWD: u64 = -100i64 as u64;
const SIGCHLD: u64 = 17;

/// The two calls the copy is sent back through: this call again, and, for
/// the `execve` it becomes having failed, `exit(127)`.
const TEXT_ARM64: [u32; 6] = [
    0xd280_1e90, // mov x16, #244
    0xd400_1001, // svc #0x80
    0xd280_0fe0, // mov x0, #127
    0xd280_0030, // mov x16, #1
    0xd400_1001, // svc #0x80
    0xd420_0020, // brk #1
];
const TEXT_X86: [u8; 21] = [
    0xb8, 0xf4, 0x00, 0x00, 0x02, // mov eax, 0x20000f4
    0x0f, 0x05, // syscall
    0xbf, 0x7f, 0x00, 0x00, 0x00, // mov edi, 127
    0xb8, 0x01, 0x00, 0x00, 0x02, // mov eax, 0x2000001
    0x0f, 0x05, // syscall
    0x0f, 0x0b, // ud2
];

/// The code a `posix_spawn`'s copy re-enters the kernel through, at the
/// start of the kernel's page of it ([`Kernel::darwin_text_page`]).
pub(super) fn text() -> Vec<u8> {
    if gex_abi::X86 { TEXT_X86.to_vec() } else { TEXT_ARM64.iter().flat_map(|w| w.to_le_bytes()).collect() }
}

impl Kernel {
    /// `posix_spawn(pid, path, adesc, argv, envp)`: the canonical call it
    /// becomes (a fork in the caller, an exec in the copy), or its answer
    /// left for [`Kernel::darwin_native`].
    pub(super) fn darwin_posix_spawn(&mut self, m: &mut GuestMem<'_>, x: [u64; 8], a: &mut [u64; 6]) -> u64 {
        let (pid_at, path, adesc) = (x[0], x[1], x[2]);
        // The copy makes the call from where it was sent; nothing else
        // does.
        let child = (commpage::TEXT..commpage::TEXT + commpage::PAGE).contains(&self.darwin_pc);
        let flags = match self.darwin_spawn_flags(m, adesc) {
            Ok(f) => f,
            Err(e) => return self.darwin_spawn_fails(child, e, a),
        };
        // The copy, or a caller that asked to become the program itself.
        if child || flags & SETEXEC != 0 {
            return match self.darwin_spawn_prepare(m, adesc) {
                Ok(()) => {
                    *a = [path, x[3], x[4], 0, 0, 0];
                    nr::EXECVE
                }
                Err(e) => self.darwin_spawn_fails(child, e, a),
            };
        }
        // The caller: the program must be there to be run, and the file
        // actions as long as they say they are.
        if let Ok(Err(e)) = self.dispatch(m, nr::FACCESSAT, [AT_FDCWD, path, 1, 0, 0, 0]) {
            return self.darwin_spawn_fails(false, e, a);
        }
        if let Err(e) = self.darwin_spawn_actions(m, adesc) {
            return self.darwin_spawn_fails(false, e, a);
        }
        // What XNU finds before there is a child, and so answers: a
        // directory is no program (EACCES), and a directory the actions move
        // to must be one (ENOENT, ENOTDIR). Found in the copy, they were
        // only its exit status of 127.
        if self.darwin_is_dir(m, path) {
            return self.darwin_spawn_fails(false, Errno::EACCES, a);
        }
        if let Err(e) = self.darwin_spawn_dirs(m, adesc) {
            return self.darwin_spawn_fails(false, e, a);
        }
        self.darwin_text_page(m);
        self.darwin_spawn_at = Some(pid_at);
        // The child's own group is there when the call returns, as it is on
        // a Mac: the parent sets it too (`setpgid` from either side is the
        // usual answer to the race), as soon as the child has a pid.
        self.darwin_spawn_pgroup = None;
        if flags & SETPGROUP != 0 {
            if let Ok(attrp) = m.read_u64(adesc + 8) {
                self.darwin_spawn_pgroup = m.read_u32(attrp + 12).ok();
            }
        }
        *a = [SIGCHLD, 0, 0, 0, 0, 0];
        nr::CLONE
    }

    /// Whether the guest path at `path` names a directory.
    fn darwin_is_dir(&mut self, m: &mut GuestMem<'_>, path: u64) -> bool {
        self.darwin_dir_of(m, path).is_ok()
    }

    /// The guest path at `path` as a directory: Ok when it is one, else
    /// what a `chdir` to it answers (ENOENT, ENOTDIR).
    fn darwin_dir_of(&mut self, m: &mut GuestMem<'_>, path: u64) -> Result<(), Errno> {
        let io = self.path_io();
        let p = io.at_path(m, AT_FDCWD as i32, path)?;
        match io.ns.metadata(&p)?.file_type {
            crate::vfs::FileType::Directory => Ok(()),
            _ => Err(Errno::ENOTDIR),
        }
    }

    /// Each directory the file actions of `adesc` move to (`addchdir_np`)
    /// must be one: the error XNU answers for the first that is not.
    fn darwin_spawn_dirs(&mut self, m: &mut GuestMem<'_>, adesc: u64) -> Result<(), Errno> {
        let Some((actions, count)) = self.darwin_spawn_actions(m, adesc)? else { return Ok(()) };
        for i in 0..count {
            let at = actions + 8 + i * ACTION;
            if m.read_u32(at)? != 5 {
                continue;
            }
            self.darwin_dir_of(m, at + 8)?;
        }
        Ok(())
    }

    /// The call has failed with `e`: its answer, or the copy's end.
    fn darwin_spawn_fails(&mut self, child: bool, e: Errno, a: &mut [u64; 6]) -> u64 {
        if child {
            *a = [127, 0, 0, 0, 0, 0];
            return nr::EXIT_GROUP;
        }
        self.darwin_answer = Some(Err(e));
        NATIVE + d::POSIX_SPAWN
    }

    /// `psa_flags` of the attributes `adesc` (a `struct
    /// _posix_spawn_args_desc`) points at; none is no flags.
    fn darwin_spawn_flags(&self, m: &GuestMem<'_>, adesc: u64) -> Result<u32, Errno> {
        if adesc == 0 {
            return Ok(0);
        }
        let (size, attrp) = (m.read_u64(adesc)?, m.read_u64(adesc + 8)?);
        if attrp == 0 || size < 16 {
            return Ok(0);
        }
        Ok(m.read_u32(attrp)? & 0xffff)
    }

    /// Where the file actions of `adesc` are and how many, when there are
    /// any: { allocated, count, the actions }, of the size said.
    fn darwin_spawn_actions(&self, m: &GuestMem<'_>, adesc: u64) -> Result<Option<(u64, u64)>, Errno> {
        if adesc == 0 {
            return Ok(None);
        }
        let (size, actions) = (m.read_u64(adesc + 16)?, m.read_u64(adesc + 24)?);
        if actions == 0 || size == 0 {
            return Ok(None);
        }
        if size < 8 {
            return Err(Errno::EINVAL);
        }
        let count = m.read_u32(actions + 4)? as u64;
        if count == 0 || 8 + count * ACTION != size {
            return Err(Errno::EINVAL);
        }
        Ok(Some((actions, count)))
    }

    /// Make this process what the attributes and file actions say the new
    /// program is to find: its session and group, its signals, its
    /// descriptors and directory.
    fn darwin_spawn_prepare(&mut self, m: &mut GuestMem<'_>, adesc: u64) -> Result<(), Errno> {
        if adesc == 0 {
            return Ok(());
        }
        let call = |k: &mut Kernel, m: &mut GuestMem<'_>, n: u64, a: [u64; 6]| -> SysResult { k.dispatch(m, n, a).unwrap_or(Err(Errno::EINVAL)) };
        let flags = self.darwin_spawn_flags(m, adesc)?;
        // (flags, the signals to reset, the signals to block, the group.)
        let attrp = m.read_u64(adesc + 8)?;
        if flags & SETSID != 0 {
            call(self, m, nr::SETSID, [0; 6])?;
        }
        if flags & SETPGROUP != 0 {
            call(self, m, nr::SETPGID, [0, m.read_u32(attrp + 12)? as u64, 0, 0, 0, 0])?;
        }
        if flags & SETSIGMASK != 0 {
            // (Neither SIGKILL nor SIGSTOP is anybody's to block.)
            let mask = abi::sigset_to_linux(m.read_u32(attrp + 8)?);
            self.signals.set_blocked(mask & !(1 << 8 | 1 << 18));
        }
        if flags & SETSIGDEF != 0 {
            let set = m.read_u32(attrp + 4)?;
            for sig in 1..32u32 {
                let l = abi::sig_to_linux(sig);
                if set & 1 << (sig - 1) != 0 && l != 0 {
                    let _ = self.signals.set_action(l, crate::signal::SigAction::default());
                }
            }
        }
        // The file actions, in order.
        let mut kept: Vec<i32> = Vec::new();
        if let Some((actions, count)) = self.darwin_spawn_actions(m, adesc)? {
            for i in 0..count {
                let at = actions + 8 + i * ACTION;
                let (kind, fd) = (m.read_u32(at)?, m.read_u32(at + 4)? as i32);
                match kind {
                    // Open (flags, mode, path) as `fd`.
                    0 => {
                        let mode = u16::from_le_bytes(m.read(at + 12, 2)?.try_into().unwrap());
                        let got = call(self, m, nr::OPENAT, [AT_FDCWD, at + 14, m.read_u32(at + 8)? as u64, mode as u64, 0, 0])? as i32;
                        if got != fd {
                            self.sys_dup2(got, fd)?;
                            self.sys_close(got)?;
                        }
                        kept.push(fd);
                    }
                    1 => {
                        self.sys_close(fd)?;
                    }
                    // `fd` as another; as itself, it is only kept open.
                    2 => {
                        let new = m.read_u32(at + 8)? as i32;
                        if new == fd {
                            call(self, m, nr::FCNTL, [fd as u64, 2, 0, 0, 0, 0])?;
                        } else {
                            self.sys_dup2(fd, new)?;
                        }
                        kept.push(new);
                    }
                    // Kept open in the new program whatever it was marked.
                    3 => {
                        call(self, m, nr::FCNTL, [fd as u64, 2, 0, 0, 0, 0])?;
                        kept.push(fd);
                    }
                    5 => {
                        call(self, m, nr::CHDIR, [at + 8, 0, 0, 0, 0, 0])?;
                    }
                    6 => {
                        call(self, m, nr::FCHDIR, [fd as u64, 0, 0, 0, 0, 0])?;
                    }
                    _ => return Err(Errno::EINVAL),
                }
            }
        }
        // Every descriptor the actions did not name is closed in the new
        // program.
        if flags & CLOEXEC_DEFAULT != 0 {
            for fd in 0..1024i32 {
                if !kept.contains(&fd) && call(self, m, nr::FCNTL, [fd as u64, 1, 0, 0, 0, 0]).is_ok() {
                    let _ = call(self, m, nr::FCNTL, [fd as u64, 2, 1, 0, 0, 0]);
                }
            }
        }
        Ok(())
    }
}

/// A forked copy's context, when the call that made it was `posix_spawn`:
/// its return goes to the calls that send it back into the kernel
/// ([`commpage::TEXT`]). Its stub returns at once on success, through the
/// link register on arm64 and the address on the stack on x86-64. Whether
/// it was one.
pub(super) fn send_back(m: &mut GuestMem<'_>, ctx_addr: u64) -> Result<bool, Errno> {
    let raw = m.read_u64(ctx_addr + ctx::LAST_SYSCALL as u64)?;
    if gex_abi::X86 {
        if raw != 0x0200_0000 | d::POSIX_SPAWN {
            return Ok(false);
        }
        let sp = m.read_u64(ctx_addr + ctx::SP as u64)?;
        m.write_u64(sp, commpage::TEXT)?;
    } else {
        if raw as u32 as u64 != d::POSIX_SPAWN {
            return Ok(false);
        }
        m.write_u64(ctx_addr + ctx::x(30) as u64, commpage::TEXT)?;
    }
    Ok(true)
}
