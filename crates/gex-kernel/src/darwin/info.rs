//! What a Darwin process asks about itself and its machine: `sysctl`,
//! `sysctlbyname`, `csops` and `proc_info`.
//!
//! libSystem asks these before `main` runs -- the page size, the number of
//! processors, the kernel's version, whether the program is signed -- and
//! none of them has a canonical call. The machine described is one Apple
//! made ([`Mac`]: a row of the root's table of them, the one the process's
//! bottle or environment chose or one its macOS ran on), running the
//! Darwin whose system calls [`super::nr`] was generated from.

use super::nr as d;
use super::*;

/// The XNU whose system calls [`super::nr`] was generated from.
pub const XNU: &str = "xnu-12377.1.9";

/// Where a Mac says which system it is. The file is the userland's, and
/// what a program is told of the kernel follows it: the version is a
/// setting of the root a program runs in, as a Windows version is of a Wine
/// prefix.
pub const SYSTEM_VERSION: &str = "/System/Library/CoreServices/SystemVersion.plist";
/// Beside it: the iOS a Mac runs Mac Catalyst programs as, and (11 to 15)
/// the version a program built for 10.15 or before is shown instead.
pub const IOS_VERSION: &str = "/System/Library/CoreServices/iOSSystemVersion.plist";
pub const COMPAT_VERSION: &str = "/System/Library/CoreServices/SystemVersionCompat.plist";

/// The releases a root's userland knows, and what each says of itself
/// (pyrus's oss/macos-releases, which its build.sh puts here): one row a
/// release, its fields apart by tabs, `-` for what is not known.
pub const RELEASES: &str = "/usr/share/gex/macos-releases";

/// The environment's word for which release a process is to be told it is
/// on, whatever its root's files say: `GEX_MACOS_VERSION=14`, `=14.8.9`,
/// `=sonoma`. Inherited, as the environment is, by what it runs: a process
/// tree is "on" that release, as a program is under a Wine prefix's
/// Windows version, without the root's being built again. The kernel then
/// gives that process the files that say so, too ([`Os::files`]).
pub const CHOSEN: &str = "GEX_MACOS_VERSION";

/// The system a process is told it is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Os {
    /// `kern.osproductversion`: the macOS version, "27.0".
    pub product: String,
    /// `kern.osversion`: its build, "26A425".
    pub build: String,
    /// `kern.osrelease`: Darwin's own version, "27.0.0".
    pub release: String,
    /// What else the release's row knows ([`Release`]), if there is one.
    pub row: Option<Release>,
    /// Told so by the environment ([`CHOSEN`]), and not by the root's
    /// files: what the files say is given from here.
    pub chosen: bool,
}

impl Default for Os {
    /// The macOS that shipped [`XNU`], for a root that says nothing.
    fn default() -> Os {
        Os { product: "26.0".into(), build: "25A354".into(), release: "25.0.0".into(), row: None, chosen: false }
    }
}

/// One of the files that say which system this is, as a process told it is
/// another release than its root's is given it ([`Os::files`]): a file of
/// the system's, read-only.
#[derive(Debug)]
pub struct VersionFile(pub Vec<u8>);

impl crate::vfs::FileHandle for VersionFile {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> SysResult {
        let start = (offset as usize).min(self.0.len());
        let n = buf.len().min(self.0.len() - start);
        buf[..n].copy_from_slice(&self.0[start..start + n]);
        Ok(n as u64)
    }
    fn metadata(&self) -> Result<crate::vfs::Metadata, Errno> {
        Ok(crate::vfs::Metadata { file_type: crate::vfs::FileType::Regular, size: self.0.len() as u64, mode: 0o644, ino: 0x5653, mtime: 0, nlink: 1, rdev: 0, times: Default::default() })
    }
}

/// A row of [`RELEASES`]: what a release is known to say of itself, where
/// it is known.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Release {
    pub product: String,
    pub build: String,
    pub darwin: Option<String>,
    /// As `kern.version` names the kernel: `xnu-13432.1.9~1`, or as much of
    /// it as is known (`xnu-13432`).
    pub xnu: Option<String>,
    /// When that kernel was built, as `kern.version` says it.
    pub kdate: Option<String>,
    /// `kern.osproductversioncompat`: SystemVersionCompat.plist's version.
    pub compat: Option<String>,
    /// `iOSSupportVersion`.
    pub ios: Option<String>,
    /// `ProductCopyright`'s last year.
    pub year: Option<String>,
    pub build_id: Option<String>,
}

impl Release {
    /// The rows of the table's `text`.
    pub fn parse(text: &str) -> Vec<Release> {
        let known = |f: Option<&str>| f.map(str::trim).filter(|f| !f.is_empty() && *f != "-" && *f != "none").map(String::from);
        text.lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                let (product, build) = (known(f.first().copied())?, known(f.get(1).copied())?);
                Some(Release {
                    product,
                    build,
                    darwin: known(f.get(2).copied()),
                    xnu: known(f.get(3).copied()),
                    kdate: known(f.get(4).copied()),
                    compat: known(f.get(5).copied()),
                    ios: known(f.get(6).copied()),
                    year: known(f.get(7).copied()),
                    build_id: known(f.get(8).copied()),
                })
            })
            .collect()
    }

    /// The row a setting names: a release exactly ("14.8.9"), or the
    /// latest of a release's updates ("14", "14.8": the first row whose
    /// version begins so, the table's order being latest first), or of a
    /// release by its name ("sonoma").
    pub fn find<'a>(rows: &'a [Release], spec: &str) -> Option<&'a Release> {
        let spec = spec.trim().to_ascii_lowercase();
        // Their names, as Apple has called them, for the releases there
        // are rows of.
        const NAMES: [(&str, &str); 7] = [("bigsur", "11"), ("big sur", "11"), ("monterey", "12"), ("ventura", "13"), ("sonoma", "14"), ("sequoia", "15"), ("tahoe", "26")];
        let spec = NAMES.iter().find(|(n, _)| *n == spec).map_or(spec.as_str(), |(_, v)| v);
        let spec = spec.strip_prefix("macos").unwrap_or(spec).trim();
        rows.iter().find(|r| r.product == spec).or_else(|| rows.iter().find(|r| r.product.starts_with(&format!("{spec}."))))
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
        let release = Os::darwin(&product)?;
        let darwin = release.split('.').next().unwrap_or("").to_string();
        let build = value("ProductBuildVersion").unwrap_or_else(|| format!("{darwin}A1"));
        Some(Os { product, build, release, ..Os::default() })
    }

    /// The system a row of the table says, as the environment chose it or
    /// as the root's files name it.
    pub fn from_release(r: &Release, chosen: bool) -> Os {
        let release = r.darwin.clone().or_else(|| Os::darwin(&r.product)).unwrap_or_default();
        Os { product: r.product.clone(), build: r.build.clone(), release, row: Some(r.clone()), chosen }
    }

    /// The Darwin release a macOS version was. Mac OS X 10.x was Darwin
    /// x + 4, its updates Darwin's minor versions; macOS 11 to 13 were
    /// Darwin 20 to 22, each update one past its own number (11.0.1 was
    /// 20.1.0, 12.5 21.6.0, 13.5 22.6.0: xnu's config/MasterVersion at
    /// those releases' tags), and 14 and 15 Darwin 23 and 24, an update its
    /// own number (recalled: xnu publishes no MasterVersion from 14); none
    /// past .6 (recalled, of 11.7, 12.7, 13.7 and 14.7). macOS 26 was Darwin
    /// 25. From 27 the two agree, an update's patch level apart (27.0.1 is
    /// 27.0.0: this Mac).
    pub fn darwin(product: &str) -> Option<String> {
        let mut parts = product.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
        let (major, minor, patch) = (parts.next()?, parts.next().unwrap_or(0), parts.next().unwrap_or(0));
        let (darwin, sub) = match major {
            10 => (minor + 4, patch),
            11..=13 => (major + 9, (minor + 1).min(6)),
            14..=15 => (major + 9, minor.min(6)),
            16..=26 => (major - 1, minor),
            _ => (major, minor),
        };
        Some(format!("{darwin}.{sub}.0"))
    }

    /// `kern.osproductversioncompat`: what the release shows a program
    /// built for 10.15 or before, or nothing (as 27 has).
    pub fn compat(&self) -> &str {
        self.row.as_ref().and_then(|r| r.compat.as_deref()).unwrap_or("")
    }

    /// `kern.iossupportversion`, where the release's row knows it.
    pub fn ios(&self) -> Option<&str> {
        self.row.as_ref().and_then(|r| r.ios.as_deref())
    }

    /// The files that say which system this is, as a Mac has them, for a
    /// process told it is on another release than its root's files say:
    /// (path, contents). SystemVersion.plist with each key the row knows,
    /// in the order a Mac writes them; iOSSystemVersion.plist where the
    /// iOS is known; SystemVersionCompat.plist for a release that had one.
    /// (As pyrus's build.sh writes them into a root, byte for byte.)
    pub fn files(&self) -> Vec<(&'static str, Vec<u8>)> {
        let r = self.row.clone().unwrap_or_default();
        let plist = |name: &str, version: &str| {
            let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n");
            let mut key = |k: &str, v: &str| s.push_str(&format!("\t<key>{k}</key>\n\t<string>{v}</string>\n"));
            if let Some(id) = &r.build_id {
                key("BuildID", id);
            }
            key("ProductBuildVersion", &self.build);
            if let Some(y) = &r.year {
                key("ProductCopyright", &format!("1983-{y} Apple Inc."));
            }
            key("ProductName", name);
            key("ProductUserVisibleVersion", version);
            key("ProductVersion", version);
            if let Some(ios) = &r.ios {
                key("iOSSupportVersion", ios);
            }
            s.push_str("</dict>\n</plist>\n");
            s.into_bytes()
        };
        let mut out = vec![(SYSTEM_VERSION, plist("macOS", &self.product))];
        if let Some(ios) = &r.ios {
            out.push((IOS_VERSION, plist("iPhone OS", ios)));
        }
        if let Some(c) = &r.compat {
            out.push((COMPAT_VERSION, plist("macOS", c)));
        }
        out
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
const KERN_PROC_PGRP: i32 = 2;
const KERN_PROC_SESSION: i32 = 3;
const KERN_PROC_UID: i32 = 5;
const KERN_PROC_RUID: i32 = 6;
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
const SZOMB: u8 = 5;

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
    ("kern.iossupportversion", &[]),
    ("kern.system_version_compat", &[]),
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
    ("hw.targettype", &[]),
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
    ("hw.optional.arm.FEAT_LRCPC2", &[]),
    ("hw.optional.armv8_2_sha512", &[]),
    ("hw.optional.armv8_2_sha3", &[]),
    ("hw.optional.arm.FEAT_AES", &[]),
    ("hw.optional.arm.FEAT_PMULL", &[]),
    ("hw.optional.arm.FEAT_SHA1", &[]),
    ("hw.optional.arm.FEAT_SHA256", &[]),
    ("hw.optional.arm.FEAT_SHA512", &[]),
    ("hw.optional.arm.FEAT_SHA3", &[]),
    ("hw.optional.arm.FEAT_DotProd", &[]),
    ("hw.optional.arm.FEAT_JSCVT", &[]),
    ("hw.optional.arm.FEAT_FlagM", &[]),
    ("hw.optional.arm.FEAT_FlagM2", &[]),
    ("hw.optional.arm.AdvSIMD", &[]),
    ("hw.optional.arm.FP_SyncExceptions", &[]),
    ("hw.perflevel1.physicalcpu", &[]),
    ("hw.perflevel1.physicalcpu_max", &[]),
    ("hw.perflevel1.logicalcpu", &[]),
    ("hw.perflevel1.logicalcpu_max", &[]),
    ("hw.perflevel1.name", &[]),
    ("machdep.cpu.brand_string", &[]),
    ("machdep.cpu.core_count", &[]),
    ("machdep.cpu.thread_count", &[]),
    ("machdep.cpu.cores_per_package", &[]),
    ("machdep.cpu.logical_per_package", &[]),
    ("gex.bottle.name", &[]),
    ("gex.bottle.graphics", &[]),
    ("gex.bottle.serial", &[]),
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
    "hw.optional.armv8_3_compnum",
    "hw.optional.armv8_gpi",
    "hw.optional.ucnormal_mem",
    "hw.optional.arm.FEAT_FHM",
    "hw.optional.arm.FEAT_FP16",
    "hw.optional.arm.FEAT_RDM",
    "hw.optional.arm.FEAT_FCMA",
    "hw.optional.arm.FEAT_LSE2",
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

/// The Mac a process is told it is on: a row of its root's table
/// ([`bottle::MODELS`]), and what its bottle says of itself.
#[derive(Clone, Debug, Default)]
pub struct Mac {
    pub row: bottle::Model,
    /// Chosen, by the environment ([`bottle::MODEL`]) or the root's
    /// configuration, and not the default of the process's macOS. A chosen
    /// Mac has its own processors; the default has the host's, as it had
    /// before a Mac could be chosen.
    pub chosen: bool,
    /// The graphics profile the configuration comes to for this process
    /// ([`bottle::Model::graphics`]). Nothing in the kernel acts on it: it
    /// is told to who asks (`gex.bottle.graphics`).
    pub graphics: String,
}

/// The machine, as `sysctl` and the host port both tell of it.
pub(super) struct Machine {
    pub cpus: i64,
    /// The cores of each kind, where the Mac has two and was chosen.
    pub levels: Option<(i64, i64)>,
    pub cpufamily: u32,
    pub cpusubfamily: u32,
    /// Bytes of memory. As `sysinfo` has it.
    pub mem: i64,
    /// CPU_TYPE_ARM64 with CPU_SUBTYPE_ARM64E, which every Mac with Apple's
    /// processor says; or CPU_TYPE_X86 (an Intel Mac says so without the
    /// 64-bit flag) with CPU_SUBTYPE_X86_64_ALL.
    pub cputype: i64,
    pub cpusubtype: i64,
}

/// CPUFAMILY_INTEL_WESTMERE: what an x86-64 program under Rosetta is told
/// (this Mac, `arch -x86_64 sysctl hw.cpufamily`), its subfamily 0.
const ROSETTA_FAMILY: u32 = 0x573b_5eec;

/// The number `gex.*` names are under: none of XNU's.
const CTL_GEX: i32 = 0x6765;

fn top_of(name: &str) -> i32 {
    match name.split('.').next() {
        Some("kern") => CTL_KERN,
        Some("vm") => CTL_VM,
        Some("hw") => CTL_HW,
        Some("machdep") => 7,
        Some("gex") => CTL_GEX,
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

/// The names at the top: `CTL_KERN` and its kind.
const TOPS: [(i32, &str); 9] = [(1, "kern"), (2, "vm"), (3, "vfs"), (4, "net"), (5, "debug"), (6, "hw"), (7, "machdep"), (8, "user"), (CTL_GEX, "gex")];

/// A `struct kinfo_proc`: a process's id, its parent's and its group's,
/// whether it has ended and waits to be collected, its name (16 bytes at
/// most), its (real, effective, saved) user and group, and its
/// controlling terminal's device and foreground group, if it has one.
#[allow(clippy::too_many_arguments)]
fn kinfo_proc(pid: i32, ppid: i32, pgid: i32, zombie: bool, comm: &str, [ruid, uid, svuid]: [u32; 3], [rgid, gid, svgid]: [u32; 3], tty: Option<(u32, i32)>) -> Vec<u8> {
    const EPROC: usize = 296;
    const P_CONTROLT: u32 = 0x2;
    let mut b = vec![0u8; KINFO_PROC];
    let w32 = |b: &mut [u8], at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    w32(&mut b, 32, P_LP64 | P_EXEC | if tty.is_some() { P_CONTROLT } else { 0 });
    b[36] = if zombie { SZOMB } else { SRUN };
    w32(&mut b, 40, pid as u32);
    // p_priority, then p_comm (16 bytes and a NUL).
    b[240] = 31;
    let comm = &comm.as_bytes()[..comm.len().min(16)];
    b[243..243 + comm.len()].copy_from_slice(comm);
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
    w32(&mut b, EPROC + 264, ppid as u32);
    w32(&mut b, EPROC + 268, pgid as u32);
    // e_tdev and e_tpgid: the terminal, or none.
    w32(&mut b, EPROC + 276, tty.map_or(u32::MAX, |t| t.0));
    w32(&mut b, EPROC + 280, tty.map_or(0, |t| t.1 as u32));
    b
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
    /// The system this process is told it is on, worked out once for each
    /// program it runs: the release its environment chooses ([`CHOSEN`]),
    /// from the table its root has ([`RELEASES`]); else what its root's
    /// [`SYSTEM_VERSION`] says, with what the table knows of that release;
    /// else, with no file, the macOS of the XNU this kernel is.
    pub fn darwin_os(&self) -> &Os {
        self.darwin_os.get_or_init(|| {
            let rows = self.read_file(RELEASES).map(|t| Release::parse(&String::from_utf8_lossy(&t))).unwrap_or_default();
            // The environment's word, then the configuration's: the
            // program's own application's, else its bottle's.
            let env = self.envp.iter().find_map(|e| e.strip_prefix(CHOSEN).and_then(|v| v.strip_prefix('='))).filter(|v| !v.is_empty());
            let conf = self.darwin_conf().setting(&self.exe_path, "macos");
            for (spec, said) in env.map(|s| (s, format!("{CHOSEN}={s}"))).into_iter().chain(conf.map(|s| (s, format!("{}: macos = {s}", bottle::CONF)))) {
                match Release::find(&rows, spec) {
                    Some(r) => return Os::from_release(r, true),
                    None => eprintln!(
                        "[gex:{}] {said}: no such release in {RELEASES}{}; this process is told its root's",
                        self.pid,
                        if rows.is_empty() { " (the root has none: a pyrus root built since it had the table has)".to_string() } else { format!(" (it has {})", rows.iter().map(|r| r.product.as_str()).collect::<Vec<_>>().join(" ")) }
                    ),
                }
            }
            let Some(os) = self.read_file(SYSTEM_VERSION).and_then(|p| Os::from_plist(&p)) else { return Os::default() };
            match rows.iter().find(|r| r.product == os.product && r.build == os.build) {
                Some(r) => Os::from_release(r, false),
                None => os,
            }
        })
    }

    /// The configuration of the root this process is in ([`bottle::CONF`]:
    /// its bottle's, or a plain root's own), read once for each program;
    /// empty where the root has none.
    pub fn darwin_conf(&self) -> &bottle::Conf {
        self.darwin_conf.get_or_init(|| self.read_file(bottle::CONF).map(|t| bottle::Conf::parse(&String::from_utf8_lossy(&t))).unwrap_or_default())
    }

    /// The Mac this process is told it is on, worked out once for each
    /// program it runs: the model its environment chooses
    /// ([`bottle::MODEL`]), else its configuration's (its application's,
    /// else its bottle's), from the table its root has
    /// ([`bottle::MODELS`]); else the first of the table that ran the
    /// macOS the process is told it is on ([`bottle::Model::default_for`]).
    /// An x86-64 program is on an Intel Mac unless its macOS ran on none
    /// ([`rosetta`]): then on the Mac an arm64 program is, translated. A
    /// model of the other processor's is not this process's to be told of,
    /// and it has the default.
    pub fn darwin_mac(&self) -> &Mac {
        self.darwin_mac.get_or_init(|| {
            let os = self.darwin_os();
            let conf = self.darwin_conf();
            let rows = self.read_file(bottle::MODELS).map(|t| bottle::Model::parse(&String::from_utf8_lossy(&t))).filter(|r| !r.is_empty());
            let rows = rows.unwrap_or_else(|| bottle::Model::parse(bottle::BUILTIN));
            let arch = if gex_abi::X86 && !rosetta(&os.product) { "x86_64" } else { "arm64" };
            let env = self.envp.iter().find_map(|e| e.strip_prefix(bottle::MODEL).and_then(|v| v.strip_prefix('='))).filter(|v| !v.is_empty());
            let mut chosen = None;
            for spec in env.into_iter().chain(conf.setting(&self.exe_path, "machine")) {
                match bottle::Model::find(&rows, spec) {
                    Some(m) if m.arch == arch => {
                        chosen = Some(m.clone());
                        break;
                    }
                    Some(_) => {}
                    None => eprintln!("[gex:{}] no Mac {spec} in {} (it has {}); this process is told its macOS's default", self.pid, bottle::MODELS, rows.iter().map(|m| m.model.as_str()).collect::<Vec<_>>().join(" ")),
                }
            }
            let row = chosen.clone().or_else(|| bottle::Model::default_for(&rows, arch, &os.product).cloned());
            let row = row.or_else(|| bottle::Model::default_for(&bottle::Model::parse(bottle::BUILTIN), arch, &os.product).cloned()).unwrap_or_default();
            let graphics = row.graphics(conf.setting(&self.exe_path, "graphics"));
            Mac { row, chosen: chosen.is_some(), graphics }
        })
    }

    /// The machine's numbers, as `sysctl`, the host port and the comm page
    /// tell them: the processors of the Mac this process was told to be on
    /// (the host's, for the default Mac), its processor's family (under
    /// Rosetta, the one Rosetta says; CPUFAMILY_UNKNOWN and
    /// CPUSUBFAMILY_UNKNOWN, 0, where the Mac's row does not know), and the
    /// memory gex gives a process whatever the Mac had.
    pub(super) fn darwin_machine(&self) -> Machine {
        let x86 = gex_abi::X86;
        let mac = self.darwin_mac();
        let translated = x86 && mac.row.arch != "x86_64";
        let own = mac.chosen.then_some(()).and_then(|()| Some((mac.row.pcores? as i64, mac.row.ecores.unwrap_or(0) as i64, mac.row.threads)));
        let (cpus, levels) = match own {
            Some((p, e, threads)) => (threads.map_or(p + e, |t| t as i64).max(1), (e > 0).then_some((p, e))),
            None => (crate::cpus::count() as i64, None),
        };
        Machine {
            cpus,
            levels,
            cpufamily: if translated { ROSETTA_FAMILY } else { mac.row.cpufamily.unwrap_or(0) },
            cpusubfamily: if translated { 0 } else { mac.row.cpusubfamily.unwrap_or(0) },
            mem: 4 << 30,
            cputype: if x86 { 7 } else { 0x0100_000c },
            cpusubtype: if x86 { 3 } else { 2 },
        }
    }

    /// What this process is given for the file at `path` in place of the
    /// root's, if it is one of the files that say which system this is and
    /// the process was told it is another ([`Os::files`]): its contents, or
    /// `Some(None)` for one that release has not (none of the root's is
    /// seen through).
    pub fn darwin_version_file(&self, path: &str) -> Option<Option<Vec<u8>>> {
        if !path.ends_with("Version.plist") && !path.ends_with("VersionCompat.plist") {
            return None;
        }
        let path = crate::vfs::normalize(path, &self.caller_cwd());
        if ![SYSTEM_VERSION, IOS_VERSION, COMPAT_VERSION].contains(&path.as_str()) {
            return None;
        }
        let os = self.darwin_os();
        if !os.chosen {
            return None;
        }
        Some(os.files().into_iter().find(|(p, _)| *p == path).map(|(_, b)| b))
    }

    /// The value of the name `name`, if this kernel has one.
    fn darwin_sysctl_value(&self, name: &str) -> Option<Val> {
        use Val::*;
        let x86 = gex_abi::X86;
        let Machine { cpus, levels, cpufamily, cpusubfamily, mem, cputype, cpusubtype } = self.darwin_machine();
        let mac = self.darwin_mac();
        // The Mac an x86-64 program is on: an Intel one, unless the
        // release ran on none.
        let intel = x86 && !rosetta(&self.darwin_os().product);
        // The performance cores, where the efficiency ones are apart.
        let fast = levels.map_or(cpus, |(p, _)| p);
        let page = abi::PAGE_SIZE as i64;
        let nofile = self.rlimits.get()[RLIMIT_NOFILE];
        Some(match name {
            "kern.ostype" => Str("Darwin".into()),
            "kern.osrelease" => Str(self.darwin_os().release.clone()),
            "kern.osrevision" => Int(199506),
            "kern.version" => Str(darwin_version(self.darwin_os(), mac.row.soc.as_deref())),
            "kern.osversion" => Str(self.darwin_os().build.clone()),
            // (The version a program built for 10.15 or before is shown,
            // to a task that has asked for it, where the release has one:
            // xnu's sysctl_osproductversion.)
            "kern.osproductversion" if self.darwin_compat.load(std::sync::atomic::Ordering::Relaxed) && !self.darwin_os().compat().is_empty() => Str(self.darwin_os().compat().into()),
            "kern.osproductversion" => Str(self.darwin_os().product.clone()),
            // (As xnu reads it: the task's flag as it is kept,
            // TF_SYS_VERSION_COMPAT, 0x8000, which is what a Mac says.)
            "kern.system_version_compat" => Int(if self.darwin_compat.load(std::sync::atomic::Ordering::Relaxed) { 0x8000 } else { 0 }),
            "kern.osproductversioncompat" => Str(self.darwin_os().compat().into()),
            "kern.iossupportversion" => Str(self.darwin_os().ios()?.into()),
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
            // The Mac's own names, where its row knows them (one it does
            // not know is a name this Mac has not).
            "hw.model" | "hw.product" => Str(mac.row.model.clone()),
            "hw.target" => Str(mac.row.target.clone()?),
            // The board without the "AP" (as a Mac's J616sAP is J616s).
            "hw.targettype" if !intel => Str(mac.row.targettype.clone()?),
            "machdep.cpu.brand_string" => Str(mac.row.brand.clone()?),
            "hw.ncpu" | "hw.availcpu" | "hw.activecpu" | "hw.logicalcpu" | "hw.logicalcpu_max" | "machdep.cpu.thread_count" | "machdep.cpu.logical_per_package" => Int(cpus),
            // (An Intel Mac's two threads a core are one processor each.)
            "hw.physicalcpu" | "hw.physicalcpu_max" | "machdep.cpu.core_count" | "machdep.cpu.cores_per_package" => Int(if mac.chosen && intel { mac.row.pcores.map_or(cpus, |p| p as i64) } else { cpus }),
            "hw.perflevel0.physicalcpu" | "hw.perflevel0.physicalcpu_max" | "hw.perflevel0.logicalcpu" | "hw.perflevel0.logicalcpu_max" => Int(fast),
            "hw.perflevel0.name" => Str("Performance".into()),
            "hw.perflevel1.physicalcpu" | "hw.perflevel1.physicalcpu_max" | "hw.perflevel1.logicalcpu" | "hw.perflevel1.logicalcpu_max" => Int(levels?.1),
            "hw.perflevel1.name" => Str(levels.map(|_| "Efficiency".to_string())?),
            "hw.nperflevels" => Int(if levels.is_some() { 2 } else { 1 }),
            "hw.packages" => Int(1),
            // What the root's configuration says of itself, for who is to
            // act on it (a graphics layer, an IOKit): the bottle's name,
            // the profile this process's settings come to, the serial
            // number made with the bottle.
            "gex.bottle.name" => Str(self.darwin_conf().get("name").unwrap_or("").into()),
            "gex.bottle.graphics" => Str(mac.graphics.clone()),
            "gex.bottle.serial" => Str(self.darwin_conf().get("serial").unwrap_or("").into()),
            "hw.byteorder" => Int(1234),
            // An `int`, so no more than it holds.
            "hw.physmem" | "hw.usermem" => Int(mem.min(i32::MAX as i64)),
            "hw.memsize" | "hw.memsize_usable" => Quad(mem),
            // The family is the Mac's (an M2's is Blizzard and Avalanche).
            "hw.cputype" => Int(cputype),
            "hw.cpusubtype" => Int(cpusubtype),
            "hw.cpu64bit_capable" => Int(1),
            "hw.cpufamily" => Int(cpufamily as i64),
            "hw.cpusubfamily" => Int(cpusubfamily as i64),
            "hw.cachelinesize" => Quad(if x86 { 64 } else { 128 }),
            "hw.l1icachesize" => Quad(if x86 { 32 << 10 } else { 128 << 10 }),
            "hw.l1dcachesize" => Quad(if x86 { 32 << 10 } else { 64 << 10 }),
            "hw.l2cachesize" => Quad(if x86 { 256 << 10 } else { 4 << 20 }),
            // `mach_absolute_time` counts nanoseconds ([`Kernel::darwin_abstime`]).
            "hw.tbfrequency" => Quad(1_000_000_000),
            "hw.optional.floatingpoint" => Int(1),
            "hw.optional.neon" | "hw.optional.AdvSIMD" | "hw.optional.arm64" | "hw.optional.armv8_1_atomics" | "hw.optional.armv8_crc32" if !x86 => Int(1),
            "hw.optional.arm.FEAT_LSE" | "hw.optional.arm.FEAT_CRC32" | "hw.optional.arm.FEAT_LRCPC" | "hw.optional.arm.FEAT_LRCPC2" | "hw.optional.arm.AdvSIMD" | "hw.optional.arm.FP_SyncExceptions" if !x86 => Int(1),
            // What the translator carries out of the rest of an Apple processor's set: AES,
            // the polynomial multiply, SHA-1, SHA-2 and SHA-3, the dot products, JavaScript's
            // conversion and the flag manipulations (`gex-isa`'s decoder and its slow path).
            "hw.optional.armv8_2_sha512" | "hw.optional.armv8_2_sha3" | "hw.optional.arm.FEAT_AES" | "hw.optional.arm.FEAT_PMULL" | "hw.optional.arm.FEAT_SHA1" | "hw.optional.arm.FEAT_SHA256" | "hw.optional.arm.FEAT_SHA512" | "hw.optional.arm.FEAT_SHA3" | "hw.optional.arm.FEAT_DotProd" | "hw.optional.arm.FEAT_JSCVT" | "hw.optional.arm.FEAT_FlagM" | "hw.optional.arm.FEAT_FlagM2" if !x86 => Int(1),
            "user.cs_path" => Str("/usr/bin:/bin:/usr/sbin:/sbin".into()),
            // Under Rosetta, or not: an x86-64 program on a macOS that
            // runs on no Intel Mac is translated ([`rosetta`]).
            "sysctl.proc_translated" if !intel => Int((x86 && !intel) as i64),
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
            // sysctl.name and sysctl.oidfmt: the name the numbers after
            // them number, and what kind of value it is (`CTLTYPE_*` with
            // that it may be read, then the letter `sysctl` prints it by),
            // which is what the `sysctl` command asks of every name first.
            // (A number nothing has, under a name that is one, is named
            // by the number.)
            [0, 1, ref rest @ ..] => {
                let name = match (name_of(rest), rest) {
                    (Some(name), _) => name.to_string(),
                    (None, [top, n]) => format!("{}.{n}", TOPS.iter().find(|(t, _)| t == top).ok_or(Errno::ENOENT)?.1),
                    _ => return Err(Errno::ENOENT),
                };
                self.darwin_sysctl_out(m, old, oldlenp, Val::Str(name))
            }
            [0, 4, ref rest @ ..] => {
                let name = name_of(rest).ok_or(Errno::ENOENT)?;
                const CTLFLAG_RD: u32 = 0x8000_0000;
                let (kind, letter): (u32, &[u8]) = match self.darwin_sysctl_value(name).ok_or(Errno::ENOENT)? {
                    Val::Int(_) => (2, b"I"),
                    Val::Str(_) => (3, b"A"),
                    Val::Quad(_) => (4, b"Q"),
                    Val::Bytes(_) => (5, b"S"),
                };
                let mut out = (kind | CTLFLAG_RD).to_le_bytes().to_vec();
                out.extend_from_slice(letter);
                out.push(0);
                self.darwin_sysctl_out(m, old, oldlenp, Val::Bytes(out))
            }
            // sysctl.name2oid: the numbers of the name in `new`.
            [0, 3] => {
                let text = m.read(new, (newlen as usize).min(256))?.to_vec();
                let text = String::from_utf8_lossy(&text);
                let text = text.trim_end_matches('\0');
                let numbers = oid_of(text).ok_or(Errno::ENOENT)?;
                self.darwin_sysctl_out(m, old, oldlenp, Val::Bytes(numbers.iter().flat_map(|v| v.to_le_bytes()).collect()))
            }
            [CTL_KERN, KERN_PROC, what, ..] => {
                let procs = self.darwin_kinfo_procs(what, oid.get(3).copied().unwrap_or(0));
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
                let counted = oid[1] == KERN_PROCARGS2;
                let args = if pid == self.pid {
                    self.darwin_procargs(counted)
                } else {
                    // Another process's, as it last published them: its
                    // arguments (the first for its file's name, which is
                    // not kept apart), and none of its environment.
                    let e = self.procs.entries().into_iter().find(|e| e.pid == pid).ok_or(Errno::EINVAL)?;
                    let argv: Vec<&[u8]> = e.cmdline().split(|b| *b == 0).filter(|a| !a.is_empty()).collect();
                    let mut b = Vec::new();
                    if counted {
                        b.extend_from_slice(&(argv.len() as u32).to_le_bytes());
                    }
                    b.extend_from_slice(argv.first().copied().unwrap_or(e.comm().as_bytes()));
                    b.push(0);
                    for a in argv {
                        b.extend_from_slice(a);
                        b.push(0);
                    }
                    b
                };
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
        // The one name a task sets: whether it is shown the compatibility
        // version (by anybody: CTLFLAG_ANYBODY), the old value answered.
        if name == "kern.system_version_compat" && new != 0 {
            let was = self.darwin_sysctl_value(name);
            if old != 0 || oldlenp != 0 {
                self.darwin_sysctl_out(m, old, oldlenp, was.unwrap_or(Val::Int(0)))?;
            }
            self.darwin_compat.store(m.read_u32(new)? != 0, std::sync::atomic::Ordering::Relaxed);
            return Ok(0);
        }
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
    /// `KERN_PROC`: a `struct kinfo_proc` for each process the selector
    /// `what` picks with `arg` -- all of them, one by its id, a process
    /// group's or a session's, a user's by the effective id or the real
    /// one. (Nothing has a terminal to be picked by.) This process's is
    /// its own kernel's to say; another's is what the table every process
    /// publishes itself in has of it: its name, its ids and its kin.
    fn darwin_kinfo_procs(&self, what: i32, arg: i32) -> Vec<u8> {
        let mut out = Vec::new();
        let mut entries = self.procs.entries();
        entries.sort_by_key(|e| e.pid);
        // (A process alone, the table not asked: at least itself.)
        if !entries.iter().any(|e| e.pid == self.pid) {
            entries.push(crate::proc::ProcEntry { pid: self.pid, ppid: self.procs.ppid(self.pid), pgid: self.procs.pgid(self.pid, 0).unwrap_or(self.pid), sid: self.procs.sid(self.pid, 0).unwrap_or(self.pid), ..Default::default() });
        }
        for e in entries {
            let ids = |at: usize| e.creds.get(4 * at..4 * at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
            let own = e.pid == self.pid;
            // (real, effective, saved) of the user and of the group.
            let uids = if own { self.creds.uids() } else { [ids(0), ids(1), ids(2)].map(|v| v.unwrap_or(self.creds.ruid())) };
            let gids = if own { self.creds.gids() } else { [ids(3), ids(4), ids(5)].map(|v| v.unwrap_or(self.creds.rgid())) };
            let picked = match what {
                KERN_PROC_ALL => true,
                KERN_PROC_PID => e.pid == arg,
                KERN_PROC_PGRP => e.pgid == arg,
                KERN_PROC_SESSION => e.sid == arg,
                KERN_PROC_UID => uids[1] == arg as u32,
                KERN_PROC_RUID => uids[0] == arg as u32,
                _ => false,
            };
            if picked {
                let comm = if own { self.darwin_comm() } else { e.comm() };
                // The terminal its session has claimed, and that
                // terminal's foreground group, where terminals are kept
                // (which `ps` shows a process by, unless asked for all).
                let tty = self.ptys.and_then(|r| {
                    let id = (r.for_sid)(e.sid);
                    (id >= 0).then(|| (abi::dev(crate::pty::pts_rdev(id as u32)), (r.fg)(id)))
                });
                out.extend_from_slice(&kinfo_proc(e.pid, e.ppid, e.pgid, e.zombie, &comm, uids, gids, tty));
            }
        }
        out
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

/// Whether macOS `product` ran on no Intel Mac, so that an x86-64 program
/// on it is one Rosetta translates: 27 and later (the last for Intel was
/// 26; recalled).
pub fn rosetta(product: &str) -> bool {
    product.split('.').next().and_then(|m| m.parse::<u32>().ok()).is_some_and(|m| m >= 27)
}

/// `kern.version`, as `uname -v` prints it: the release's kernel by its
/// name and the date it was built, where the table knows them (else this
/// one's, [`XNU`], and the word "gex" where the date is), and the machine
/// it was built for: the chip `soc` of the Mac the process is told it is
/// on ([`Mac`]), or an Intel Mac's (under Rosetta, the chip's still).
pub fn darwin_version(os: &Os, soc: Option<&str>) -> String {
    let row = os.row.as_ref();
    let soc = if gex_abi::X86 && !rosetta(&os.product) { "X86_64".to_string() } else { soc.map_or("ARM64".to_string(), |s| format!("ARM64_{s}")) };
    let date = row.and_then(|r| r.kdate.as_deref()).unwrap_or("gex");
    let xnu = row.and_then(|r| r.xnu.as_deref()).unwrap_or(XNU);
    format!("Darwin Kernel Version {}: {date}; root:{xnu}/RELEASE_{soc}", os.release)
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
        assert_eq!(os("10.6.8", "10K549"), Os { product: "10.6.8".into(), build: "10K549".into(), release: "10.8.0".into(), ..Os::default() });
        assert_eq!(os("10.14.6", "18G103").release, "18.6.0");
        // (xnu's config/MasterVersion at 7195.50.7.100.1, 7195.141.2,
        // 8020.140.41, 8792.41.9.)
        assert_eq!(os("11.0.1", "20B29").release, "20.1.0");
        assert_eq!(os("11.5", "20G71").release, "20.6.0");
        assert_eq!(os("11.6", "20G165").release, "20.6.0");
        assert_eq!(os("12.5", "21G72").release, "21.6.0");
        assert_eq!(os("13.0", "22A380").release, "22.1.0");
        assert_eq!(os("15.1", "24B83").release, "24.1.0");
        assert_eq!(os("26.0", "25A354").release, "25.0.0");
        assert_eq!(os("27.0.1", "26A434").release, "27.0.0");
        assert_eq!(Os::from_plist(b"not a plist"), None);
    }

    const TABLE: &str = "# a comment\n#product\tbuild\tdarwin\txnu\tkdate\tcompat\tios\tyear\tbuildid\tcf\tfoundation\tappkit\tsource\n\
        27.0.1\t26A434\t27.0.0\txnu-13432.1.9~1\tTue Aug 11 21:05:48 PDT 2026\tnone\t27.0\t2026\t0B27979E-B7E7-11F1-B3D2-24EC53F49301\t5027.0\t5027.0\t2775.1\tthis Mac\n\
        27.0\t26A425\t-\txnu-13432\t-\tnone\t27.0\t2026\tF8088D3E-A1DA-11F1-AAD4-A82B6E44CE4F\t5027.0\t5027.0\t2775.1\tthe SDK\n\
        14.8.9\t23J631\t-\t-\t-\t10.16\t-\t-\t-\t-\t-\t-\tthe catalog\n\
        14.6\t23G80\t-\t-\t-\t10.16\t-\t-\t-\t-\t-\t-\tan older one\n";

    #[test]
    fn a_release_is_chosen_by_its_number_or_its_name() {
        let rows = Release::parse(TABLE);
        assert_eq!(rows.len(), 4);
        let find = |s: &str| Release::find(&rows, s).map(|r| r.product.as_str());
        assert_eq!(find("27.0"), Some("27.0"));
        assert_eq!(find("27"), Some("27.0.1"));
        assert_eq!(find("14"), Some("14.8.9"));
        assert_eq!(find("14.6"), Some("14.6"));
        assert_eq!(find("Sonoma"), Some("14.8.9"));
        assert_eq!(find("macos14"), Some("14.8.9"));
        assert_eq!(find("13"), None);
        assert_eq!(find("2"), None);
        let os = Os::from_release(Release::find(&rows, "14").unwrap(), true);
        assert_eq!((os.product.as_str(), os.build.as_str(), os.release.as_str(), os.compat(), os.ios()), ("14.8.9", "23J631", "23.6.0", "10.16", None));
        let os = Os::from_release(Release::find(&rows, "27.0.1").unwrap(), false);
        assert_eq!(os.compat(), "");
        assert_eq!(os.ios(), Some("27.0"));
    }

    #[test]
    fn the_kernel_is_named_as_the_release_says() {
        let rows = Release::parse(TABLE);
        let v = |s: &str| darwin_version(&Os::from_release(Release::find(&rows, s).unwrap(), true), Some("T8112"));
        // (27 on either processor: an x86-64 program on it is under Rosetta.)
        assert_eq!(v("27.0.1"), "Darwin Kernel Version 27.0.0: Tue Aug 11 21:05:48 PDT 2026; root:xnu-13432.1.9~1/RELEASE_ARM64_T8112");
        assert_eq!(v("27.0"), "Darwin Kernel Version 27.0.0: gex; root:xnu-13432/RELEASE_ARM64_T8112");
        let intel = if gex_abi::X86 { "X86_64" } else { "ARM64_T8112" };
        assert_eq!(v("14"), format!("Darwin Kernel Version 23.6.0: gex; root:{XNU}/RELEASE_{intel}"));
    }

    #[test]
    fn a_chosen_release_has_the_files_a_mac_of_it_has() {
        let rows = Release::parse(TABLE);
        let os = Os::from_release(Release::find(&rows, "27.0").unwrap(), true);
        let files = os.files();
        assert_eq!(files.iter().map(|f| f.0).collect::<Vec<_>>(), [SYSTEM_VERSION, IOS_VERSION]);
        // As the SDK's own SystemVersion.plist is, byte for byte.
        let want = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\
            \t<key>BuildID</key>\n\t<string>F8088D3E-A1DA-11F1-AAD4-A82B6E44CE4F</string>\n\t<key>ProductBuildVersion</key>\n\t<string>26A425</string>\n\
            \t<key>ProductCopyright</key>\n\t<string>1983-2026 Apple Inc.</string>\n\t<key>ProductName</key>\n\t<string>macOS</string>\n\
            \t<key>ProductUserVisibleVersion</key>\n\t<string>27.0</string>\n\t<key>ProductVersion</key>\n\t<string>27.0</string>\n\
            \t<key>iOSSupportVersion</key>\n\t<string>27.0</string>\n</dict>\n</plist>\n";
        assert_eq!(String::from_utf8_lossy(&files[0].1), want);
        assert_eq!(Os::from_plist(&files[0].1).map(|o| (o.product, o.build)), Some(("27.0".into(), "26A425".into())));
        let os = Os::from_release(Release::find(&rows, "14").unwrap(), true);
        let files = os.files();
        assert_eq!(files.iter().map(|f| f.0).collect::<Vec<_>>(), [SYSTEM_VERSION, COMPAT_VERSION]);
        assert!(String::from_utf8_lossy(&files[1].1).contains("<key>ProductVersion</key>\n\t<string>10.16</string>"));
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
