//! What a Darwin process asks about itself and its machine: `sysctl`,
//! `sysctlbyname`, `csops` and `proc_info`.
//!
//! libSystem asks these before `main` runs -- the page size, the number of
//! processors, the kernel's version, whether the program is signed -- and
//! none of them has a canonical call. The machine described is one Apple
//! made (an M1's identifiers on arm64), running the Darwin whose system
//! calls [`super::nr`] was generated from.

use super::nr as d;
use super::*;

/// The XNU whose system calls [`super::nr`] was generated from.
pub const XNU: &str = "xnu-12377.1.9";

/// Where a Mac says which system it is. The file is the userland's, and
/// what a program is told of the kernel follows it: the version is a
/// setting of the root a program runs in, as a Windows version is of a Wine
/// prefix.
pub const SYSTEM_VERSION: &str = "/System/Library/CoreServices/SystemVersion.plist";

/// The system a process is told it is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Os {
    /// `kern.osproductversion`: the macOS version, "26.0".
    pub product: String,
    /// `kern.osversion`: its build, "25A354".
    pub build: String,
    /// `kern.osrelease`: Darwin's own version, "25.0.0".
    pub release: String,
}

impl Default for Os {
    /// The macOS that shipped [`XNU`].
    fn default() -> Os {
        Os { product: "26.0".into(), build: "25A354".into(), release: "25.0.0".into() }
    }
}

impl Os {
    /// The system a `SystemVersion.plist` describes: its `ProductVersion`
    /// and `ProductBuildVersion`, and the Darwin release that macOS was.
    pub fn from_plist(plist: &[u8]) -> Option<Os> {
        let text = String::from_utf8_lossy(plist);
        let value = |key: &str| {
            let rest = &text[text.find(&format!("<key>{key}</key>"))?..];
            let rest = &rest[rest.find("<string>")? + 8..];
            Some(rest[..rest.find("</string>")?].trim().to_string())
        };
        let product = value("ProductVersion")?;
        let mut parts = product.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
        let (major, minor, patch) = (parts.next()?, parts.next().unwrap_or(0), parts.next().unwrap_or(0));
        // Mac OS X 10.x was Darwin x + 4, its updates Darwin's minor
        // versions; macOS 11 to 15 were Darwin 20 to 24; macOS 26 was
        // Darwin 25; from 27 the two agree.
        let (darwin, sub) = match major {
            10 => (minor + 4, patch),
            11..=15 => (major + 9, minor),
            16..=26 => (major - 1, minor),
            _ => (major, minor),
        };
        let build = value("ProductBuildVersion").unwrap_or_else(|| format!("{darwin}A1"));
        Some(Os { product, build, release: format!("{darwin}.{sub}.0") })
    }
}

const CTL_KERN: i32 = 1;
const CTL_NET: i32 = 4;
const PF_ROUTE: i32 = 17;
const NET_RT_IFLIST: i32 = 3;
const CTL_VM: i32 = 2;
const CTL_HW: i32 = 6;
const CTL_USER: i32 = 8;
const CTL_MAXNAME: u64 = 12;
/// `KERN_PROC` and its selectors, `KERN_PROCARGS2`.
const KERN_PROC: i32 = 14;
const KERN_PROC_ALL: i32 = 0;
const KERN_PROC_PID: i32 = 1;
const KERN_PROCARGS: i32 = 38;
const KERN_PROCARGS2: i32 = 49;
/// The first number of a name that has no fixed one (XNU's `OID_AUTO`
/// starts at 100; a name's is its row here).
const AUTO: i32 = 0x1000;

/// `struct kinfo_proc`.
const KINFO_PROC: usize = 648;
/// `p_flag`: a 64-bit process that has called `exec`.
const P_LP64: u32 = 0x4;
const P_EXEC: u32 = 0x4000;
/// `p_stat`: running.
const SRUN: u8 = 2;

/// A value `sysctl` reports.
enum Val {
    /// A string, with its NUL.
    Str(String),
    /// An `int`.
    Int(i64),
    /// A `long long`, which a caller with an `int`'s room gets as one.
    Quad(i64),
    /// A structure.
    Bytes(Vec<u8>),
}

/// `HW_PAGESIZE`, the number: an `int`, where the name `hw.pagesize` is a
/// `long long` with a number of its own.
const HW_PAGESIZE: i32 = 7;

/// The names there are, with the numbers the fixed ones have had since BSD
/// (`<sys/sysctl.h>`); the rest are numbered by their row.
const NAMES: &[(&str, &[i32])] = &[
    ("kern.ostype", &[CTL_KERN, 1]),
    ("kern.osrelease", &[CTL_KERN, 2]),
    ("kern.osrevision", &[CTL_KERN, 3]),
    ("kern.version", &[CTL_KERN, 4]),
    ("kern.maxvnodes", &[CTL_KERN, 5]),
    ("kern.maxproc", &[CTL_KERN, 6]),
    ("kern.maxfiles", &[CTL_KERN, 7]),
    ("kern.argmax", &[CTL_KERN, 8]),
    ("kern.securelevel", &[CTL_KERN, 9]),
    ("kern.hostname", &[CTL_KERN, 10]),
    ("kern.hostid", &[CTL_KERN, 11]),
    ("kern.clockrate", &[CTL_KERN, 12]),
    ("kern.posix1version", &[CTL_KERN, 17]),
    ("kern.ngroups", &[CTL_KERN, 18]),
    ("kern.job_control", &[CTL_KERN, 19]),
    ("kern.saved_ids", &[CTL_KERN, 20]),
    ("kern.boottime", &[CTL_KERN, 21]),
    ("kern.nisdomainname", &[CTL_KERN, 22]),
    ("kern.maxfilesperproc", &[CTL_KERN, 29]),
    ("kern.maxprocperuid", &[CTL_KERN, 30]),
    ("kern.netboot", &[CTL_KERN, 40]),
    ("kern.osversion", &[CTL_KERN, 65]),
    ("kern.safeboot", &[CTL_KERN, 66]),
    ("kern.osproductversion", &[]),
    ("kern.osproductversioncompat", &[]),
    ("kern.osreleasetype", &[]),
    ("kern.secure_kernel", &[]),
    ("kern.hv_vmm_present", &[]),
    ("kern.bootargs", &[]),
    ("kern.willshutdown", &[]),
    ("kern.uuid", &[]),
    ("kern.bootsessionuuid", &[]),
    ("kern.osvariant_status", &[]),
    ("kern.monotonicclock_usecs", &[]),
    ("vm.loadavg", &[CTL_VM, 2]),
    ("vm.pagesize", &[]),
    ("hw.machine", &[CTL_HW, 1]),
    ("hw.model", &[CTL_HW, 2]),
    ("hw.ncpu", &[CTL_HW, 3]),
    ("hw.byteorder", &[CTL_HW, 4]),
    ("hw.physmem", &[CTL_HW, 5]),
    ("hw.usermem", &[CTL_HW, 6]),
    ("hw.pagesize", &[]),
    ("hw.memsize", &[CTL_HW, 24]),
    ("hw.availcpu", &[CTL_HW, 25]),
    ("hw.target", &[CTL_HW, 26]),
    ("hw.product", &[CTL_HW, 27]),
    ("hw.activecpu", &[]),
    ("hw.physicalcpu", &[]),
    ("hw.physicalcpu_max", &[]),
    ("hw.logicalcpu", &[]),
    ("hw.logicalcpu_max", &[]),
    ("hw.packages", &[]),
    ("hw.pagesize32", &[]),
    ("hw.cputype", &[]),
    ("hw.cpusubtype", &[]),
    ("hw.cpu64bit_capable", &[]),
    ("hw.cpufamily", &[]),
    ("hw.cpusubfamily", &[]),
    ("hw.cachelinesize", &[]),
    ("hw.l1icachesize", &[]),
    ("hw.l1dcachesize", &[]),
    ("hw.l2cachesize", &[]),
    ("hw.tbfrequency", &[]),
    ("hw.memsize_usable", &[]),
    ("hw.nperflevels", &[]),
    ("hw.perflevel0.physicalcpu", &[]),
    ("hw.perflevel0.physicalcpu_max", &[]),
    ("hw.perflevel0.logicalcpu", &[]),
    ("hw.perflevel0.logicalcpu_max", &[]),
    ("hw.perflevel0.name", &[]),
    ("hw.optional.floatingpoint", &[]),
    ("hw.optional.neon", &[]),
    ("hw.optional.AdvSIMD", &[]),
    ("hw.optional.arm64", &[]),
    ("hw.optional.armv8_1_atomics", &[]),
    ("hw.optional.armv8_crc32", &[]),
    ("hw.optional.arm.FEAT_LSE", &[]),
    ("hw.optional.arm.FEAT_CRC32", &[]),
    ("hw.optional.arm.FEAT_LRCPC", &[]),
    ("hw.optional.arm.AdvSIMD", &[]),
    ("hw.optional.arm.FP_SyncExceptions", &[]),
    ("user.cs_path", &[CTL_USER, 1]),
    ("sysctl.proc_translated", &[]),
    ("security.mac.proc_enforce", &[]),
    ("security.mac.vnode_enforce", &[]),
];

/// The processor features a program may ask after and this one has not:
/// zero, as a Mac without them says, where a name nobody knows is an error.
const ABSENT: &[&str] = &[
    "hw.optional.neon_hpfp",
    "hw.optional.neon_fp16",
    "hw.optional.armv8_2_fhm",
    "hw.optional.armv8_2_sha512",
    "hw.optional.armv8_2_sha3",
    "hw.optional.armv8_3_compnum",
    "hw.optional.armv8_gpi",
    "hw.optional.ucnormal_mem",
    "hw.optional.arm.FEAT_AES",
    "hw.optional.arm.FEAT_PMULL",
    "hw.optional.arm.FEAT_SHA1",
    "hw.optional.arm.FEAT_SHA256",
    "hw.optional.arm.FEAT_SHA512",
    "hw.optional.arm.FEAT_SHA3",
    "hw.optional.arm.FEAT_FHM",
    "hw.optional.arm.FEAT_FP16",
    "hw.optional.arm.FEAT_RDM",
    "hw.optional.arm.FEAT_DotProd",
    "hw.optional.arm.FEAT_FCMA",
    "hw.optional.arm.FEAT_JSCVT",
    "hw.optional.arm.FEAT_LSE2",
    "hw.optional.arm.FEAT_LRCPC2",
    "hw.optional.arm.FEAT_FlagM",
    "hw.optional.arm.FEAT_FlagM2",
    "hw.optional.arm.FEAT_FRINTTS",
    "hw.optional.arm.FEAT_DPB",
    "hw.optional.arm.FEAT_DPB2",
    "hw.optional.arm.FEAT_DIT",
    "hw.optional.arm.FEAT_CSV2",
    "hw.optional.arm.FEAT_CSV3",
    "hw.optional.arm.FEAT_SB",
    "hw.optional.arm.FEAT_SSBS",
    "hw.optional.arm.FEAT_BTI",
    "hw.optional.arm.FEAT_BF16",
    "hw.optional.arm.FEAT_I8MM",
    "hw.optional.arm.FEAT_ECV",
    "hw.optional.arm.FEAT_AFP",
    "hw.optional.arm.FEAT_RPRES",
    "hw.optional.arm.FEAT_PAuth",
    "hw.optional.arm.FEAT_PAuth2",
    "hw.optional.arm.FEAT_FPAC",
    "hw.optional.arm.FEAT_SME",
    "hw.optional.arm.FEAT_SME2",
    "hw.optional.arm.FEAT_SPECRES",
    "hw.optional.arm.FEAT_WFxT",
    "hw.optional.arm.FEAT_EBF16",
    "hw.optional.arm.FEAT_MTE",
    "hw.optional.arm.FEAT_MTE2",
    "hw.optional.arm.SME_F32F32",
    "hw.optional.arm.SME_BI32I32",
    "hw.optional.arm.SME_B16F32",
    "hw.optional.arm.SME_F16F32",
    "hw.optional.arm.SME_I8I32",
    "hw.optional.arm.SME_I16I32",
    "hw.optional.arm.FEAT_SME_F64F64",
    "hw.optional.arm.FEAT_SME_I16I64",
    "hw.optional.arm.sme_max_svl_b",
    "hw.optional.arm.caps",
];

/// The top-level number of a name with no fixed one.
/// The machine, as `sysctl` and the host port both tell of it.
pub(super) struct Machine {
    pub cpus: i64,
    /// Bytes of memory. As `sysinfo` has it.
    pub mem: i64,
    /// CPU_TYPE_ARM64 with CPU_SUBTYPE_ARM64E, which every Mac with Apple's
    /// processor says; or CPU_TYPE_X86 (an Intel Mac says so without the
    /// 64-bit flag) with CPU_SUBTYPE_X86_64_ALL.
    pub cputype: i64,
    pub cpusubtype: i64,
}

impl Machine {
    pub fn this() -> Machine {
        let x86 = gex_abi::X86;
        Machine { cpus: crate::cpus::count() as i64, mem: 4 << 30, cputype: if x86 { 7 } else { 0x0100_000c }, cpusubtype: if x86 { 3 } else { 2 } }
    }
}

fn top_of(name: &str) -> i32 {
    match name.split('.').next() {
        Some("kern") => CTL_KERN,
        Some("vm") => CTL_VM,
        Some("hw") => CTL_HW,
        Some("user") => CTL_USER,
        Some("security") => 14,
        _ => 0,
    }
}

/// The numbers of `name`, if there is such a name.
fn oid_of(name: &str) -> Option<Vec<i32>> {
    if let Some(row) = NAMES.iter().position(|(n, _)| *n == name) {
        let fixed = NAMES[row].1;
        return Some(if fixed.is_empty() { vec![top_of(name), AUTO + row as i32] } else { fixed.to_vec() });
    }
    ABSENT.iter().position(|n| *n == name).map(|row| vec![CTL_HW, AUTO + (NAMES.len() + row) as i32])
}

/// The name `oid` numbers, if it numbers one.
fn name_of(oid: &[i32]) -> Option<&'static str> {
    if let Some((n, _)) = NAMES.iter().find(|(_, fixed)| !fixed.is_empty() && *fixed == oid) {
        return Some(n);
    }
    let [top, row] = *oid else { return None };
    let row = row.checked_sub(AUTO)? as usize;
    let name = NAMES.get(row).filter(|(_, fixed)| fixed.is_empty()).map(|(n, _)| *n).or_else(|| ABSENT.get(row.checked_sub(NAMES.len())?).copied())?;
    (top_of(name) == top || (top == CTL_HW && row >= NAMES.len())).then_some(name)
}

impl Kernel {
    /// The system this process is told it is on: what its root's
    /// [`SYSTEM_VERSION`] says, read once, or with none the default.
    pub fn darwin_os(&self) -> &Os {
        self.darwin_os.get_or_init(|| self.read_file(SYSTEM_VERSION).and_then(|p| Os::from_plist(&p)).unwrap_or_default())
    }

    /// The value of the name `name`, if this kernel has one.
    fn darwin_sysctl_value(&self, name: &str) -> Option<Val> {
        use Val::*;
        let x86 = gex_abi::X86;
        let Machine { cpus, mem, cputype, cpusubtype } = Machine::this();
        let page = abi::PAGE_SIZE as i64;
        let nofile = self.rlimits.get()[RLIMIT_NOFILE];
        Some(match name {
            "kern.ostype" => Str("Darwin".into()),
            "kern.osrelease" => Str(self.darwin_os().release.clone()),
            "kern.osrevision" => Int(199506),
            "kern.version" => Str(darwin_version(&self.darwin_os().release)),
            "kern.osversion" => Str(self.darwin_os().build.clone()),
            "kern.osproductversion" => Str(self.darwin_os().product.clone()),
            "kern.osproductversioncompat" => Str("16.0".into()),
            "kern.osreleasetype" => Str("User".into()),
            "kern.hostname" => Str("gex".into()),
            "kern.nisdomainname" | "kern.bootargs" => Str(String::new()),
            "kern.hostid" | "kern.securelevel" | "kern.netboot" | "kern.safeboot" | "kern.secure_kernel" | "kern.hv_vmm_present" | "kern.willshutdown" => Int(0),
            "kern.maxvnodes" => Int(263168),
            "kern.maxproc" => Int(4000),
            "kern.maxprocperuid" => Int(2666),
            "kern.maxfiles" => Int(122880),
            "kern.maxfilesperproc" => Int(nofile[1].min(61440) as i64),
            "kern.argmax" => Int(1 << 20),
            "kern.ngroups" => Int(16),
            "kern.posix1version" => Int(200112),
            "kern.job_control" | "kern.saved_ids" => Int(1),
            // { hz, tick, tickadj, profhz, stathz }
            "kern.clockrate" => Bytes([100i32, 10000, 0, 100, 100].iter().flat_map(|v| v.to_le_bytes()).collect()),
            // A `struct timeval`: when the monotonic clock was zero.
            "kern.boottime" => {
                let (secs, nanos) = (self.clock)();
                let boot = (secs * 1_000_000_000 + nanos as u64).saturating_sub(clock_ns(self.clock, 1));
                let mut b = (boot / 1_000_000_000).to_le_bytes().to_vec();
                b.extend_from_slice(&(boot % 1_000_000_000 / 1000).to_le_bytes());
                Bytes(b)
            }
            "kern.monotonicclock_usecs" => Quad((clock_ns(self.clock, 1) / 1000) as i64),
            "kern.uuid" | "kern.bootsessionuuid" => Str("00000000-0000-0000-0000-000000000000".into()),
            "kern.osvariant_status" => Quad(0),
            // Three fixed-point averages and their scale.
            "vm.loadavg" => {
                let mut b = vec![0u8; 24];
                b[16..24].copy_from_slice(&2048u64.to_le_bytes());
                Bytes(b)
            }
            "vm.pagesize" | "hw.pagesize" | "hw.pagesize32" => Quad(page),
            "hw.machine" => Str(if x86 { "x86_64" } else { "arm64" }.into()),
            "hw.model" | "hw.product" => Str(if x86 { "MacBookPro16,1" } else { "MacBookAir10,1" }.into()),
            "hw.target" => Str(if x86 { "Mac-E1008331FDC96864" } else { "J313AP" }.into()),
            "hw.ncpu" | "hw.availcpu" | "hw.activecpu" | "hw.physicalcpu" | "hw.physicalcpu_max" | "hw.logicalcpu" | "hw.logicalcpu_max" => Int(cpus),
            "hw.perflevel0.physicalcpu" | "hw.perflevel0.physicalcpu_max" | "hw.perflevel0.logicalcpu" | "hw.perflevel0.logicalcpu_max" => Int(cpus),
            "hw.perflevel0.name" => Str("Performance".into()),
            "hw.nperflevels" | "hw.packages" => Int(1),
            "hw.byteorder" => Int(1234),
            // An `int`, so no more than it holds.
            "hw.physmem" | "hw.usermem" => Int(mem.min(i32::MAX as i64)),
            "hw.memsize" | "hw.memsize_usable" => Quad(mem),
            // The family is the M1's, or Skylake's.
            "hw.cputype" => Int(cputype),
            "hw.cpusubtype" => Int(cpusubtype),
            "hw.cpu64bit_capable" => Int(1),
            "hw.cpufamily" => Int(if x86 { 0x37fc_219f } else { 0x1b58_8bb3 }),
            "hw.cpusubfamily" => Int(if x86 { 0 } else { 2 }),
            "hw.cachelinesize" => Quad(if x86 { 64 } else { 128 }),
            "hw.l1icachesize" => Quad(if x86 { 32 << 10 } else { 128 << 10 }),
            "hw.l1dcachesize" => Quad(if x86 { 32 << 10 } else { 64 << 10 }),
            "hw.l2cachesize" => Quad(if x86 { 256 << 10 } else { 4 << 20 }),
            // `mach_absolute_time` counts nanoseconds ([`Kernel::darwin_abstime`]).
            "hw.tbfrequency" => Quad(1_000_000_000),
            "hw.optional.floatingpoint" => Int(1),
            "hw.optional.neon" | "hw.optional.AdvSIMD" | "hw.optional.arm64" | "hw.optional.armv8_1_atomics" | "hw.optional.armv8_crc32" if !x86 => Int(1),
            "hw.optional.arm.FEAT_LSE" | "hw.optional.arm.FEAT_CRC32" | "hw.optional.arm.FEAT_LRCPC" | "hw.optional.arm.AdvSIMD" | "hw.optional.arm.FP_SyncExceptions" if !x86 => Int(1),
            "user.cs_path" => Str("/usr/bin:/bin:/usr/sbin:/sbin".into()),
            // Not under Rosetta.
            "sysctl.proc_translated" if !x86 => Int(0),
            "security.mac.proc_enforce" | "security.mac.vnode_enforce" => Int(1),
            n if ABSENT.contains(&n) && !x86 => Int(0),
            _ => return None,
        })
    }

    /// `sysctl(name, namelen, old, oldlenp, new, newlen)`: the value a
    /// vector of numbers names.
    pub(super) fn darwin_sysctl(&mut self, m: &mut GuestMem<'_>, x: [u64; 8]) -> SysResult {
        let (name, namelen, old, oldlenp, new, newlen) = (x[0], x[1] & 0xffff_ffff, x[2], x[3], x[4], x[5]);
        if !(2..=CTL_MAXNAME).contains(&namelen) {
            return Err(Errno::EINVAL);
        }
        let mut oid = Vec::with_capacity(namelen as usize);
        for i in 0..namelen {
            oid.push(m.read_u32(name + 4 * i)? as i32);
        }
        match oid[..] {
            // sysctl.name2oid: the numbers of the name in `new`.
            [0, 3] => {
                let text = m.read(new, (newlen as usize).min(256))?.to_vec();
                let text = String::from_utf8_lossy(&text);
                let text = text.trim_end_matches('\0');
                let numbers = oid_of(text).ok_or(Errno::ENOENT)?;
                self.darwin_sysctl_out(m, old, oldlenp, Val::Bytes(numbers.iter().flat_map(|v| v.to_le_bytes()).collect()))
            }
            [CTL_KERN, KERN_PROC, what, ..] => {
                let procs = match (what, oid.get(3)) {
                    (KERN_PROC_PID, Some(&pid)) if pid == self.pid => self.darwin_kinfo_proc(),
                    (KERN_PROC_ALL, _) => self.darwin_kinfo_proc(),
                    // Nobody else: no process, and no error.
                    _ => Vec::new(),
                };
                self.darwin_sysctl_out(m, old, oldlenp, Val::Bytes(procs))
            }
            // The routing table's list of interfaces and their addresses
            // (`NET_RT_IFLIST`), which is what `getifaddrs` reads: for an
            // address family or all (0), one interface or all (0).
            [CTL_NET, PF_ROUTE, 0, family, NET_RT_IFLIST, index] => self.darwin_sysctl_out(m, old, oldlenp, Val::Bytes(interface_list(family, index))),
            [CTL_HW, HW_PAGESIZE] => {
                self.darwin_sysctl_out(m, old, oldlenp, Val::Int(abi::PAGE_SIZE as i64))?;
                if new != 0 { Err(Errno::EPERM) } else { Ok(0) }
            }
            [CTL_KERN, KERN_PROCARGS | KERN_PROCARGS2, pid] => {
                if pid != self.pid {
                    return Err(Errno::EINVAL);
                }
                let args = self.darwin_procargs(oid[1] == KERN_PROCARGS2);
                self.darwin_sysctl_out(m, old, oldlenp, Val::Bytes(args))
            }
            _ => {
                let Some(text) = name_of(&oid) else {
                    // Numbers past a value's, as XNU answers them; anything
                    // else is no name.
                    return Err(if oid.len() > 2 && name_of(&oid[..2]).is_some() { Errno::EISDIR } else { Errno::ENOENT });
                };
                self.darwin_sysctl_named(m, text, old, oldlenp, new)
            }
        }
    }

    /// `sysctlbyname(name, namelen, old, oldlenp, new, newlen)`.
    pub(super) fn darwin_sysctlbyname(&mut self, m: &mut GuestMem<'_>, x: [u64; 8]) -> SysResult {
        let (name, namelen, old, oldlenp, new) = (x[0], x[1], x[2], x[3], x[4]);
        if namelen > 256 {
            return Err(Errno::ENAMETOOLONG);
        }
        let text = m.read(name, namelen as usize)?.to_vec();
        let text = String::from_utf8_lossy(&text).into_owned();
        let text = text.trim_end_matches('\0');
        self.darwin_sysctl_named(m, text, old, oldlenp, new)
    }

    fn darwin_sysctl_named(&mut self, m: &mut GuestMem<'_>, name: &str, old: u64, oldlenp: u64, new: u64) -> SysResult {
        let Some(v) = self.darwin_sysctl_value(name) else {
            if self.trace || std::env::var_os("GEX_DARWIN_UNIMPLEMENTED").is_some() {
                eprintln!("[gex:{}] sysctl {name}: no such name", self.pid);
            }
            return Err(Errno::ENOENT);
        };
        self.darwin_sysctl_out(m, old, oldlenp, v)?;
        // Nothing here is set.
        if new != 0 {
            return Err(Errno::EPERM);
        }
        Ok(0)
    }

    /// Hand a value to `old`, as XNU's handlers do: the value and its
    /// length through `oldlenp`, or the length alone when there is no
    /// buffer (which is how a caller asks for it). Into too small a buffer
    /// a string is not written at all and its length is zero; anything else
    /// is cut to fit. Both are `ENOMEM`.
    fn darwin_sysctl_out(&mut self, m: &mut GuestMem<'_>, old: u64, oldlenp: u64, v: Val) -> SysResult {
        let room = if oldlenp != 0 { m.read_u64(oldlenp)? } else { 0 };
        let string = matches!(v, Val::Str(_));
        let bytes = match v {
            Val::Str(s) => {
                let mut b = s.into_bytes();
                b.push(0);
                b
            }
            Val::Int(v) => (v as i32).to_le_bytes().to_vec(),
            Val::Quad(v) if old != 0 && room == 4 => i32::try_from(v).map_err(|_| Errno::ERANGE)?.to_le_bytes().to_vec(),
            Val::Quad(v) => v.to_le_bytes().to_vec(),
            Val::Bytes(b) => b,
        };
        if old == 0 {
            if oldlenp != 0 {
                m.write_u64(oldlenp, bytes.len() as u64)?;
            }
            return Ok(0);
        }
        let n = bytes.len().min(room as usize);
        if n < bytes.len() {
            let n = if string { 0 } else { n };
            m.write(old, &bytes[..n])?;
            if oldlenp != 0 {
                m.write_u64(oldlenp, n as u64)?;
            }
            return Err(Errno::ENOMEM);
        }
        m.write(old, &bytes)?;
        if oldlenp != 0 {
            m.write_u64(oldlenp, n as u64)?;
        }
        Ok(0)
    }

    /// This process's `struct kinfo_proc`.
    fn darwin_kinfo_proc(&self) -> Vec<u8> {
        const EPROC: usize = 296;
        let mut b = vec![0u8; KINFO_PROC];
        let w32 = |b: &mut [u8], at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        w32(&mut b, 32, P_LP64 | P_EXEC);
        b[36] = SRUN;
        w32(&mut b, 40, self.pid as u32);
        // p_priority, then p_comm (16 bytes and a NUL).
        b[240] = 31;
        let comm = self.darwin_comm();
        b[243..243 + comm.len()].copy_from_slice(comm.as_bytes());
        let [ruid, uid, svuid] = self.creds.uids();
        let [rgid, gid, svgid] = self.creds.gids();
        // e_pcred: p_ruid, p_svuid, p_rgid, p_svgid.
        w32(&mut b, EPROC + 16 + 80, ruid);
        w32(&mut b, EPROC + 16 + 84, svuid);
        w32(&mut b, EPROC + 16 + 88, rgid);
        w32(&mut b, EPROC + 16 + 92, svgid);
        // e_ucred: cr_ref, cr_uid, cr_ngroups, cr_groups[0].
        w32(&mut b, EPROC + 120, 1);
        w32(&mut b, EPROC + 120 + 4, uid);
        b[EPROC + 120 + 8] = 1;
        w32(&mut b, EPROC + 120 + 12, gid);
        w32(&mut b, EPROC + 264, self.procs.ppid(self.pid) as u32);
        w32(&mut b, EPROC + 268, self.procs.pgid(self.pid, 0).unwrap_or(self.pid) as u32);
        // e_tdev: no terminal; e_tpgid likewise.
        w32(&mut b, EPROC + 276, u32::MAX);
        b
    }

    /// The program's name as `ps` shows it: its file's, 16 bytes at most.
    fn darwin_comm(&self) -> String {
        let name = self.exe_path.rsplit('/').next().unwrap_or("");
        let mut end = name.len().min(16);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        name[..end].to_string()
    }

    /// `KERN_PROCARGS2`: the argument count, the executable's path, then
    /// the arguments and the environment, each string with its NUL.
    /// `KERN_PROCARGS` is the same without the count.
    fn darwin_procargs(&self, counted: bool) -> Vec<u8> {
        let mut b = Vec::new();
        if counted {
            b.extend_from_slice(&(self.argv.len() as u32).to_le_bytes());
        }
        b.extend_from_slice(self.exe_path.as_bytes());
        b.push(0);
        // The strings begin on a word, as they do on the stack.
        while b.len() % 8 != 0 {
            b.push(0);
        }
        for s in self.argv.iter().chain(self.envp.iter()) {
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        b
    }

    /// `csops(pid, op, addr, size)` (and `csops_audittoken`, whose token
    /// names the same process): the code-signing state of a process. Every
    /// program here is as one signed ad hoc and valid: no team, no
    /// entitlements.
    pub(super) fn darwin_csops(&mut self, m: &mut GuestMem<'_>, x: [u64; 8]) -> SysResult {
        const STATUS: u64 = 0;
        const MARKINVALID: u64 = 1;
        const MARKHARD: u64 = 2;
        const MARKKILL: u64 = 3;
        const CDHASH: u64 = 5;
        const ENTITLEMENTS_BLOB: u64 = 7;
        const MARKRESTRICT: u64 = 8;
        const BLOB: u64 = 10;
        const IDENTITY: u64 = 11;
        const TEAMID: u64 = 14;
        const DER_ENTITLEMENTS_BLOB: u64 = 16;
        const CS_VALID: u32 = 0x1;
        const CS_ADHOC: u32 = 0x2;
        const CS_SIGNED: u32 = 0x2000_0000;
        let (pid, op, addr, size) = (x[0] as i32, x[1] & 0xffff_ffff, x[2], x[3] & 0xffff_ffff);
        if pid != 0 && pid != self.pid && self.procs.ppid(pid) == 0 && pid != 1 {
            return Err(Errno::ESRCH);
        }
        match op {
            STATUS => {
                if size < 4 {
                    return Err(Errno::EINVAL);
                }
                m.write_u32(addr, CS_VALID | CS_ADHOC | CS_SIGNED)?;
                Ok(0)
            }
            MARKINVALID | MARKHARD | MARKKILL | MARKRESTRICT => Ok(0),
            CDHASH => {
                if size != 20 {
                    return Err(Errno::EINVAL);
                }
                m.zero(addr, 20)?;
                Ok(0)
            }
            // No entitlements: an empty blob header.
            ENTITLEMENTS_BLOB | DER_ENTITLEMENTS_BLOB => {
                if size < 8 {
                    return Err(Errno::ERANGE);
                }
                m.zero(addr, 8)?;
                Ok(0)
            }
            // { type, length (big-endian) } then the identifier.
            IDENTITY => {
                if size < 8 {
                    return Err(Errno::ERANGE);
                }
                let id = self.darwin_comm();
                let whole = 8 + id.len() as u64 + 1;
                m.write_u32(addr, 0)?;
                m.write(addr + 4, &(whole as u32).to_be_bytes())?;
                if size < whole {
                    return Err(Errno::ERANGE);
                }
                m.write(addr + 8, id.as_bytes())?;
                m.write(addr + 8 + id.len() as u64, &[0])?;
                Ok(0)
            }
            TEAMID => Err(Errno::ENOENT),
            BLOB => Err(Errno::EINVAL),
            _ => Err(Errno::EINVAL),
        }
    }

    /// `proc_info(call, pid, flavor, arg, buffer, size)`: what libproc asks
    /// about a process, here the caller's own.
    pub(super) fn darwin_proc_info(&mut self, m: &mut GuestMem<'_>, x: [u64; 8]) -> SysResult {
        const CALL_LISTPIDS: u64 = 1;
        const CALL_PIDINFO: u64 = 2;
        const CALL_SETCONTROL: u64 = 5;
        const CALL_PIDRUSAGE: u64 = 9;
        const CALL_SET_DYLD_IMAGES: u64 = 15;
        const PIDTBSDINFO: u64 = 3;
        const PIDPATHINFO: u64 = 11;
        const PIDT_SHORTBSDINFO: u64 = 13;
        const PIDUNIQIDENTIFIERINFO: u64 = 17;
        const PIDT_BSDINFOWITHUNIQID: u64 = 18;
        const MAXPATHLEN: u64 = 1024;
        let (call, pid, flavor, buf, size) = (x[0] & 0xffff_ffff, x[1] as i32, x[2] & 0xffff_ffff, x[4], x[5] & 0xffff_ffff);
        match call {
            // (type, typeinfo, buffer, size): every process is this one.
            CALL_LISTPIDS => {
                if buf == 0 {
                    return Ok(4 * 20);
                }
                if size < 4 {
                    return Err(Errno::ENOMEM);
                }
                m.write_u32(buf, self.pid as u32)?;
                Ok(4)
            }
            CALL_PIDINFO => {
                if pid != self.pid {
                    return Err(Errno::ESRCH);
                }
                let [ruid, uid, svuid] = self.creds.uids();
                let [rgid, gid, svgid] = self.creds.gids();
                let (ppid, pgid) = (self.procs.ppid(self.pid) as u32, self.procs.pgid(self.pid, 0).unwrap_or(self.pid) as u32);
                let comm = self.darwin_comm();
                let w32 = |b: &mut [u8], at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
                let bsdinfo = |me: &Kernel| {
                    let mut b = vec![0u8; 136];
                    // pbi_flags: PROC_FLAG_LP64; pbi_status: SRUN.
                    w32(&mut b, 0, 0x10);
                    w32(&mut b, 4, SRUN as u32);
                    w32(&mut b, 12, me.pid as u32);
                    w32(&mut b, 16, ppid);
                    for (at, v) in [(20, uid), (24, gid), (28, ruid), (32, rgid), (36, svuid), (40, svgid)] {
                        w32(&mut b, at, v);
                    }
                    b[48..48 + comm.len().min(15)].copy_from_slice(&comm.as_bytes()[..comm.len().min(15)]);
                    b[64..64 + comm.len()].copy_from_slice(comm.as_bytes());
                    w32(&mut b, 96, me.fds.list().len() as u32);
                    w32(&mut b, 100, pgid);
                    w32(&mut b, 108, u32::MAX);
                    b
                };
                let uniq = |me: &Kernel| {
                    let mut b = vec![0u8; 56];
                    b[16..24].copy_from_slice(&(me.pid as u64).to_le_bytes());
                    b[24..32].copy_from_slice(&(ppid as u64).to_le_bytes());
                    w32(&mut b, 32, 1);
                    w32(&mut b, 36, 1);
                    b
                };
                let out = match flavor {
                    PIDPATHINFO => {
                        if size > 4 * MAXPATHLEN {
                            return Err(Errno::EOVERFLOW);
                        }
                        let mut p = self.exe_path.clone().into_bytes();
                        p.push(0);
                        if (size as usize) < p.len() {
                            return Err(Errno::ENOSPC);
                        }
                        m.write(buf, &p)?;
                        return Ok(0);
                    }
                    PIDTBSDINFO => bsdinfo(self),
                    PIDT_SHORTBSDINFO => {
                        let mut b = vec![0u8; 64];
                        w32(&mut b, 0, self.pid as u32);
                        w32(&mut b, 4, ppid);
                        w32(&mut b, 8, pgid);
                        w32(&mut b, 12, SRUN as u32);
                        b[16..16 + comm.len().min(15)].copy_from_slice(&comm.as_bytes()[..comm.len().min(15)]);
                        w32(&mut b, 32, 0x10);
                        for (at, v) in [(36, uid), (40, gid), (44, ruid), (48, rgid), (52, svuid), (56, svgid)] {
                            w32(&mut b, at, v);
                        }
                        b
                    }
                    PIDUNIQIDENTIFIERINFO => uniq(self),
                    PIDT_BSDINFOWITHUNIQID => {
                        let mut b = bsdinfo(self);
                        b.extend_from_slice(&uniq(self));
                        b
                    }
                    _ => return Err(Errno::EINVAL),
                };
                if (size as usize) < out.len() {
                    return Err(Errno::ENOMEM);
                }
                m.write(buf, &out)?;
                Ok(out.len() as u64)
            }
            // What dyld tells the kernel of the images it loaded, and what
            // a process asks of its own scheduling: taken, not kept.
            CALL_SETCONTROL | CALL_SET_DYLD_IMAGES => Ok(0),
            CALL_PIDRUSAGE => Err(Errno::EOPNOTSUPP),
            _ => {
                let n = NATIVE + d::PROC_INFO;
                if !self.unimplemented.contains(&n) {
                    self.unimplemented.push(n);
                    if self.trace || std::env::var_os("GEX_DARWIN_UNIMPLEMENTED").is_some() {
                        eprintln!("[gex:{}] proc_info call {call} flavor {flavor}: not one gex answers", self.pid);
                    }
                }
                Err(Errno::EINVAL)
            }
        }
    }
}

/// `kern.version`, as `uname -v` prints it.
pub fn darwin_version(release: &str) -> String {
    let soc = if gex_abi::X86 { "X86_64" } else { "ARM64_T8103" };
    format!("Darwin Kernel Version {release}: gex; root:{XNU}/RELEASE_{soc}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plist(product: &str, build: &str) -> Vec<u8> {
        format!("<plist><dict>\n<key>ProductBuildVersion</key>\n<string>{build}</string>\n<key>ProductName</key><string>macOS</string>\n<key>ProductVersion</key>\n\t<string>{product}</string></dict></plist>").into_bytes()
    }

    #[test]
    fn the_system_is_the_one_its_version_file_names() {
        let os = |p: &str, b: &str| Os::from_plist(&plist(p, b)).unwrap();
        assert_eq!(os("10.6.8", "10K549"), Os { product: "10.6.8".into(), build: "10K549".into(), release: "10.8.0".into() });
        assert_eq!(os("10.14.6", "18G103").release, "18.6.0");
        assert_eq!(os("11.6", "20G165").release, "20.6.0");
        assert_eq!(os("15.1", "24B83").release, "24.1.0");
        assert_eq!(os("26.0", "25A354"), Os::default());
        assert_eq!(os("27.0.1", "26A434").release, "27.0.0");
        assert_eq!(Os::from_plist(b"not a plist"), None);
    }
}

/// What `sysctl` answers for the list of interfaces: for each, a routing
/// message that says what it is (`RTM_IFINFO`: an `if_msghdr` and its
/// link-level address), then one for each address it has (`RTM_NEWADDR`:
/// an `ifa_msghdr`, then the netmask, the address and where a broadcast
/// goes, in the order of their bits). The kernel's two
/// are named as a Mac names its own: `lo0` and `en0`.
fn interface_list(family: i32, only: i32) -> Vec<u8> {
    const AF_INET: i32 = 2;
    // RTA_NETMASK, RTA_IFA, RTA_BRD.
    const ADDRS: i32 = 0x04 | 0x20 | 0x80;
    let mut out = Vec::new();
    for (index, loopback, mtu, mac, addr, prefix) in crate::netlink::interfaces() {
        if only != 0 && only as u32 != index {
            continue;
        }
        // IFF_UP | IFF_RUNNING | IFF_MULTICAST, and what kind it is:
        // IFF_LOOPBACK, or IFF_BROADCAST | IFF_NOTRAILERS | IFF_SIMPLEX.
        let flags: i32 = 0x8041 | if loopback { 0x8 } else { 0x822 };
        // A `sockaddr_dl`: the interface's name, then its hardware address.
        let name: &[u8] = if loopback { b"lo0" } else { b"en0" };
        let mut link = vec![20u8, 18, index as u8, (index >> 8) as u8, if loopback { 0x18 } else { 0x06 }, 3, if loopback { 0 } else { 6 }, 0];
        link.extend_from_slice(name);
        if !loopback {
            link.extend_from_slice(&mac);
        }
        link.resize(20, 0);
        // The `if_msghdr`: its length, version 5, RTM_IFINFO, which
        // addresses follow (RTA_IFP), the flags, the index, and an
        // `if_data` that says the kind, the address's length and the MTU.
        let mut m = vec![0u8; 112];
        m[0..2].copy_from_slice(&132u16.to_le_bytes());
        (m[2], m[3]) = (5, 0x0e);
        m[4..8].copy_from_slice(&0x10i32.to_le_bytes());
        m[8..12].copy_from_slice(&flags.to_le_bytes());
        m[12..14].copy_from_slice(&(index as u16).to_le_bytes());
        (m[16], m[19]) = (if loopback { 0x18 } else { 0x06 }, if loopback { 0 } else { 6 });
        m[24..28].copy_from_slice(&(if loopback { 16384 } else { mtu }).to_le_bytes());
        out.extend_from_slice(&m);
        out.extend_from_slice(&link);
        if family != 0 && family != AF_INET {
            continue;
        }
        let inet = |a: [u8; 4]| {
            let mut s = [0u8; 16];
            (s[0], s[1]) = (16, AF_INET as u8);
            s[4..8].copy_from_slice(&a);
            s
        };
        let mask = (!0u32).checked_shl(32 - prefix as u32).unwrap_or(0).to_be_bytes();
        let broadcast = if loopback { addr } else { std::array::from_fn(|i| addr[i] | !mask[i]) };
        // The `ifa_msghdr`: length, version, RTM_NEWADDR, the addresses
        // that follow, flags (RTF_UP), the index, a metric.
        let mut a = vec![0u8; 20];
        a[0..2].copy_from_slice(&(20u16 + 16 + 16 + 16).to_le_bytes());
        (a[2], a[3]) = (5, 0x0c);
        a[4..8].copy_from_slice(&ADDRS.to_le_bytes());
        a[8..12].copy_from_slice(&1i32.to_le_bytes());
        a[12..14].copy_from_slice(&(index as u16).to_le_bytes());
        out.extend_from_slice(&a);
        out.extend_from_slice(&inet(mask));
        out.extend_from_slice(&inet(addr));
        out.extend_from_slice(&inet(broadcast));
    }
    out
}
