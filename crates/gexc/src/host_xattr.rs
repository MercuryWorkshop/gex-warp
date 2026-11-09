//! The host's `l*xattr`/`f*xattr` calls for gex-kernel's HostFs, which
//! forbids `unsafe` and so takes them as a hook (`set_host_xattr`).

use gex_kernel::errno::Errno;
use gex_kernel::vfs::hostfs::host_xattr::{set_host_xattr, On};
use gex_kernel::vfs::XattrOp;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;

pub fn install() {
    set_host_xattr(call);
}

#[cfg(target_os = "macos")]
mod sys {
    //! macOS names its xattr calls differently from Linux: one call per
    //! operation, with an `options` argument whose `XATTR_NOFOLLOW` stands in
    //! for the `l` prefix, and its own flag values for create/replace.

    pub const XATTR_NOFOLLOW: i32 = 0x0008;
    /// Linux's `XATTR_CREATE`/`XATTR_REPLACE` are 1/2; macOS's are 2/4.
    pub fn set_flags(flags: i32) -> i32 {
        let mut out = 0;
        if flags & 1 != 0 {
            out |= 0x0002; // XATTR_CREATE
        }
        if flags & 2 != 0 {
            out |= 0x0004; // XATTR_REPLACE
        }
        out
    }

    pub unsafe fn get(path: *const libc::c_char, name: *const libc::c_char, b: *mut libc::c_void, l: usize) -> isize {
        libc::getxattr(path, name, b, l, 0, XATTR_NOFOLLOW)
    }
    pub unsafe fn fget(fd: i32, name: *const libc::c_char, b: *mut libc::c_void, l: usize) -> isize {
        libc::fgetxattr(fd, name, b, l, 0, 0)
    }
    pub unsafe fn set(path: *const libc::c_char, name: *const libc::c_char, v: *const libc::c_void, l: usize, flags: i32) -> isize {
        libc::setxattr(path, name, v, l, 0, set_flags(flags) | XATTR_NOFOLLOW) as isize
    }
    pub unsafe fn fset(fd: i32, name: *const libc::c_char, v: *const libc::c_void, l: usize, flags: i32) -> isize {
        libc::fsetxattr(fd, name, v, l, 0, set_flags(flags)) as isize
    }
    pub unsafe fn list(path: *const libc::c_char, b: *mut libc::c_char, l: usize) -> isize {
        libc::listxattr(path, b, l, XATTR_NOFOLLOW)
    }
    pub unsafe fn flist(fd: i32, b: *mut libc::c_char, l: usize) -> isize {
        libc::flistxattr(fd, b, l, 0)
    }
    pub unsafe fn remove(path: *const libc::c_char, name: *const libc::c_char) -> isize {
        libc::removexattr(path, name, XATTR_NOFOLLOW) as isize
    }
    pub unsafe fn fremove(fd: i32, name: *const libc::c_char) -> isize {
        libc::fremovexattr(fd, name, 0) as isize
    }
}

#[cfg(not(target_os = "macos"))]
mod sys {
    pub unsafe fn get(path: *const libc::c_char, name: *const libc::c_char, b: *mut libc::c_void, l: usize) -> isize {
        libc::lgetxattr(path, name, b, l)
    }
    pub unsafe fn fget(fd: i32, name: *const libc::c_char, b: *mut libc::c_void, l: usize) -> isize {
        libc::fgetxattr(fd, name, b, l)
    }
    pub unsafe fn set(path: *const libc::c_char, name: *const libc::c_char, v: *const libc::c_void, l: usize, flags: i32) -> isize {
        libc::lsetxattr(path, name, v, l, flags)
    }
    pub unsafe fn fset(fd: i32, name: *const libc::c_char, v: *const libc::c_void, l: usize, flags: i32) -> isize {
        libc::fsetxattr(fd, name, v, l, flags)
    }
    pub unsafe fn list(path: *const libc::c_char, b: *mut libc::c_char, l: usize) -> isize {
        libc::llistxattr(path, b, l)
    }
    pub unsafe fn flist(fd: i32, b: *mut libc::c_char, l: usize) -> isize {
        libc::flistxattr(fd, b, l)
    }
    pub unsafe fn remove(path: *const libc::c_char, name: *const libc::c_char) -> isize {
        libc::lremovexattr(path, name)
    }
    pub unsafe fn fremove(fd: i32, name: *const libc::c_char) -> isize {
        libc::fremovexattr(fd, name)
    }
}

fn call(on: &On<'_>, op: XattrOp<'_>) -> Result<usize, Errno> {
    let path = match on {
        On::Path(p) => Some(CString::new(p.as_os_str().as_bytes()).map_err(|_| Errno::EINVAL)?),
        On::Fd(_) => None,
    };
    let fd = match on { On::Fd(fd) => *fd, On::Path(_) => -1 };
    let name_c = |n: &str| CString::new(n).map_err(|_| Errno::EINVAL);
    // SAFETY: every pointer is a live CString or slice for the call's length.
    let r: isize = unsafe {
        match op {
            XattrOp::Get { name, buf } => {
                let n = name_c(name)?;
                let (b, l) = (buf.as_mut_ptr() as *mut libc::c_void, buf.len());
                match &path {
                    Some(p) => sys::get(p.as_ptr(), n.as_ptr(), b, l),
                    None => sys::fget(fd, n.as_ptr(), b, l),
                }
            }
            XattrOp::Set { name, value, flags } => {
                let n = name_c(name)?;
                let (v, l) = (value.as_ptr() as *const libc::c_void, value.len());
                (match &path {
                    Some(p) => sys::set(p.as_ptr(), n.as_ptr(), v, l, flags as i32),
                    None => sys::fset(fd, n.as_ptr(), v, l, flags as i32),
                }) as isize
            }
            XattrOp::List { buf } => {
                let (b, l) = (buf.as_mut_ptr() as *mut libc::c_char, buf.len());
                match &path {
                    Some(p) => sys::list(p.as_ptr(), b, l),
                    None => sys::flist(fd, b, l),
                }
            }
            XattrOp::Remove { name } => {
                let n = name_c(name)?;
                (match &path {
                    Some(p) => sys::remove(p.as_ptr(), n.as_ptr()),
                    None => sys::fremove(fd, n.as_ptr()),
                }) as isize
            }
        }
    };
    if r < 0 {
        // (a macOS host's number is respelled: its ENOATTR is not Linux's)
        return Err(Errno::from_io(&std::io::Error::last_os_error()));
    }
    Ok(r as usize)
}
