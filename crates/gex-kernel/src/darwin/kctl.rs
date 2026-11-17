//! Kernel control sockets: `socket(PF_SYSTEM, SOCK_DGRAM, SYSPROTO_CONTROL)`,
//! a control found by name (`ioctl(CTLIOCGINFO)`) and connected to by its
//! id (`struct sockaddr_ctl`), then a datagram conversation with that part of
//! the kernel.
//!
//! The one control here is the network statistics' (`com.apple.network.statistics`,
//! XNU's `ntstat`): symptomsd's NetworkStatistics manager opens it at start,
//! and without it its KernelStatisticsHandler is nil, which symptomsd puts in
//! a dictionary and aborts on. The network here has no sources to report (no
//! sockets of the guest's are the kernel's to count), so every request about
//! all of them is answered as XNU answers one with none: success, and a
//! request about one source is refused, it having none (`ENOENT`). Any other
//! control name is not there (`ENOENT`, as XNU answers an unregistered one).

use crate::errno::{Errno, SysResult};
use crate::vfs::{FileHandle, FileType, Metadata};
use std::collections::VecDeque;
use std::sync::Mutex;

/// `PF_SYSTEM`, and its control protocol.
pub const PF_SYSTEM: u64 = 32;
pub const SYSPROTO_CONTROL: u64 = 2;
/// `_IOWR('N', 3, struct ctl_info)`: a control's id by its name.
pub const CTLIOCGINFO: u64 = 0xc064_4e03;
/// `AF_SYS_CONTROL`, the `ss_sysaddr` of a `struct sockaddr_ctl`.
const AF_SYS_CONTROL: u16 = 2;

/// The controls there are, by name: their ids.
const CONTROLS: &[(&str, u32)] = &[("com.apple.network.statistics", NTSTAT_ID)];
const NTSTAT_ID: u32 = 1;

// ntstat's messages (`bsd/net/ntstat.h`): a `nstat_msg_hdr` (context,
// type, length, flags) and what each type carries after it.
const SUCCESS: u32 = 0;
const ERROR: u32 = 1;
const ADD_SRC: u32 = 1001;
const ADD_ALL_SRCS: u32 = 1002;
const REM_SRC: u32 = 1003;
const QUERY_SRC: u32 = 1004;
const GET_SRC_DESC: u32 = 1005;
const GET_UPDATE: u32 = 1007;
const SUBSCRIBE_SYSINFO: u32 = 1008;
const GET_DETAILS: u32 = 1009;
/// `NSTAT_SRC_REF_ALL`: a request about every source.
const SRC_REF_ALL: u64 = u64::MAX;
const HDR: usize = 16;
/// XNU's numbers for the errors a request is refused with.
const ENOENT: u32 = 2;
const EINVAL: u32 = 22;

/// The id a control's name has, if there is such a control.
pub fn control_id(name: &[u8]) -> Option<u32> {
    let name = name.split(|&b| b == 0).next().unwrap_or(&[]);
    CONTROLS.iter().find(|(n, _)| n.as_bytes() == name).map(|(_, id)| *id)
}

/// A kernel control socket: the control it is connected to, and the
/// datagrams the control has for it.
#[derive(Debug)]
pub struct KernControl {
    connected: Mutex<Option<u32>>,
    replies: Mutex<VecDeque<Vec<u8>>>,
    queue: crate::wait::Queue,
}

impl Default for KernControl {
    fn default() -> Self {
        Self::new()
    }
}

impl KernControl {
    pub fn new() -> Self {
        KernControl { connected: Mutex::new(None), replies: Mutex::new(VecDeque::new()), queue: crate::wait::Queue::new() }
    }

    /// `connect` with a `struct sockaddr_ctl` (`sc_len`, `sc_family`,
    /// `ss_sysaddr`, `sc_id`, `sc_unit`): to the control of that id.
    pub fn connect(&self, addr: &[u8]) -> SysResult {
        if addr.len() < 12 || addr[1] as u64 != PF_SYSTEM || u16::from_le_bytes([addr[2], addr[3]]) != AF_SYS_CONTROL {
            return Err(Errno::EINVAL);
        }
        let id = u32::from_le_bytes(addr[4..8].try_into().unwrap());
        if !CONTROLS.iter().any(|(_, c)| *c == id) {
            return Err(Errno::ENOENT);
        }
        let mut c = self.connected.lock().unwrap();
        if c.is_some() {
            return Err(Errno::EISCONN);
        }
        *c = Some(id);
        Ok(0)
    }

    /// ntstat's answer to one request.
    fn ntstat(req: &[u8]) -> Vec<u8> {
        let context = u64::from_le_bytes(req[0..8].try_into().unwrap());
        let kind = u32::from_le_bytes(req[8..12].try_into().unwrap());
        let src = req.get(HDR..HDR + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()));
        let all = src == Some(SRC_REF_ALL);
        let error = match kind {
            ADD_ALL_SRCS | SUBSCRIBE_SYSINFO => 0,
            QUERY_SRC | GET_UPDATE | GET_DETAILS if all => 0,
            QUERY_SRC | GET_UPDATE | GET_DETAILS | REM_SRC | GET_SRC_DESC => ENOENT,
            // (A source to add is a provider's own: there are none.)
            ADD_SRC => ENOENT,
            _ => EINVAL,
        };
        let mut out = Vec::new();
        out.extend_from_slice(&context.to_le_bytes());
        if error == 0 {
            out.extend_from_slice(&SUCCESS.to_le_bytes());
            out.extend_from_slice(&(HDR as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
        } else {
            // A `nstat_msg_error` with the request after it, as XNU sends
            // one back.
            out.extend_from_slice(&ERROR.to_le_bytes());
            out.extend_from_slice(&((HDR + 4 + req.len()) as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&error.to_le_bytes());
            out.extend_from_slice(req);
        }
        out
    }
}

impl FileHandle for KernControl {
    fn read_at(&self, _offset: u64, buf: &mut [u8]) -> SysResult {
        let mut q = self.replies.lock().unwrap();
        let Some(d) = q.pop_front() else {
            return Err(Errno::EAGAIN);
        };
        // A datagram: what does not fit is dropped.
        let n = d.len().min(buf.len());
        buf[..n].copy_from_slice(&d[..n]);
        Ok(n as u64)
    }

    fn write_at(&self, _offset: u64, buf: &[u8]) -> SysResult {
        if self.connected.lock().unwrap().is_none() {
            return Err(Errno::ENOTCONN);
        }
        if buf.len() < HDR {
            return Err(Errno::EINVAL);
        }
        self.replies.lock().unwrap().push_back(Self::ntstat(buf));
        self.queue.notify();
        Ok(buf.len() as u64)
    }

    fn metadata(&self) -> Result<Metadata, Errno> {
        Ok(Metadata { owner: None, file_type: FileType::Fifo, size: 0, mode: 0o600, ino: 0, mtime: 0, nlink: 1, rdev: 0, times: Default::default() })
    }

    fn is_stream(&self) -> bool {
        true
    }

    fn poll_readable(&self) -> bool {
        !self.replies.lock().unwrap().is_empty()
    }

    fn wait_queue(&self) -> u32 {
        self.queue.id()
    }

    fn wakes(&self) -> Option<u64> {
        Some(self.queue.wakes())
    }

    fn poll_writable(&self) -> bool {
        true
    }

    fn available(&self) -> Option<u64> {
        Some(self.replies.lock().unwrap().front().map_or(0, |d| d.len() as u64))
    }

    fn as_kctl(&self) -> Option<&KernControl> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(kind: u32, src: Option<u64>) -> Vec<u8> {
        let mut r = 7u64.to_le_bytes().to_vec();
        r.extend_from_slice(&kind.to_le_bytes());
        r.extend_from_slice(&(HDR as u16 + if src.is_some() { 8 } else { 0 }).to_le_bytes());
        r.extend_from_slice(&0u16.to_le_bytes());
        if let Some(s) = src {
            r.extend_from_slice(&s.to_le_bytes());
        }
        r
    }

    fn connected() -> KernControl {
        let k = KernControl::new();
        let mut addr = vec![32u8, 32, 2, 0];
        addr.extend_from_slice(&NTSTAT_ID.to_le_bytes());
        addr.extend_from_slice(&[0; 24]);
        k.connect(&addr).unwrap();
        k
    }

    fn reply(k: &KernControl) -> Vec<u8> {
        let mut b = [0u8; 256];
        let n = k.read_at(0, &mut b).unwrap() as usize;
        b[..n].to_vec()
    }

    #[test]
    fn the_statistics_are_found_by_name_and_nothing_else() {
        let mut name = b"com.apple.network.statistics".to_vec();
        name.resize(96, 0);
        assert_eq!(control_id(&name), Some(NTSTAT_ID));
        assert_eq!(control_id(b"com.apple.netsrc\0"), None);
    }

    #[test]
    fn requests_about_every_source_succeed_and_about_one_are_refused() {
        let k = connected();
        k.write_at(0, &request(ADD_ALL_SRCS, Some(0))).unwrap();
        let r = reply(&k);
        assert_eq!((r.len(), &r[0..8], &r[8..12]), (HDR, &7u64.to_le_bytes()[..], &SUCCESS.to_le_bytes()[..]));
        k.write_at(0, &request(QUERY_SRC, Some(SRC_REF_ALL))).unwrap();
        assert_eq!(&reply(&k)[8..12], &SUCCESS.to_le_bytes());
        let one = request(QUERY_SRC, Some(5));
        k.write_at(0, &one).unwrap();
        let r = reply(&k);
        assert_eq!(&r[8..12], &ERROR.to_le_bytes());
        assert_eq!(u32::from_le_bytes(r[16..20].try_into().unwrap()), 2);
        assert_eq!(&r[20..], &one[..]);
        assert_eq!(k.read_at(0, &mut [0u8; 64]), Err(Errno::EAGAIN));
    }

    #[test]
    fn nothing_is_said_before_connecting_or_to_no_control() {
        let k = KernControl::new();
        assert_eq!(k.write_at(0, &request(ADD_ALL_SRCS, None)), Err(Errno::ENOTCONN));
        let mut addr = vec![32u8, 32, 2, 0];
        addr.extend_from_slice(&99u32.to_le_bytes());
        addr.extend_from_slice(&[0; 24]);
        assert_eq!(k.connect(&addr), Err(Errno::ENOENT));
    }
}

impl crate::syscalls::Kernel {
    /// Whether descriptor `fd` is a kernel control socket.
    pub(super) fn darwin_is_kctl(&self, fd: i32) -> bool {
        self.fds.get(fd).is_ok_and(|f| f.handle.as_kctl().is_some())
    }

    /// `socket(PF_SYSTEM, type, protocol)`: a control socket, datagrams or
    /// a stream of them (XNU takes either). Kernel events
    /// (`SYSPROTO_EVENT`) are not here.
    pub(super) fn darwin_kctl_socket(&mut self, ty: u64, protocol: u64) -> SysResult {
        const SOCK_STREAM: u64 = 1;
        const SOCK_DGRAM: u64 = 2;
        if protocol != SYSPROTO_CONTROL || !matches!(ty, SOCK_STREAM | SOCK_DGRAM) {
            return Err(Errno::EPROTONOSUPPORT);
        }
        let handle: std::sync::Arc<dyn FileHandle> = std::sync::Arc::new(KernControl::new());
        let file = crate::file::OpenFile::new(handle, "socket:[kctl]".to_string(), 0, true, true);
        self.fds.insert(file, false)
    }

    /// `ioctl(fd, CTLIOCGINFO, &ctl_info)`: the id of the control named in
    /// `ctl_info`, written into it.
    pub(super) fn darwin_kctl_info(&mut self, m: &mut crate::GuestMem<'_>, fd: i32, info: u64) -> SysResult {
        // (Asked of any other descriptor, XNU says there is no such device.)
        if !self.darwin_is_kctl(fd) {
            self.fds.get(fd)?;
            return Err(Errno::ENODEV);
        }
        let name = m.read(info + 4, 96)?.to_vec();
        let id = control_id(&name).ok_or(Errno::ENOENT)?;
        m.write_u32(info, id)?;
        Ok(0)
    }

    /// `connect(fd, &sockaddr_ctl, len)`.
    pub(super) fn darwin_kctl_connect(&mut self, m: &mut crate::GuestMem<'_>, fd: i32, addr: u64, len: u64) -> SysResult {
        let file = self.fds.get(fd)?;
        let k = file.handle.as_kctl().ok_or(Errno::ENOTSOCK)?;
        let addr = m.read(addr, (len as usize).min(32))?.to_vec();
        k.connect(&addr)
    }
}
