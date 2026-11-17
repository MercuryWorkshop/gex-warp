//! File ports: an open file as a Mach port (`fileport_makeport`), which a
//! message carries to another task like any send right, and a descriptor
//! for it again there (`fileport_makefd`). How libxpc sends a descriptor
//! (`xpc_fd_create`, `xpc_dictionary_set_fd`): CVMServer is handed the file
//! an OpenCL build's code is to be written to so.

use super::port::{Kind, Port};
use crate::file::OpenFile;
use crate::syscalls::{Kernel, SysResult};
use crate::{Errno, GuestMem};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The files, by the handle their port carries ([`Kind::FilePort`]); one
/// goes when its port does ([`forget`]).
static FILES: Mutex<Option<(u32, HashMap<u32, Arc<OpenFile>>)>> = Mutex::new(None);

fn table() -> std::sync::MutexGuard<'static, Option<(u32, HashMap<u32, Arc<OpenFile>>)>> {
    FILES.lock().unwrap_or_else(|e| e.into_inner())
}

/// A file port's file, when its port has gone.
pub(super) fn forget(h: u32) {
    if let Some((_, map)) = table().as_mut() {
        map.remove(&h);
    }
}

impl Kernel {
    /// `fileport_makeport(fd, &port)`: a send right to a new port for the
    /// file `fd` is open on.
    pub(super) fn darwin_fileport_makeport(&mut self, m: &mut GuestMem<'_>, fd: u64, out: u64) -> SysResult {
        let file = self.fds.get(fd as i32)?;
        let port = {
            let mut t = table();
            let (next, map) = t.get_or_insert_with(|| (1, HashMap::new()));
            let h = *next;
            *next = next.wrapping_add(1).max(1);
            map.insert(h, file);
            Port::new(Kind::FilePort(h))
        };
        let space = self.darwin_space();
        let name = space.lock().unwrap_or_else(|e| e.into_inner()).give_send(port);
        m.write_u32(out, name)?;
        Ok(0)
    }

    /// `fileport_makefd(port)`: a descriptor for the file a file port is
    /// for, closed on exec as XNU makes it.
    pub(super) fn darwin_fileport_makefd(&mut self, name: u64) -> SysResult {
        let space = self.darwin_space();
        let port = space.lock().unwrap_or_else(|e| e.into_inner()).port_named(self.pid, name as u32);
        let Some(Kind::FilePort(h)) = port.map(|p| p.kind) else { return Err(Errno::EINVAL) };
        let file = table().as_ref().and_then(|(_, m)| m.get(&h).cloned()).ok_or(Errno::EINVAL)?;
        self.fds.insert(file, true)
    }
}
