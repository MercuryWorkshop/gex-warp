//! Darwin's socket calls.
//!
//! The calls are BSD's, which Linux's descend from, so each is the
//! canonical call of the same name with its numbers respelled: the address
//! family, the `MSG_*` flags, and the level and name of an option. The
//! structures that differ -- a `sockaddr` that begins with its own length,
//! a `msghdr` with 32-bit lengths, a 12-byte `cmsghdr` -- are read and
//! written in Darwin's layout by the socket code itself, where it asks
//! [`super::on`].

use super::nr as d;
use super::*;
use crate::syscalls::nr;

const AF_INET6: u64 = 30;
const K_AF_INET6: u64 = 10;
const SOL_SOCKET: u64 = 0xffff;
const K_SOL_SOCKET: u64 = 1;
const K_SOCK_NONBLOCK: u64 = 0o4000;

/// Each `(darwin, kernel)` pair of `MSG_*` flags.
const MSG: [(u64, u64); 9] = [
    (0x1, 0x1),          // MSG_OOB
    (0x2, 0x2),          // MSG_PEEK
    (0x4, 0x4),          // MSG_DONTROUTE
    (0x8, 0x80),         // MSG_EOR
    (0x10, 0x20),        // MSG_TRUNC
    (0x20, 0x8),         // MSG_CTRUNC
    (0x40, 0x100),       // MSG_WAITALL
    (0x80, 0x40),        // MSG_DONTWAIT
    (0x8_0000, 0x4000),  // MSG_NOSIGNAL
];

fn msg_flags(f: u64) -> u64 {
    MSG.iter().filter(|(d, _)| f & d != 0).fold(0, |o, (_, k)| o | k)
}

/// Each `(darwin, kernel)` pair of `SOL_SOCKET` option names.
const SO: [(u64, u64); 17] = [
    (0x1, 1),       // SO_DEBUG
    (0x2, 30),      // SO_ACCEPTCONN
    (0x4, 2),       // SO_REUSEADDR
    (0x8, 9),       // SO_KEEPALIVE
    (0x10, 5),      // SO_DONTROUTE
    (0x20, 6),      // SO_BROADCAST
    (0x80, 13),     // SO_LINGER
    (0x100, 10),    // SO_OOBINLINE
    (0x200, 15),    // SO_REUSEPORT
    (0x1001, 7),    // SO_SNDBUF
    (0x1002, 8),    // SO_RCVBUF
    (0x1003, 19),   // SO_SNDLOWAT
    (0x1004, 18),   // SO_RCVLOWAT
    (0x1005, 21),   // SO_SNDTIMEO
    (0x1006, 20),   // SO_RCVTIMEO
    (0x1007, 4),    // SO_ERROR
    (0x1008, 3),    // SO_TYPE
];
const SO_ERROR: u64 = 0x1007;
/// Options with nothing behind them here that a program sets and goes on:
/// `SO_NOSIGPIPE`, `SO_LINGER_SEC`, `SO_TIMESTAMP`, `SO_USELOOPBACK`.
const SO_ACCEPTED: [u64; 4] = [0x1022, 0x1080, 0x400, 0x40];
/// What is waiting to be read, and to be sent.
const SO_NREAD: u64 = 0x1020;
const SO_NWRITE: u64 = 0x1024;

/// The kernel's address family for Darwin's.
fn family(domain: u64) -> u64 {
    match domain {
        AF_INET6 => K_AF_INET6,
        // Linux's AF_INET6 is Darwin's AF_CCITT, which nothing has.
        K_AF_INET6 => u64::MAX,
        f => f,
    }
}

/// The kernel's level and name for an option, if it has the option.
fn option(level: u64, name: u64) -> Option<(u64, u64)> {
    match level & 0xffff_ffff {
        SOL_SOCKET => SO.iter().find(|(d, _)| *d == name & 0xffff_ffff).map(|(_, k)| (K_SOL_SOCKET, *k)),
        // IPPROTO_TCP: TCP_NODELAY and TCP_MAXSEG are the same; the rest
        // are not Linux's.
        6 if matches!(name, 1 | 2) => Some((6, name)),
        _ => None,
    }
}

impl Kernel {
    /// A socket call as its canonical one, if `code` is one.
    pub(super) fn darwin_socket_call(&mut self, m: &mut GuestMem<'_>, code: u64, x: [u64; 8], a: &mut [u64; 6]) -> Option<u64> {
        let mut set = |n: u64, args: &[u64]| {
            *a = [0; 6];
            a[..args.len()].copy_from_slice(args);
            Some(n)
        };
        match code {
            d::SOCKET => set(nr::SOCKET, &[family(x[0]), x[1] & 0xf, x[2]]),
            d::SOCKETPAIR => set(nr::SOCKETPAIR, &[family(x[0]), x[1] & 0xf, x[2], x[3]]),
            d::CONNECT | d::CONNECT_NOCANCEL => set(nr::CONNECT, &x[..3]),
            d::BIND => set(nr::BIND, &x[..3]),
            d::LISTEN => set(nr::LISTEN, &x[..2]),
            // A socket accepted from a non-blocking listener is
            // non-blocking too, on BSD.
            d::ACCEPT | d::ACCEPT_NOCANCEL => {
                let nonblock = self.fds.get(x[0] as i32).is_ok_and(|f| f.flags() & oflags::O_NONBLOCK != 0);
                set(nr::ACCEPT4, &[x[0], x[1], x[2], if nonblock { K_SOCK_NONBLOCK } else { 0 }])
            }
            d::GETPEERNAME => set(nr::GETPEERNAME, &x[..3]),
            d::GETSOCKNAME => set(nr::GETSOCKNAME, &x[..3]),
            // Of a socket: the kernel agrees to one of anything.
            d::SHUTDOWN => match socket_family_type(&self.fd_io(), x[0] as i32) {
                Ok(_) | Err(Errno::EBADF) => set(nr::SHUTDOWN, &x[..2]),
                Err(_) => None,
            },
            d::SENDTO | d::SENDTO_NOCANCEL => set(nr::SENDTO, &[x[0], x[1], x[2], msg_flags(x[3]), x[4], x[5]]),
            d::RECVFROM | d::RECVFROM_NOCANCEL => set(nr::RECVFROM, &[x[0], x[1], x[2], msg_flags(x[3]), x[4], x[5]]),
            d::SENDMSG | d::SENDMSG_NOCANCEL => set(nr::SENDMSG, &[x[0], x[1], msg_flags(x[2])]),
            d::RECVMSG | d::RECVMSG_NOCANCEL => set(nr::RECVMSG, &[x[0], x[1], msg_flags(x[2])]),
            d::SETSOCKOPT => match option(x[1], x[2]) {
                Some((level, name)) => set(nr::SETSOCKOPT, &[x[0], level, name, x[3], x[4]]),
                None => None,
            },
            d::GETSOCKOPT => match option(x[1], x[2]) {
                Some((level, name)) => {
                    if x[1] & 0xffff_ffff == SOL_SOCKET && x[2] & 0xffff_ffff == SO_ERROR {
                        self.darwin_post = Post::SoError { at: x[3] };
                    }
                    let _ = m;
                    set(nr::GETSOCKOPT, &[x[0], level, name, x[3], x[4]])
                }
                None => None,
            },
            _ => None,
        }
    }

    /// `setsockopt` and `getsockopt` of an option the kernel has no name
    /// for: the few Darwin's own programs set are taken, the counts of
    /// waiting bytes answered, and anything else is no option.
    pub(super) fn darwin_sockopt(&mut self, m: &mut GuestMem<'_>, get: bool, x: [u64; 8]) -> SysResult {
        let (fd, level, name, val, len) = (x[0] as i32, x[1] & 0xffff_ffff, x[2] & 0xffff_ffff, x[3], x[4]);
        socket_family_type(&self.fd_io(), fd)?;
        let put = |m: &mut GuestMem<'_>, v: u32| -> SysResult {
            if val != 0 && len != 0 {
                if m.read_u32(len)? < 4 {
                    return Err(Errno::EINVAL);
                }
                m.write_u32(val, v)?;
                m.write_u32(len, 4)?;
            }
            Ok(0)
        };
        match (level, name, get) {
            (SOL_SOCKET, n, false) if SO_ACCEPTED.contains(&n) => Ok(0),
            (SOL_SOCKET, n, true) if SO_ACCEPTED.contains(&n) => put(m, 0),
            (SOL_SOCKET, SO_NREAD, true) => {
                let waiting = self.sys_ioctl(m, fd, 0x541b, self.ctx_addr + SCRATCH_SLOT as u64).and_then(|_| m.read_u32(self.ctx_addr + SCRATCH_SLOT as u64)).unwrap_or(0);
                put(m, waiting)
            }
            (SOL_SOCKET, SO_NWRITE, true) => put(m, 0),
            _ => {
                if self.trace || std::env::var_os("GEX_DARWIN_UNIMPLEMENTED").is_some() {
                    eprintln!("[gex:{}] {}sockopt({fd}, {level:#x}, {name:#x}): an option Darwin has and gex does not", self.pid, if get { "get" } else { "set" });
                }
                Err(Errno::ENOPROTOOPT)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_and_options_are_respelled() {
        // MSG_DONTWAIT | MSG_NOSIGNAL | MSG_PEEK
        assert_eq!(msg_flags(0x80 | 0x8_0000 | 0x2), 0x40 | 0x4000 | 0x2);
        assert_eq!(option(SOL_SOCKET, 0x1007), Some((1, 4)));
        assert_eq!(option(SOL_SOCKET, 0x4), Some((1, 2)));
        assert_eq!(option(SOL_SOCKET, 0x1022), None);
        assert_eq!(option(6, 1), Some((6, 1)));
        assert_eq!(family(30), 10);
        assert_eq!(family(1), 1);
    }
}
