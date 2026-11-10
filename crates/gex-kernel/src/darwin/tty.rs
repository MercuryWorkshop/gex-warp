//! `ioctl`, which on Darwin is mostly the terminal.
//!
//! BSD's requests are numbered by a letter, a number and the size of what
//! they pass (`_IOR('t', 19, struct termios)`), where Linux's terminal
//! requests are the bare numbers they were in 1991; and BSD's
//! `struct termios` has 64-bit flag words, its control characters in another
//! order, and the line speeds as numbers of their own rather than codes
//! inside `c_cflag`. Each request is made as the kernel's own, with the
//! structure respelled either side of it.

use super::*;

/// Darwin's requests (`<sys/ttycom.h>`, `<sys/filio.h>`).
mod req {
    pub const TIOCGETA: u64 = 0x4048_7413;
    pub const TIOCSETA: u64 = 0x8048_7414;
    pub const TIOCSETAW: u64 = 0x8048_7415;
    pub const TIOCSETAF: u64 = 0x8048_7416;
    pub const TIOCGWINSZ: u64 = 0x4008_7468;
    pub const TIOCSWINSZ: u64 = 0x8008_7467;
    pub const TIOCGPGRP: u64 = 0x4004_7477;
    pub const TIOCSPGRP: u64 = 0x8004_7476;
    pub const TIOCSCTTY: u64 = 0x2000_7461;
    pub const TIOCNOTTY: u64 = 0x2000_7471;
    pub const TIOCOUTQ: u64 = 0x4004_7473;
    pub const TIOCEXCL: u64 = 0x2000_740d;
    pub const TIOCNXCL: u64 = 0x2000_740e;
    pub const TIOCFLUSH: u64 = 0x8004_7410;
    pub const TIOCDRAIN: u64 = 0x2000_745e;
    pub const TIOCSTART: u64 = 0x2000_746e;
    pub const TIOCSTOP: u64 = 0x2000_746f;
    pub const TIOCSTI: u64 = 0x8001_7472;
    pub const TIOCPTYGRANT: u64 = 0x2000_7454;
    pub const TIOCPTYGNAME: u64 = 0x4080_7453;
    pub const TIOCPTYUNLK: u64 = 0x2000_7452;
    pub const FIONREAD: u64 = 0x4004_667f;
    pub const FIONBIO: u64 = 0x8004_667e;
    pub const FIOASYNC: u64 = 0x8004_667d;
    pub const FIOCLEX: u64 = 0x2000_6601;
    pub const FIONCLEX: u64 = 0x2000_6602;
    pub const FIODTYPE: u64 = 0x4004_667a;
}

/// The kernel's.
mod k {
    pub const TCGETS: u64 = 0x5401;
    pub const TCSETS: u64 = 0x5402;
    pub const TCSBRK: u64 = 0x5409;
    pub const TCXONC: u64 = 0x540a;
    pub const TCFLSH: u64 = 0x540b;
    pub const TIOCEXCL: u64 = 0x540c;
    pub const TIOCNXCL: u64 = 0x540d;
    pub const TIOCSCTTY: u64 = 0x540e;
    pub const TIOCGPGRP: u64 = 0x540f;
    pub const TIOCSPGRP: u64 = 0x5410;
    pub const TIOCOUTQ: u64 = 0x5411;
    pub const TIOCSTI: u64 = 0x5412;
    pub const TIOCGWINSZ: u64 = 0x5413;
    pub const TIOCSWINSZ: u64 = 0x5414;
    pub const FIONREAD: u64 = 0x541b;
    pub const FIONBIO: u64 = 0x5421;
    pub const TIOCNOTTY: u64 = 0x5422;
    pub const FIONCLEX: u64 = 0x5450;
    pub const FIOCLEX: u64 = 0x5451;
    pub const FIOASYNC: u64 = 0x5452;
    pub const TIOCGPTN: u64 = 0x8004_5430;
    pub const TIOCSPTLCK: u64 = 0x4004_5431;
}

/// Darwin's `struct termios`, and the kernel's (`TCGETS`'s).
pub const TERMIOS: usize = 72;
const K_TERMIOS: usize = 36;

/// Each `(darwin, kernel)` pair of bits in the four flag words. A field of
/// more than one bit (`CSIZE`) is a pair for each bit: the encodings agree
/// but for where they sit.
const IFLAG: &[(u64, u32)] = &[
    (0x1, 0x1),       // IGNBRK
    (0x2, 0x2),       // BRKINT
    (0x4, 0x4),       // IGNPAR
    (0x8, 0x8),       // PARMRK
    (0x10, 0x10),     // INPCK
    (0x20, 0x20),     // ISTRIP
    (0x40, 0x40),     // INLCR
    (0x80, 0x80),     // IGNCR
    (0x100, 0x100),   // ICRNL
    (0x200, 0x400),   // IXON
    (0x400, 0x1000),  // IXOFF
    (0x800, 0x800),   // IXANY
    (0x2000, 0x2000), // IMAXBEL
    (0x4000, 0x4000), // IUTF8
];
const OFLAG: &[(u64, u32)] = &[
    (0x1, 0x1),   // OPOST
    (0x2, 0x4),   // ONLCR
    (0x4, 0x1800), // OXTABS, which Linux spells TABDLY's XTABS
    (0x10, 0x8),  // OCRNL
    (0x20, 0x10), // ONOCR
    (0x40, 0x20), // ONLRET
];
const CFLAG: &[(u64, u32)] = &[
    (0x100, 0x10),           // CSIZE, low bit
    (0x200, 0x20),           // CSIZE, high bit
    (0x400, 0x40),           // CSTOPB
    (0x800, 0x80),           // CREAD
    (0x1000, 0x100),         // PARENB
    (0x2000, 0x200),         // PARODD
    (0x4000, 0x400),         // HUPCL
    (0x8000, 0x800),         // CLOCAL
    (0x3_0000, 0x8000_0000), // CRTSCTS (CCTS_OFLOW | CRTS_IFLOW)
];
const LFLAG: &[(u64, u32)] = &[
    (0x1, 0x800),         // ECHOKE
    (0x2, 0x10),          // ECHOE
    (0x4, 0x20),          // ECHOK
    (0x8, 0x8),           // ECHO
    (0x10, 0x40),         // ECHONL
    (0x20, 0x400),        // ECHOPRT
    (0x40, 0x200),        // ECHOCTL
    (0x80, 0x1),          // ISIG
    (0x100, 0x2),         // ICANON
    (0x400, 0x8000),      // IEXTEN
    (0x800, 0x1_0000),    // EXTPROC
    (0x40_0000, 0x100),   // TOSTOP
    (0x80_0000, 0x1000),  // FLUSHO
    (0x2000_0000, 0x4000), // PENDIN
    (0x8000_0000, 0x80),  // NOFLSH
];

/// Each `(darwin, kernel)` pair of indices into `c_cc`. Darwin has two the
/// kernel does not (`VDSUSP` 11, `VSTATUS` 18).
const CC: &[(usize, usize)] = &[
    (0, 4),   // VEOF
    (1, 11),  // VEOL
    (2, 16),  // VEOL2
    (3, 2),   // VERASE
    (4, 14),  // VWERASE
    (5, 3),   // VKILL
    (6, 12),  // VREPRINT
    (8, 0),   // VINTR
    (9, 1),   // VQUIT
    (10, 10), // VSUSP
    (12, 8),  // VSTART
    (13, 9),  // VSTOP
    (14, 15), // VLNEXT
    (15, 13), // VDISCARD
    (16, 6),  // VMIN
    (17, 5),  // VTIME
];
/// `VMIN` and `VTIME` are counts; every other entry is a character, which
/// is disabled by `0xff` on Darwin and by zero in the kernel.
const VMIN: usize = 16;
const VTIME: usize = 17;

/// The line speeds the kernel has a code for (`CBAUD`), each with it.
const SPEEDS: &[(u64, u32)] = &[
    (0, 0),
    (50, 1),
    (75, 2),
    (110, 3),
    (134, 4),
    (150, 5),
    (200, 6),
    (300, 7),
    (600, 8),
    (1200, 9),
    (1800, 10),
    (2400, 11),
    (4800, 12),
    (9600, 13),
    (19200, 14),
    (38400, 15),
    (57600, 0x1001),
    (115200, 0x1002),
    (230400, 0x1003),
];
const CBAUD: u32 = 0x100f;

fn to_kernel(table: &[(u64, u32)], v: u64) -> u32 {
    table.iter().filter(|(d, _)| v & d != 0).fold(0, |o, (_, k)| o | k)
}

fn to_darwin(table: &[(u64, u32)], v: u32) -> u64 {
    table.iter().filter(|(_, k)| v & k == *k).fold(0, |o, (d, _)| o | d)
}

/// A Darwin `struct termios` as the kernel's.
pub fn termios_to_kernel(d: &[u8; TERMIOS]) -> [u8; K_TERMIOS] {
    let word = |i: usize| u64::from_le_bytes(d[8 * i..8 * i + 8].try_into().unwrap());
    let speed = word(8);
    let code = SPEEDS.iter().rev().find(|(s, _)| *s <= speed).map_or(0, |(_, c)| *c);
    let flags = [to_kernel(IFLAG, word(0)), to_kernel(OFLAG, word(1)), to_kernel(CFLAG, word(2)) | code, to_kernel(LFLAG, word(3))];
    let mut k = [0u8; K_TERMIOS];
    for (i, f) in flags.iter().enumerate() {
        k[4 * i..4 * i + 4].copy_from_slice(&f.to_le_bytes());
    }
    // c_line at 16, then c_cc.
    for &(dc, kc) in CC {
        let c = d[32 + dc];
        k[17 + kc] = if dc != VMIN && dc != VTIME && c == 0xff { 0 } else { c };
    }
    k
}

/// The kernel's `struct termios` as Darwin's.
pub fn termios_to_darwin(k: &[u8; K_TERMIOS]) -> [u8; TERMIOS] {
    let word = |i: usize| u32::from_le_bytes(k[4 * i..4 * i + 4].try_into().unwrap());
    let cflag = word(2);
    let speed = SPEEDS.iter().find(|(_, c)| *c == cflag & CBAUD).map_or(0, |(s, _)| *s);
    let flags = [to_darwin(IFLAG, word(0)), to_darwin(OFLAG, word(1)), to_darwin(CFLAG, cflag), to_darwin(LFLAG, word(3))];
    let mut d = [0u8; TERMIOS];
    for (i, f) in flags.iter().enumerate() {
        d[8 * i..8 * i + 8].copy_from_slice(&f.to_le_bytes());
    }
    d[32..52].fill(0xff);
    for &(dc, kc) in CC {
        let c = k[17 + kc];
        d[32 + dc] = if dc != VMIN && dc != VTIME && c == 0 { 0xff } else { c };
    }
    d[56..64].copy_from_slice(&speed.to_le_bytes());
    d[64..72].copy_from_slice(&speed.to_le_bytes());
    d
}

impl Kernel {
    /// `ioctl(fd, request, arg)`. A request refused for want of a terminal
    /// is `ENOTTY`, but of a character device that is none (`/dev/null`)
    /// it is `ENODEV`: its driver's answer to a request it does not know.
    pub(super) fn darwin_ioctl(&mut self, m: &mut GuestMem<'_>, fd: i32, request: u64, arg: u64) -> SysResult {
        match self.darwin_ioctl_as(m, fd, request, arg) {
            Err(Errno::ENOTTY) if self.fds.get(fd).is_ok_and(|f| f.handle.metadata().is_ok_and(|md| md.file_type == crate::vfs::FileType::CharDevice)) => Err(Errno::ENODEV),
            r => r,
        }
    }

    fn darwin_ioctl_as(&mut self, m: &mut GuestMem<'_>, fd: i32, request: u64, arg: u64) -> SysResult {
        let request = request & 0xffff_ffff;
        let scratch = self.ctx_addr + SCRATCH_SLOT as u64;
        // A request that is the kernel's under another number.
        let same = match request {
            req::TIOCGWINSZ => Some(k::TIOCGWINSZ),
            req::TIOCSWINSZ => Some(k::TIOCSWINSZ),
            req::TIOCGPGRP => Some(k::TIOCGPGRP),
            req::TIOCSPGRP => Some(k::TIOCSPGRP),
            req::TIOCSCTTY => Some(k::TIOCSCTTY),
            req::TIOCNOTTY => Some(k::TIOCNOTTY),
            req::TIOCOUTQ => Some(k::TIOCOUTQ),
            req::TIOCEXCL => Some(k::TIOCEXCL),
            req::TIOCNXCL => Some(k::TIOCNXCL),
            req::TIOCSTI => Some(k::TIOCSTI),
            req::FIONREAD => Some(k::FIONREAD),
            req::FIONBIO => Some(k::FIONBIO),
            req::FIOASYNC => Some(k::FIOASYNC),
            req::FIOCLEX => Some(k::FIOCLEX),
            req::FIONCLEX => Some(k::FIONCLEX),
            _ => None,
        };
        if let Some(kreq) = same {
            return self.sys_ioctl(m, fd, kreq, arg);
        }
        match request {
            // The kernel's structure is the smaller: it is read into the
            // caller's own buffer, and respelled there.
            req::TIOCGETA => {
                self.fds.get(fd)?;
                let was: [u8; TERMIOS] = m.read(arg, TERMIOS)?.try_into().unwrap();
                if let Err(e) = self.sys_ioctl(m, fd, k::TCGETS, arg) {
                    let _ = m.write(arg, &was);
                    return Err(e);
                }
                let got: [u8; K_TERMIOS] = m.read(arg, K_TERMIOS)?.try_into().unwrap();
                m.write(arg, &termios_to_darwin(&got))?;
                Ok(0)
            }
            // And for a set, the caller's is the kernel's for the length of
            // the call.
            req::TIOCSETA | req::TIOCSETAW | req::TIOCSETAF => {
                self.fds.get(fd)?;
                let was: [u8; TERMIOS] = m.read(arg, TERMIOS)?.try_into().unwrap();
                m.write(arg, &termios_to_kernel(&was))?;
                let r = self.sys_ioctl(m, fd, k::TCSETS + (request - req::TIOCSETA), arg);
                m.write(arg, &was)?;
                r
            }
            // What to discard: FREAD, FWRITE, or (neither named) both.
            req::TIOCFLUSH => {
                let which = match m.read_u32(arg)? & 3 {
                    1 => 0,
                    2 => 1,
                    _ => 2,
                };
                self.sys_ioctl(m, fd, k::TCFLSH, which)
            }
            req::TIOCDRAIN => self.sys_ioctl(m, fd, k::TCSBRK, 1),
            req::TIOCSTART => self.sys_ioctl(m, fd, k::TCXONC, 1),
            req::TIOCSTOP => self.sys_ioctl(m, fd, k::TCXONC, 0),
            // `grantpt`, `unlockpt` and `ptsname`, which are requests of
            // the master here.
            req::TIOCPTYGRANT => self.sys_ioctl(m, fd, k::TIOCGPTN, scratch).map(|_| 0),
            req::TIOCPTYUNLK => {
                m.write_u32(scratch, 0)?;
                self.sys_ioctl(m, fd, k::TIOCSPTLCK, scratch)
            }
            req::TIOCPTYGNAME => {
                self.sys_ioctl(m, fd, k::TIOCGPTN, scratch)?;
                let mut name = format!("/dev/pts/{}", m.read_u32(scratch)?).into_bytes();
                name.resize(128, 0);
                m.write(arg, &name)?;
                Ok(0)
            }
            // What kind of device: none of the ones this asks after.
            req::FIODTYPE => {
                self.fds.get(fd)?;
                Err(Errno::ENOTTY)
            }
            _ => {
                self.fds.get(fd)?;
                if self.trace || std::env::var_os("GEX_DARWIN_UNIMPLEMENTED").is_some() {
                    eprintln!("[gex:{}] ioctl({fd}, {request:#x}): a request Darwin has and gex does not", self.pid);
                }
                Err(Errno::ENOTTY)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn darwin(iflag: u64, oflag: u64, cflag: u64, lflag: u64, speed: u64) -> [u8; TERMIOS] {
        let mut d = [0u8; TERMIOS];
        for (i, f) in [iflag, oflag, cflag, lflag].iter().enumerate() {
            d[8 * i..8 * i + 8].copy_from_slice(&f.to_le_bytes());
        }
        d[32..52].fill(0xff);
        d[56..64].copy_from_slice(&speed.to_le_bytes());
        d[64..72].copy_from_slice(&speed.to_le_bytes());
        d
    }

    #[test]
    fn a_terminal_as_macos_makes_one_is_the_one_linux_makes() {
        // What a new pty has on macOS: ICRNL|IXON|IXANY|IMAXBEL|BRKINT|IUTF8,
        // OPOST|ONLCR, CS8|CREAD|HUPCL, the usual echoes with ICANON|ISIG|
        // IEXTEN, at 9600.
        let mut d = darwin(0x6b02, 0x3, 0x4b00, 0x5cb, 9600);
        d[32] = 4; // VEOF ^D
        d[32 + 8] = 3; // VINTR ^C
        d[32 + 16] = 1; // VMIN
        let k = termios_to_kernel(&d);
        let word = |i: usize| u32::from_le_bytes(k[4 * i..4 * i + 4].try_into().unwrap());
        // ICRNL|IXON|IXANY|IMAXBEL|BRKINT|IUTF8 in Linux's bits.
        assert_eq!(word(0), 0x100 | 0x400 | 0x800 | 0x2000 | 0x2 | 0x4000);
        assert_eq!(word(1), 0x1 | 0x4);
        // CS8|CREAD|HUPCL, B9600.
        assert_eq!(word(2), 0x30 | 0x80 | 0x400 | 13);
        // ECHOKE|ECHOE|ECHO|ECHOCTL|ISIG|ICANON|IEXTEN.
        assert_eq!(word(3), 0x800 | 0x10 | 0x8 | 0x200 | 0x1 | 0x2 | 0x8000);
        assert_eq!((k[17 + 4], k[17], k[17 + 6]), (4, 3, 1));
        // A disabled character is zero there.
        assert_eq!(k[17 + 1], 0);
        assert_eq!(termios_to_darwin(&k), d);
    }

    #[test]
    fn raw_mode_survives_the_trip() {
        let mut d = darwin(0, 0, 0x300 | 0x800, 0, 115200);
        d[32 + 16] = 1;
        d[32 + 17] = 0;
        let k = termios_to_kernel(&d);
        assert_eq!(u32::from_le_bytes(k[8..12].try_into().unwrap()), 0x30 | 0x80 | 0x1002);
        assert_eq!(termios_to_darwin(&k), d);
    }
}
