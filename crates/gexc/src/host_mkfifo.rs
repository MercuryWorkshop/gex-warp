//! The host's `mkfifo` for gex-kernel's HostFs, which forbids `unsafe` and so
//! takes it as a hook (`host_mkfifo::set`).

use gex_kernel::errno::Errno;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;

pub fn install() {
    gex_kernel::vfs::hostfs::host_mkfifo::set(call);
}

fn call(host: &std::path::Path, mode: u32) -> Result<(), Errno> {
    let path = CString::new(host.as_os_str().as_bytes()).map_err(|_| Errno::EINVAL)?;
    // SAFETY: a NUL-terminated path that outlives the call.
    if unsafe { libc::mkfifo(path.as_ptr(), mode as libc::mode_t) } == 0 {
        Ok(())
    } else {
        Err(Errno::from_io(&std::io::Error::last_os_error()))
    }
}
