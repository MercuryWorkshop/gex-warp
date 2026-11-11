//! Mach-O images, loaded as XNU's exec loads them.
//!
//! What `bsd/kern/mach_loader.c` does for a process, in the terms the rest
//! of gex already has: a thin 64-bit image (or the slice of a universal file
//! that is this guest's) becomes an [`Image`] whose segments are the file's
//! `LC_SEGMENT_64` commands, slid when the image is position-independent; a
//! dynamic executable brings its dynamic linker (`LC_LOAD_DYLINKER`,
//! `/usr/lib/dyld`) with it, placed as an ELF interpreter is, and the process
//! starts in the linker. The linker does every fixup itself -- the kernel
//! applies none -- so nothing here reads the fixup chains or the bind
//! opcodes.
//!
//! [`build_stack`] lays out the stack XNU's `exec_copyout_strings` hands a
//! new process: `argc`, `argv`, `envp` and the `apple` vector, with the main
//! executable's header address below them for a program its linker starts.

use crate::dynamic::Program;
use crate::elf::{Image, LoadError, Segment, Symbol};
use crate::stack::StackLayout;

pub const MH_MAGIC_64: u32 = 0xfeed_facf;
/// A universal file's magic, as its big-endian header reads.
const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_MAGIC_64: u32 = 0xcafe_babf;

pub const CPU_TYPE_X86_64: u32 = 0x0100_0007;
/// `CPU_SUBTYPE_X86_64_H`.
pub const CPU_SUBTYPE_X86_64_H: u32 = 8;
pub const CPU_TYPE_ARM64: u32 = 0x0100_000c;
/// The `cputype` of an image this build's guest runs.
pub const CPU_TYPE_GUEST: u32 = if gex_abi::X86 { CPU_TYPE_X86_64 } else { CPU_TYPE_ARM64 };
/// The capability bits of a `cpusubtype` (arm64e's pointer-authentication
/// ABI version lives there).
const CPU_SUBTYPE_MASK: u32 = 0xff00_0000;
pub const CPU_SUBTYPE_ARM64E: u32 = 2;

pub const MH_EXECUTE: u32 = 2;
pub const MH_DYLIB: u32 = 6;
pub const MH_DYLINKER: u32 = 7;
pub const MH_BUNDLE: u32 = 8;
const MH_PIE: u32 = 0x20_0000;

const LC_REQ_DYLD: u32 = 0x8000_0000;
const LC_SYMTAB: u32 = 0x2;
const LC_UNIXTHREAD: u32 = 0x5;
const LC_LOAD_DYLIB: u32 = 0xc;
const LC_ID_DYLIB: u32 = 0xd;
const LC_LOAD_DYLINKER: u32 = 0xe;
const LC_LOAD_WEAK_DYLIB: u32 = 0x18 | LC_REQ_DYLD;
const LC_SEGMENT_64: u32 = 0x19;
const LC_UUID: u32 = 0x1b;
const LC_REEXPORT_DYLIB: u32 = 0x1f | LC_REQ_DYLD;
const LC_FUNCTION_STARTS: u32 = 0x26;
const LC_MAIN: u32 = 0x28 | LC_REQ_DYLD;
const LC_LOAD_UPWARD_DYLIB: u32 = 0x23 | LC_REQ_DYLD;

const VM_PROT_READ: u32 = 1;
const VM_PROT_WRITE: u32 = 2;
const VM_PROT_EXECUTE: u32 = 4;

/// A section holding only instructions (`__text`).
const S_ATTR_PURE_INSTRUCTIONS: u32 = 0x8000_0000;
/// A section holding some (`__stubs`, `__stub_helper`).
const S_ATTR_SOME_INSTRUCTIONS: u32 = 0x0000_0400;

const N_STAB: u8 = 0xe0;
const N_TYPE: u8 = 0x0e;
const N_SECT: u8 = 0x0e;

/// The page size a Mach-O image of this guest is laid out for: 16 KiB on
/// arm64, 4 KiB on x86-64.
pub const PAGE_SIZE: u64 = if gex_abi::X86 { 0x1000 } else { 0x4000 };

/// Whether `bytes` begin a Mach-O file gex may be asked to run: a thin
/// 64-bit little-endian image, or a universal file. (A Java class file
/// shares the universal magic; its "architecture count" is a version number
/// in the thousands, a universal file's a handful.)
pub fn is_macho(bytes: &[u8]) -> bool {
    let Some(head) = bytes.get(..8) else { return false };
    let le = u32::from_le_bytes(head[..4].try_into().unwrap());
    if le == MH_MAGIC_64 {
        return true;
    }
    let be = u32::from_be_bytes(head[..4].try_into().unwrap());
    let n = u32::from_be_bytes(head[4..8].try_into().unwrap());
    (be == FAT_MAGIC || be == FAT_MAGIC_64) && (1..=32).contains(&n)
}

fn bad(what: &'static str) -> LoadError {
    LoadError::Malformed(what)
}

struct Rd<'a>(&'a [u8]);

impl<'a> Rd<'a> {
    fn bytes(&self, off: u64, len: u64) -> Result<&'a [u8], LoadError> {
        let end = off.checked_add(len).ok_or(LoadError::TooShort)?;
        self.0.get(off as usize..end as usize).ok_or(LoadError::TooShort)
    }
    fn u32(&self, off: u64) -> Result<u32, LoadError> {
        Ok(u32::from_le_bytes(self.bytes(off, 4)?.try_into().unwrap()))
    }
    fn u64(&self, off: u64) -> Result<u64, LoadError> {
        Ok(u64::from_le_bytes(self.bytes(off, 8)?.try_into().unwrap()))
    }
    /// A fixed 16-byte name, up to its first NUL.
    fn name16(&self, off: u64) -> Result<String, LoadError> {
        let b = self.bytes(off, 16)?;
        let n = b.iter().position(|c| *c == 0).unwrap_or(16);
        Ok(String::from_utf8_lossy(&b[..n]).into_owned())
    }
    /// A NUL-terminated string within `[off, end)`.
    fn cstr(&self, off: u64, end: u64) -> Result<String, LoadError> {
        if off >= end {
            return Err(bad("a string outside its load command"));
        }
        let b = self.bytes(off, end - off)?;
        let n = b.iter().position(|c| *c == 0).unwrap_or(b.len());
        Ok(String::from_utf8_lossy(&b[..n]).into_owned())
    }
}

/// The slice of `bytes` that is this guest's image: all of a thin file, the
/// matching architecture of a universal one, as `(offset, length)`.
///
/// Of two arm64 slices the plain one is preferred to arm64e: third-party
/// programs are arm64, and an arm64e image's pointer authentication is
/// something gex only ignores. (XNU prefers arm64e where the binary may use
/// it, which is Apple's own.)
pub fn guest_slice(bytes: &[u8]) -> Result<(usize, usize), LoadError> {
    guest_slice_in(bytes, bytes.len() as u64)
}

/// [`guest_slice`] of a file `file_len` bytes long whose first bytes are
/// `bytes` (all of it, or enough to hold a universal file's table).
fn guest_slice_in(bytes: &[u8], file_len: u64) -> Result<(usize, usize), LoadError> {
    let head = bytes.get(..8).ok_or(LoadError::TooShort)?;
    if u32::from_le_bytes(head[..4].try_into().unwrap()) == MH_MAGIC_64 {
        return Ok((0, file_len as usize));
    }
    let magic = u32::from_be_bytes(head[..4].try_into().unwrap());
    if magic != FAT_MAGIC && magic != FAT_MAGIC_64 {
        return Err(bad("not a Mach-O file"));
    }
    let n = u32::from_be_bytes(head[4..8].try_into().unwrap()) as usize;
    let wide = magic == FAT_MAGIC_64;
    let entry = if wide { 32 } else { 20 };
    let be32 = |o: usize| bytes.get(o..o + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()));
    let be64 = |o: usize| bytes.get(o..o + 8).map(|b| u64::from_be_bytes(b.try_into().unwrap()));
    let mut best: Option<(u32, usize, usize)> = None;
    for i in 0..n {
        let at = 8 + i * entry;
        let (Some(cputype), Some(sub)) = (be32(at), be32(at + 4)) else {
            return Err(LoadError::TooShort);
        };
        let (off, len) = if wide {
            (be64(at + 8).ok_or(LoadError::TooShort)?, be64(at + 16).ok_or(LoadError::TooShort)?)
        } else {
            (be32(at + 8).ok_or(LoadError::TooShort)? as u64, be32(at + 12).ok_or(LoadError::TooShort)? as u64)
        };
        if cputype != CPU_TYPE_GUEST {
            continue;
        }
        if off.checked_add(len).is_none_or(|e| e > file_len) {
            return Err(bad("a universal file's slice lies outside the file"));
        }
        // Lower is better: the plain subtype, then arm64e (or x86_64h, the
        // slice for Haswell and later, which may use what the plain one
        // does not), then its variants.
        let rank = match (sub & !CPU_SUBTYPE_MASK, sub & CPU_SUBTYPE_MASK) {
            (CPU_SUBTYPE_ARM64E, 0) => 1,
            (CPU_SUBTYPE_ARM64E, _) => 2,
            (CPU_SUBTYPE_X86_64_H, _) => 1,
            (0..=1, _) | (3, _) => 0,
            _ => 3,
        };
        if best.is_none_or(|(r, _, _)| rank < r) {
            best = Some((rank, off as usize, len as usize));
        }
    }
    best.map(|(_, o, l)| (o, l)).ok_or(LoadError::NotAarch64)
}

/// One `LC_SEGMENT_64`.
#[derive(Clone, Debug)]
pub struct MachSegment {
    pub name: String,
    pub vmaddr: u64,
    pub vmsize: u64,
    pub fileoff: u64,
    pub filesize: u64,
    pub maxprot: u32,
    pub initprot: u32,
    pub sections: Vec<MachSection>,
}

impl MachSegment {
    /// `__PAGEZERO`: address space the image wants left unmapped below it,
    /// as XNU recognises it (nothing of the file, no access, from zero).
    pub fn is_pagezero(&self) -> bool {
        self.vmaddr == 0 && self.filesize == 0 && self.vmsize != 0 && self.initprot == 0 && self.maxprot == 0
    }
}

#[derive(Clone, Debug)]
pub struct MachSection {
    pub name: String,
    pub addr: u64,
    pub size: u64,
    pub flags: u32,
}

/// Where a Mach-O image says it starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// `LC_UNIXTHREAD`: the initial program counter (a static executable's,
    /// the dynamic linker's).
    Thread { pc: u64 },
    /// `LC_MAIN`: `main`'s offset from the image's header, for the dynamic
    /// linker to call, and the main thread's stack size (0: the default).
    Main { entryoff: u64, stacksize: u64 },
}

/// A parsed Mach-O image: its load commands, nothing placed.
#[derive(Clone, Debug)]
pub struct MachO {
    pub cpusubtype: u32,
    pub filetype: u32,
    pub flags: u32,
    pub segments: Vec<MachSegment>,
    pub entry: Option<Entry>,
    /// `LC_LOAD_DYLINKER`.
    pub dylinker: Option<String>,
    /// `LC_ID_DYLIB`: a library's install name.
    pub id: Option<String>,
    /// The libraries it loads, in order.
    pub dylibs: Vec<String>,
    pub uuid: Option<[u8; 16]>,
    /// `(symoff, nsyms, stroff, strsize)`.
    symtab: Option<(u64, u64, u64, u64)>,
    /// `(dataoff, datasize)` of `LC_FUNCTION_STARTS`.
    function_starts: Option<(u64, u64)>,
}

impl MachO {
    pub fn pie(&self) -> bool {
        self.flags & MH_PIE != 0 || self.filetype == MH_DYLINKER || self.filetype == MH_DYLIB || self.filetype == MH_BUNDLE
    }

    /// arm64e: its code signs and authenticates pointers.
    pub fn arm64e(&self) -> bool {
        !gex_abi::X86 && self.cpusubtype & !CPU_SUBTYPE_MASK == CPU_SUBTYPE_ARM64E
    }

    /// The segments that occupy memory (every one but `__PAGEZERO`).
    pub fn mapped(&self) -> impl Iterator<Item = &MachSegment> {
        self.segments.iter().filter(|s| !s.is_pagezero() && s.vmsize != 0)
    }

    /// The unslid address of the image's header: the segment that maps the
    /// start of the file.
    pub fn header_vmaddr(&self) -> Option<u64> {
        self.mapped().find(|s| s.fileoff == 0 && s.filesize != 0).map(|s| s.vmaddr)
    }

    /// The unslid `[lowest, highest)` the mapped segments span.
    pub fn span(&self) -> Option<(u64, u64)> {
        let lo = self.mapped().map(|s| s.vmaddr).min()?;
        let hi = self.mapped().map(|s| s.vmaddr.saturating_add(s.vmsize)).max()?;
        Some((lo, hi))
    }
}

/// Parse one thin image (a [`guest_slice`]).
pub fn parse(data: &[u8]) -> Result<MachO, LoadError> {
    parse_head(data, data.len() as u64)
}

/// [`parse`] from the image's header and load commands alone (`data`), the
/// image itself `len` bytes long.
fn parse_head(data: &[u8], len: u64) -> Result<MachO, LoadError> {
    let r = Rd(data);
    if data.len() < 32 {
        return Err(LoadError::TooShort);
    }
    if r.u32(0)? != MH_MAGIC_64 {
        return Err(bad("not a 64-bit little-endian Mach-O image"));
    }
    if r.u32(4)? != CPU_TYPE_GUEST {
        return Err(LoadError::NotAarch64);
    }
    let cpusubtype = r.u32(8)?;
    let filetype = r.u32(12)?;
    let ncmds = r.u32(16)?;
    let sizeofcmds = r.u32(20)? as u64;
    let flags = r.u32(24)?;
    if 32 + sizeofcmds > data.len() as u64 {
        return Err(bad("load commands run past the end of the file"));
    }
    let mut m = MachO {
        cpusubtype,
        filetype,
        flags,
        segments: Vec::new(),
        entry: None,
        dylinker: None,
        id: None,
        dylibs: Vec::new(),
        uuid: None,
        symtab: None,
        function_starts: None,
    };
    let mut at = 32u64;
    let cmds_end = 32 + sizeofcmds;
    for _ in 0..ncmds {
        if at + 8 > cmds_end {
            return Err(bad("a load command runs past the load commands"));
        }
        let cmd = r.u32(at)?;
        let size = r.u32(at + 4)? as u64;
        if size < 8 || size % 8 != 0 || at + size > cmds_end {
            return Err(bad("a load command of an impossible size"));
        }
        let end = at + size;
        match cmd {
            LC_SEGMENT_64 => {
                if size < 72 {
                    return Err(bad("a short segment command"));
                }
                let nsects = r.u32(at + 64)? as u64;
                if 72 + nsects * 80 > size {
                    return Err(bad("a segment's sections run past its command"));
                }
                let mut sections = Vec::with_capacity(nsects as usize);
                for i in 0..nsects {
                    let s = at + 72 + i * 80;
                    sections.push(MachSection {
                        name: r.name16(s)?,
                        addr: r.u64(s + 32)?,
                        size: r.u64(s + 40)?,
                        flags: r.u32(s + 64)?,
                    });
                }
                let seg = MachSegment {
                    name: r.name16(at + 8)?,
                    vmaddr: r.u64(at + 24)?,
                    vmsize: r.u64(at + 32)?,
                    fileoff: r.u64(at + 40)?,
                    filesize: r.u64(at + 48)?,
                    maxprot: r.u32(at + 56)?,
                    initprot: r.u32(at + 60)?,
                    sections,
                };
                if seg.filesize > seg.vmsize && seg.vmsize != 0 {
                    return Err(bad("a segment with more file than memory"));
                }
                if seg.fileoff.checked_add(seg.filesize).is_none_or(|e| e > len) {
                    return Err(bad("a segment's contents lie outside the file"));
                }
                if seg.vmaddr.checked_add(seg.vmsize).is_none() {
                    return Err(bad("a segment that wraps the address space"));
                }
                m.segments.push(seg);
            }
            LC_UNIXTHREAD => {
                // flavor, count, then the thread state: ARM_THREAD_STATE64
                // (x0-x28, fp, lr, sp, pc) or x86_THREAD_STATE64 (rip the
                // seventeenth register).
                let pc_at = if gex_abi::X86 { at + 16 + 16 * 8 } else { at + 16 + 32 * 8 };
                if pc_at + 8 > end {
                    return Err(bad("a short thread command"));
                }
                m.entry = Some(Entry::Thread { pc: r.u64(pc_at)? });
            }
            LC_MAIN => {
                if size < 24 {
                    return Err(bad("a short entry-point command"));
                }
                m.entry = Some(Entry::Main { entryoff: r.u64(at + 8)?, stacksize: r.u64(at + 16)? });
            }
            LC_LOAD_DYLINKER => {
                let off = r.u32(at + 8)? as u64;
                m.dylinker = Some(r.cstr(at + off, end)?);
            }
            LC_ID_DYLIB => {
                let off = r.u32(at + 8)? as u64;
                m.id = Some(r.cstr(at + off, end)?);
            }
            LC_LOAD_DYLIB | LC_LOAD_WEAK_DYLIB | LC_REEXPORT_DYLIB | LC_LOAD_UPWARD_DYLIB => {
                let off = r.u32(at + 8)? as u64;
                m.dylibs.push(r.cstr(at + off, end)?);
            }
            LC_UUID if size >= 24 => {
                m.uuid = Some(r.bytes(at + 8, 16)?.try_into().unwrap());
            }
            LC_SYMTAB if size >= 24 => {
                m.symtab = Some((r.u32(at + 8)? as u64, r.u32(at + 12)? as u64, r.u32(at + 16)? as u64, r.u32(at + 20)? as u64));
            }
            LC_FUNCTION_STARTS if size >= 16 => {
                m.function_starts = Some((r.u32(at + 8)? as u64, r.u32(at + 12)? as u64));
            }
            _ => {}
        }
        at = end;
    }
    Ok(m)
}

/// A Mach-O file's (or its guest slice's) segments and function symbols,
/// unslid, and where in the file the slice starts: what a host needs to
/// find a library's functions where the guest's dynamic linker mapped it.
pub fn file_functions(bytes: &[u8]) -> Option<(MachO, Vec<Symbol>, u64)> {
    let (off, len) = guest_slice(bytes).ok()?;
    let data = bytes.get(off..off.checked_add(len)?)?;
    let m = parse(data).ok()?;
    let syms = functions(&m, data);
    Some((m, syms, off as u64))
}

/// The function symbols of `m` (unslid): the symbol table's definitions in
/// code sections, and every `LC_FUNCTION_STARTS` address no symbol names
/// (a stripped image has only those). Each is as long as the gap to the
/// next start in its section.
fn functions(m: &MachO, data: &[u8]) -> Vec<Symbol> {
    let r = Rd(data);
    // The sections in command order: `n_sect` counts them from one.
    let sections: Vec<&MachSection> = m.segments.iter().flat_map(|s| s.sections.iter()).collect();
    let is_code = |s: &MachSection| s.flags & (S_ATTR_PURE_INSTRUCTIONS | S_ATTR_SOME_INSTRUCTIONS) != 0;
    let mut found: std::collections::BTreeMap<u64, String> = std::collections::BTreeMap::new();
    if let Some((symoff, nsyms, stroff, strsize)) = m.symtab {
        for i in 0..nsyms.min(1 << 24) {
            let at = symoff + i * 16;
            let (Ok(strx), Ok(info), Ok(value)) = (r.u32(at), r.bytes(at + 4, 2), r.u64(at + 8)) else { break };
            let (n_type, n_sect) = (info[0], info[1]);
            if n_type & N_STAB != 0 || n_type & N_TYPE != N_SECT || n_sect == 0 {
                continue;
            }
            let Some(sect) = sections.get(n_sect as usize - 1) else { continue };
            if !is_code(sect) || value < sect.addr || value >= sect.addr + sect.size {
                continue;
            }
            if strx as u64 >= strsize {
                continue;
            }
            let Ok(name) = r.cstr(stroff + strx as u64, stroff + strsize) else { continue };
            // Assembler-local labels (`ltmp0`, `L...`) name no function.
            if name.is_empty() || name.starts_with('L') || name.starts_with("ltmp") {
                continue;
            }
            found.entry(value).or_insert(name);
        }
    }
    if let (Some((off, size)), Some(text)) = (m.function_starts, m.header_vmaddr()) {
        if let Ok(b) = r.bytes(off, size) {
            let mut addr = text;
            let mut i = 0;
            while i < b.len() {
                // One ULEB128 delta from the previous start; zero ends them.
                let (mut delta, mut shift) = (0u64, 0u32);
                loop {
                    let Some(&byte) = b.get(i) else { break };
                    i += 1;
                    delta |= ((byte & 0x7f) as u64).checked_shl(shift).unwrap_or(0);
                    shift += 7;
                    if byte & 0x80 == 0 {
                        break;
                    }
                }
                if delta == 0 {
                    break;
                }
                addr = addr.wrapping_add(delta);
                found.entry(addr).or_insert_with(|| format!("fn_{addr:x}"));
            }
        }
    }
    let starts: Vec<u64> = found.keys().copied().collect();
    found
        .into_iter()
        .enumerate()
        .map(|(i, (value, name))| {
            let sect_end = sections
                .iter()
                .find(|s| value >= s.addr && value < s.addr + s.size)
                .map_or(value, |s| s.addr + s.size);
            let next = starts.get(i + 1).copied().unwrap_or(u64::MAX);
            Symbol { name, value, size: next.min(sect_end) - value, ifunc: false }
        })
        .collect()
}

/// Place a parsed image: its mapped segments at `vmaddr + slide`, with
/// their bytes from `data` (the whole image), or left in the file (`None`:
/// a plan, whose segments are empty and whose symbols are not read).
fn place(m: &MachO, data: Option<&[u8]>, slide: u64) -> Result<Image, LoadError> {
    let mut segments = Vec::new();
    let mut code = Vec::new();
    let (mut text_lo, mut text_hi, mut top) = (u64::MAX, 0u64, 0u64);
    let limit = gex_abi::mem_flavor().limit();
    for s in m.mapped() {
        let vaddr = s.vmaddr.wrapping_add(slide);
        // Mapped by whole pages, as XNU maps them.
        let mem_size = s.vmsize.next_multiple_of(PAGE_SIZE);
        let end = vaddr.checked_add(mem_size).filter(|e| *e <= limit);
        let Some(end) = end else {
            return Err(LoadError::SegmentOutOfRange { vaddr, size: mem_size });
        };
        if vaddr < gex_abi::DISPATCH_DIR_END {
            return Err(LoadError::InRuntimeRegion { vaddr, size: mem_size });
        }
        let executable = s.initprot & VM_PROT_EXECUTE != 0;
        if executable {
            text_lo = text_lo.min(vaddr);
            text_hi = text_hi.max(end);
            for sect in &s.sections {
                if sect.flags & (S_ATTR_PURE_INSTRUCTIONS | S_ATTR_SOME_INSTRUCTIONS) != 0 && sect.size != 0 {
                    let a = sect.addr.wrapping_add(slide);
                    code.push((a, a + sect.size));
                }
            }
        }
        top = top.max(end);
        segments.push(Segment {
            vaddr,
            bytes: data.map_or_else(Vec::new, |d| d[s.fileoff as usize..(s.fileoff + s.filesize) as usize].to_vec()),
            mem_size,
            readable: s.initprot & VM_PROT_READ != 0,
            writable: s.initprot & VM_PROT_WRITE != 0,
            executable,
        });
    }
    if segments.is_empty() {
        return Err(bad("an image with nothing to map"));
    }
    for (i, a) in segments.iter().enumerate() {
        for b in &segments[i + 1..] {
            if a.vaddr < b.vaddr + b.mem_size && b.vaddr < a.vaddr + a.mem_size {
                return Err(bad("an image whose segments overlap"));
            }
        }
    }
    let header = m.header_vmaddr().map(|a| a.wrapping_add(slide));
    let entry = match m.entry {
        Some(Entry::Thread { pc }) => pc.wrapping_add(slide),
        Some(Entry::Main { entryoff, .. }) => header.ok_or(bad("an entry point with no header to count from"))?.wrapping_add(entryoff),
        None => 0,
    };
    let symbols = data
        .map_or_else(Vec::new, |d| functions(m, d))
        .into_iter()
        .map(|mut s| {
            s.value = s.value.wrapping_add(slide);
            s
        })
        .collect();
    Ok(Image {
        bias: slide,
        class32: false,
        entry,
        segments,
        // No program headers: the header a Mach-O process finds is its
        // image's own, whose address the stack gives it.
        phdr_vaddr: 0,
        phentsize: 0,
        phnum: 0,
        brk: top,
        text_range: if text_lo == u64::MAX { (0, 0) } else { (text_lo, text_hi) },
        code,
        tls: None,
        symbols,
        relocs: Vec::new(),
        init_array: Vec::new(),
        dynsyms: Vec::new(),
        named_relocs: Vec::new(),
        irelative: Vec::new(),
        pie: m.pie(),
        interp: m.dylinker.clone(),
        soname: m.id.clone(),
        needed: m.dylibs.clone(),
        runpath: Vec::new(),
    })
}

/// The slide that puts a position-independent image's lowest segment at
/// `base`; none for an image linked to stay where it is.
fn slide_for(m: &MachO, base: u64) -> Result<u64, LoadError> {
    let (lo, _) = m.span().ok_or(bad("an image with nothing to map"))?;
    Ok(if m.pie() { base.wrapping_sub(lo) } else { 0 })
}

/// What a Darwin process is started with beyond its images.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Darwin {
    /// The address of the main executable's Mach-O header.
    pub mach_header: u64,
    /// Whether the dynamic linker starts the program (the header's address
    /// is then on the stack below `argc`).
    pub dynamic: bool,
    /// `LC_MAIN`'s stack size, 0 for the default.
    pub stack_size: u64,
    /// The main executable is arm64e.
    pub arm64e: bool,
    /// The main executable's `LC_UUID`.
    pub uuid: Option<[u8; 16]>,
}

/// Load a Mach-O executable, and the dynamic linker it names, as
/// [`crate::load_program`] loads an ELF program and its interpreter.
pub fn load_program(
    bytes: &[u8],
    pie_base: u64,
    interp_base: u64,
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Result<Program, LoadError> {
    let (off, len) = guest_slice(bytes)?;
    let data = &bytes[off..off + len];
    let m = parse(data)?;
    if m.filetype != MH_EXECUTE {
        return Err(LoadError::UnsupportedType(m.filetype as u16));
    }
    let exe = place(&m, Some(data), slide_for(&m, pie_base)?)?;
    let mut darwin = darwin_of(&m, &exe);
    let Some(path) = m.dylinker.clone() else {
        if !matches!(m.entry, Some(Entry::Thread { .. })) {
            return Err(bad("an executable with neither a dynamic linker nor a thread state to start from"));
        }
        return Ok(Program { exe, interp: None, direct: None, darwin: Some(darwin) });
    };
    let Some(linker_bytes) = read(&path) else {
        return Err(LoadError::NeedsInterpreter(path));
    };
    let (loff, llen) = guest_slice(&linker_bytes)?;
    let ldata = &linker_bytes[loff..loff + llen];
    let lm = parse(ldata)?;
    if lm.filetype != MH_DYLINKER || !matches!(lm.entry, Some(Entry::Thread { .. })) {
        return Err(bad("the dynamic linker is not one"));
    }
    let interp = place(&lm, Some(ldata), slide_for(&lm, interp_base)?)?;
    check_apart(&exe, &interp)?;
    darwin.dynamic = true;
    Ok(Program { exe, interp: Some(interp), direct: None, darwin: Some(darwin) })
}

/// Two independently placed images must not claim the same addresses.
fn check_apart(exe: &Image, interp: &Image) -> Result<(), LoadError> {
    for a in &exe.segments {
        for b in &interp.segments {
            let (a_end, b_end) = (a.vaddr + a.mem_size, b.vaddr + b.mem_size);
            if a.vaddr < b_end && b.vaddr < a_end {
                return Err(LoadError::ImageOverlap {
                    first_start: a.vaddr,
                    first_end: a_end,
                    second_start: b.vaddr,
                    second_end: b_end,
                });
            }
        }
    }
    Ok(())
}

/// What [`Darwin`] says of executable `m`, placed as `exe`.
fn darwin_of(m: &MachO, exe: &Image) -> Darwin {
    Darwin {
        mach_header: m.header_vmaddr().map_or(0, |a| a.wrapping_add(exe.bias)),
        dynamic: false,
        stack_size: match m.entry {
            Some(Entry::Main { stacksize, .. }) => stacksize,
            _ => 0,
        },
        arm64e: m.arm64e(),
        uuid: m.uuid,
    }
}

/// One image of a plan: the guest's slice of the file `read_at` reads,
/// parsed from its header and load commands alone, and where the slice
/// starts in the file.
fn plan_image(read_at: &dyn Fn(u64, &mut [u8]) -> Option<usize>) -> Result<(MachO, u64), LoadError> {
    let read = |off: u64, len: usize| -> Result<Vec<u8>, LoadError> {
        let mut v = vec![0u8; len];
        let n = read_at(off, &mut v).ok_or(LoadError::TooShort)?;
        v.truncate(n);
        Ok(v)
    };
    // A universal file's table of slices is in its first page; a slice's
    // header and load commands are its first `32 + sizeofcmds` bytes.
    let head = read(0, 4096)?;
    if !is_macho(&head) {
        return Err(bad("not a Mach-O file"));
    }
    let thin = head.len() >= 4 && u32::from_le_bytes(head[..4].try_into().unwrap()) == MH_MAGIC_64;
    let (off, len) = if thin {
        (0, u64::MAX)
    } else {
        // (`guest_slice` checks each slice against the file's length, which
        // a plan does not know: it is taken as the table's largest claim.)
        let (o, l) = guest_slice_of(&head)?;
        (o as u64, l as u64)
    };
    let mut cmds = read(off, 4096)?;
    if cmds.len() < 32 {
        return Err(LoadError::TooShort);
    }
    let need = 32 + u32::from_le_bytes(cmds[20..24].try_into().unwrap()) as usize;
    if need > cmds.len() && need <= 1 << 22 {
        cmds = read(off, need)?;
    }
    Ok((parse_head(&cmds, len)?, off))
}

/// [`guest_slice`] of a universal file's first bytes alone: the slices it
/// names lie past them.
fn guest_slice_of(table: &[u8]) -> Result<(usize, usize), LoadError> {
    let n = u32::from_be_bytes(table.get(4..8).ok_or(LoadError::TooShort)?.try_into().unwrap()) as usize;
    let wide = u32::from_be_bytes(table[..4].try_into().unwrap()) == FAT_MAGIC_64;
    let entry = if wide { 32 } else { 20 };
    let mut end = table.len() as u64;
    for i in 0..n.min(32) {
        let at = 8 + i * entry;
        let Some(e) = table.get(at..at + entry) else { return Err(LoadError::TooShort) };
        let (off, len) = if wide {
            (u64::from_be_bytes(e[8..16].try_into().unwrap()), u64::from_be_bytes(e[16..24].try_into().unwrap()))
        } else {
            (u32::from_be_bytes(e[8..12].try_into().unwrap()) as u64, u32::from_be_bytes(e[12..16].try_into().unwrap()) as u64)
        };
        end = end.max(off.saturating_add(len));
    }
    guest_slice_in(table, end)
}

/// [`load_program`] for `execve`, from headers alone, as
/// [`crate::plan_program`] plans an ELF program: the program with its
/// segments' bytes left in the files, and where in each file they are --
/// the executable's ranges, then the dynamic linker's, in
/// [`Program::placements`] order. `read_at` reads a range of a guest path.
pub fn plan_program(
    exe: &str,
    pie_base: u64,
    interp_base: u64,
    read_at: &dyn Fn(&str, u64, &mut [u8]) -> Option<usize>,
) -> Result<(Program, crate::elf::FileRanges), LoadError> {
    let (m, off) = plan_image(&|o, b| read_at(exe, o, b))?;
    if m.filetype != MH_EXECUTE {
        return Err(LoadError::UnsupportedType(m.filetype as u16));
    }
    let image = place(&m, None, slide_for(&m, pie_base)?)?;
    let mut ranges: crate::elf::FileRanges = m.mapped().map(|s| (off + s.fileoff, s.filesize)).collect();
    let mut darwin = darwin_of(&m, &image);
    let Some(path) = m.dylinker.clone() else {
        if !matches!(m.entry, Some(Entry::Thread { .. })) {
            return Err(bad("an executable with neither a dynamic linker nor a thread state to start from"));
        }
        return Ok((Program { exe: image, interp: None, direct: None, darwin: Some(darwin) }, ranges));
    };
    let (lm, loff) = match plan_image(&|o, b| read_at(&path, o, b)) {
        Ok(p) => p,
        Err(_) if read_at(&path, 0, &mut [0u8; 1]).is_none() => return Err(LoadError::NeedsInterpreter(path)),
        Err(e) => return Err(e),
    };
    if lm.filetype != MH_DYLINKER || !matches!(lm.entry, Some(Entry::Thread { .. })) {
        return Err(bad("the dynamic linker is not one"));
    }
    let interp = place(&lm, None, slide_for(&lm, interp_base)?)?;
    check_apart(&image, &interp)?;
    ranges.extend(lm.mapped().map(|s| (loff + s.fileoff, s.filesize)));
    darwin.dynamic = true;
    Ok((Program { exe: image, interp: Some(interp), direct: None, darwin: Some(darwin) }, ranges))
}

/// What the `apple` vector says of a new process, beyond its path.
#[derive(Clone, Debug, Default)]
pub struct AppleStrings {
    /// The path the program was started by (`executable_path=`).
    pub exe_path: String,
    /// The main thread's stack: its top and its size.
    pub stack: (u64, u64),
    /// The main thread's port name (`th_port=`), when the kernel has made
    /// one.
    pub thread_port: Option<u32>,
    /// Sixteen bytes the guards and the allocator's entropy are drawn from.
    pub random: [u8; 16],
}

impl AppleStrings {
    /// The strings, in XNU's order (`exec_add_apple_strings`).
    pub fn strings(&self, darwin: &Darwin) -> Vec<String> {
        let word = |i: usize| {
            // A different mix of the sixteen bytes for each value.
            let mut h = 0xcbf2_9ce4_8422_2325u64 ^ (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            for b in self.random {
                h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
            }
            h
        };
        let mut v = vec![
            format!("executable_path={}", self.exe_path),
            // No preemption-free zone: there is no commpage text.
            "pfz=0x0".to_string(),
            // The first guard's low byte is zero, so that the canary ends
            // any C string that runs into it.
            format!("stack_guard=0x{:016x}", word(0) & !0xff),
            format!("malloc_entropy=0x{:016x},0x{:016x}", word(1), word(2)),
            format!("ptr_munge=0x{:016x}", word(3)),
            format!("main_stack=0x{:x},0x{:x},0x{:x},0x{:x}", self.stack.0, self.stack.1, self.stack.0, self.stack.1),
            // What the kernel knows the program's file by, for the
            // allocator to keep its secrets by: twenty bytes of hash.
            // (libmalloc will not start the first process without it.)
            format!("executable_boothash={:016x}{:016x}{:08x}", word(4), word(5), word(6) as u32),
        ];
        if !gex_abi::X86 {
            v.push(format!("arm64e_abi={}", if darwin.arm64e { "os" } else { "all" }));
        }
        if let Some(p) = self.thread_port {
            v.push(format!("th_port=0x{p:x}"));
        }
        v
    }
}

/// The initial stack of a Darwin process (`exec_copyout_strings`), from
/// `stack_top` down:
///
/// ```text
///         the strings: the path, argv's, envp's, apple's
///         0
///         apple[n-1] .. apple[0]
///         0
///         envp[n-1] .. envp[0]
///         0
///         argv[argc-1] .. argv[0]
///         argc
///   sp -> the main executable's header      (a dynamic program only)
/// ```
pub fn build_stack(darwin: &Darwin, stack_top: u64, args: &[String], env: &[String], apple: &[String]) -> StackLayout {
    let mut blob: Vec<u8> = Vec::new();
    let mut put = |list: &[String]| -> Vec<usize> {
        list.iter()
            .map(|s| {
                let at = blob.len();
                blob.extend_from_slice(s.as_bytes());
                blob.push(0);
                at
            })
            .collect()
    };
    // The apple strings first, as XNU has the path (the first of them)
    // lowest; then the arguments and the environment.
    let apple_at = put(apple);
    let argv_at = put(args);
    let env_at = put(env);
    while !blob.len().is_multiple_of(16) {
        blob.push(0);
    }
    // Sixteen bytes are left free at the very top, as XNU leaves them.
    let blob_addr = (stack_top - 16 - blob.len() as u64) & !0xf;
    let mut words: Vec<u64> = Vec::new();
    if darwin.dynamic {
        words.push(darwin.mach_header);
    }
    words.push(args.len() as u64);
    words.extend(argv_at.iter().map(|o| blob_addr + *o as u64));
    words.push(0);
    words.extend(env_at.iter().map(|o| blob_addr + *o as u64));
    words.push(0);
    words.extend(apple_at.iter().map(|o| blob_addr + *o as u64));
    words.push(0);
    let sp = (blob_addr - words.len() as u64 * 8) & !0xf;
    let mut bytes = vec![0u8; (blob_addr + blob.len() as u64 - sp) as usize];
    for (i, w) in words.iter().enumerate() {
        bytes[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes());
    }
    let at = (blob_addr - sp) as usize;
    bytes[at..at + blob.len()].copy_from_slice(&blob);
    StackLayout { sp, low: sp, bytes }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u64 = 0x1_0000_0000;

    fn put32(v: &mut [u8], at: usize, x: u32) {
        v[at..at + 4].copy_from_slice(&x.to_le_bytes());
    }
    fn put64(v: &mut [u8], at: usize, x: u64) {
        v[at..at + 8].copy_from_slice(&x.to_le_bytes());
    }

    /// A segment command with no sections.
    fn segment(name: &str, vmaddr: u64, vmsize: u64, fileoff: u64, filesize: u64, prot: u32) -> Vec<u8> {
        let mut c = vec![0u8; 72];
        put32(&mut c, 0, LC_SEGMENT_64);
        put32(&mut c, 4, 72);
        c[8..8 + name.len()].copy_from_slice(name.as_bytes());
        put64(&mut c, 24, vmaddr);
        put64(&mut c, 32, vmsize);
        put64(&mut c, 40, fileoff);
        put64(&mut c, 48, filesize);
        put32(&mut c, 56, prot);
        put32(&mut c, 60, prot);
        c
    }

    fn thread(pc: u64) -> Vec<u8> {
        let n = if gex_abi::X86 { 16 + 21 * 8 } else { 16 + 34 * 8 };
        let mut c = vec![0u8; n];
        put32(&mut c, 0, LC_UNIXTHREAD);
        put32(&mut c, 4, n as u32);
        put64(&mut c, if gex_abi::X86 { 16 + 16 * 8 } else { 16 + 32 * 8 }, pc);
        c
    }

    fn string_cmd(cmd: u32, s: &str) -> Vec<u8> {
        let n = (12 + s.len() + 1).next_multiple_of(8);
        let mut c = vec![0u8; n];
        put32(&mut c, 0, cmd);
        put32(&mut c, 4, n as u32);
        put32(&mut c, 8, 12);
        c[12..12 + s.len()].copy_from_slice(s.as_bytes());
        c
    }

    fn main_cmd(entryoff: u64) -> Vec<u8> {
        let mut c = vec![0u8; 24];
        put32(&mut c, 0, LC_MAIN);
        put32(&mut c, 4, 24);
        put64(&mut c, 8, entryoff);
        c
    }

    /// A file of `PAGE_SIZE * 2` bytes: the header and commands in its
    /// first page (`__TEXT`), a second page of data.
    fn image(filetype: u32, flags: u32, base: u64, cmds: &[Vec<u8>]) -> Vec<u8> {
        let p = PAGE_SIZE;
        let mut all = vec![
            segment("__TEXT", base, p, 0, p, VM_PROT_READ | VM_PROT_EXECUTE),
            segment("__DATA", base + p, 2 * p, p, p, VM_PROT_READ | VM_PROT_WRITE),
        ];
        if filetype == MH_EXECUTE {
            all.insert(0, segment("__PAGEZERO", 0, base, 0, 0, 0));
        }
        all.extend(cmds.iter().cloned());
        let mut v = vec![0u8; 2 * p as usize];
        put32(&mut v, 0, MH_MAGIC_64);
        put32(&mut v, 4, CPU_TYPE_GUEST);
        put32(&mut v, 12, filetype);
        put32(&mut v, 16, all.len() as u32);
        put32(&mut v, 20, all.iter().map(|c| c.len()).sum::<usize>() as u32);
        put32(&mut v, 24, flags);
        let mut at = 32;
        for c in &all {
            v[at..at + c.len()].copy_from_slice(c);
            at += c.len();
        }
        v[p as usize] = 0x5a;
        v
    }

    #[test]
    fn a_static_executable_is_placed_where_it_was_linked() {
        let low = gex_abi::PIE_BASE;
        let file = image(MH_EXECUTE, 0, low, &[thread(low + 0x400)]);
        assert!(is_macho(&file));
        let p = load_program(&file, 0x2000_0000, 0x3000_0000, &|_| None).unwrap();
        let d = p.darwin.as_ref().unwrap();
        assert!(!d.dynamic);
        assert_eq!(d.mach_header, low);
        assert_eq!(p.entry(), low + 0x400);
        // `__PAGEZERO` is address space, not a mapping.
        assert_eq!(p.exe.segments.len(), 2);
        assert_eq!(p.exe.segments[1].vaddr, low + PAGE_SIZE);
        assert_eq!(p.exe.segments[1].mem_size, 2 * PAGE_SIZE);
        assert_eq!(p.exe.segments[1].bytes[0], 0x5a);
        assert_eq!(p.exe.brk, low + 3 * PAGE_SIZE);
    }

    #[test]
    fn a_dynamic_executable_is_slid_and_enters_its_linker() {
        let exe = image(MH_EXECUTE, MH_PIE, BASE, &[main_cmd(0x123), string_cmd(LC_LOAD_DYLINKER, "/usr/lib/dyld")]);
        let dyld = image(MH_DYLINKER, 0, 0, &[thread(0x40)]);
        let p = load_program(&exe, 0x2000_0000, 0x3000_0000, &|path| (path == "/usr/lib/dyld").then(|| dyld.clone())).unwrap();
        let d = p.darwin.as_ref().unwrap();
        assert!(d.dynamic);
        // The 4 GiB link address slides down to where gex puts programs.
        assert_eq!(d.mach_header, 0x2000_0000);
        assert_eq!(p.exe.bias, 0x2000_0000u64.wrapping_sub(BASE));
        assert_eq!(p.exe.entry, 0x2000_0000 + 0x123);
        assert_eq!(p.entry(), 0x3000_0000 + 0x40);
        assert_eq!(p.exe.interp.as_deref(), Some("/usr/lib/dyld"));
        let missing = load_program(&exe, 0x2000_0000, 0x3000_0000, &|_| None).unwrap_err();
        assert_eq!(missing, LoadError::NeedsInterpreter("/usr/lib/dyld".into()));
    }

    /// `execve`'s plan places what `load_program` places, from the files.
    #[test]
    fn a_plan_is_the_program_without_its_bytes() {
        let exe = image(MH_EXECUTE, MH_PIE, BASE, &[main_cmd(0x123), string_cmd(LC_LOAD_DYLINKER, "/usr/lib/dyld")]);
        let dyld = image(MH_DYLINKER, 0, 0, &[thread(0x40)]);
        let file = |path: &str| match path {
            "/exe" => Some(exe.clone()),
            "/usr/lib/dyld" => Some(dyld.clone()),
            _ => None,
        };
        let read_at = |path: &str, off: u64, out: &mut [u8]| {
            let f = file(path)?;
            let from = (off as usize).min(f.len());
            let n = out.len().min(f.len() - from);
            out[..n].copy_from_slice(&f[from..from + n]);
            Some(n)
        };
        let loaded = load_program(&exe, 0x2000_0000, 0x3000_0000, &|p| file(p)).unwrap();
        let (planned, ranges) = plan_program("/exe", 0x2000_0000, 0x3000_0000, &read_at).unwrap();
        assert_eq!(planned.entry(), loaded.entry());
        assert_eq!(planned.darwin, loaded.darwin);
        let segs: Vec<_> = loaded.placements().collect();
        let plan: Vec<_> = planned.placements().collect();
        assert_eq!((segs.len(), ranges.len()), (plan.len(), plan.len()));
        let files = [&exe, &dyld];
        for (i, ((l, lf), (p, pf))) in segs.iter().zip(&plan).enumerate() {
            assert_eq!((l.vaddr, l.mem_size, lf), (p.vaddr, p.mem_size, pf));
            assert!(p.bytes.is_empty());
            let f = files[usize::from(pf.is_some())];
            let (o, n) = ranges[i];
            assert_eq!(&f[o as usize..(o + n) as usize], &l.bytes[..]);
        }
        assert!(plan_program("/nope", 0x2000_0000, 0x3000_0000, &read_at).is_err());
    }

    #[test]
    fn a_universal_file_gives_the_guests_slice() {
        let thin = image(MH_EXECUTE, 0, gex_abi::PIE_BASE, &[thread(gex_abi::PIE_BASE)]);
        let other = if gex_abi::X86 { CPU_TYPE_ARM64 } else { CPU_TYPE_X86_64 };
        let at = 0x8000usize;
        let mut fat = vec![0u8; at + thin.len()];
        fat[0..4].copy_from_slice(&FAT_MAGIC.to_be_bytes());
        fat[4..8].copy_from_slice(&2u32.to_be_bytes());
        for (i, (cpu, off, len)) in [(other, 0x4000u32, 0x4000u32), (CPU_TYPE_GUEST, at as u32, thin.len() as u32)].iter().enumerate() {
            let e = 8 + i * 20;
            fat[e..e + 4].copy_from_slice(&cpu.to_be_bytes());
            fat[e + 8..e + 12].copy_from_slice(&off.to_be_bytes());
            fat[e + 12..e + 16].copy_from_slice(&len.to_be_bytes());
        }
        fat[at..].copy_from_slice(&thin);
        assert!(is_macho(&fat));
        assert_eq!(guest_slice(&fat).unwrap(), (at, thin.len()));
        assert!(load_program(&fat, 0x2000_0000, 0x3000_0000, &|_| None).is_ok());
        // A Java class file has the same first four bytes.
        assert!(!is_macho(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 0x34]));
    }

    #[test]
    fn the_stack_is_the_one_xnu_builds() {
        let d = Darwin { mach_header: 0x1234_0000, dynamic: true, stack_size: 0, arm64e: false, uuid: None };
        let args = vec!["prog".to_string(), "one".to_string()];
        let env = vec!["A=1".to_string()];
        let apple = AppleStrings { exe_path: "/bin/prog".into(), stack: (0x8000_0000, 0x80_0000), thread_port: Some(0x103), random: [7; 16] }.strings(&d);
        let l = build_stack(&d, 0x8000_0000, &args, &env, &apple);
        assert_eq!(l.sp % 16, 0);
        let word = |a: u64| u64::from_le_bytes(l.bytes[(a - l.low) as usize..][..8].try_into().unwrap());
        let text = |a: u64| {
            let b = &l.bytes[(a - l.low) as usize..];
            String::from_utf8_lossy(&b[..b.iter().position(|c| *c == 0).unwrap()]).into_owned()
        };
        assert_eq!(word(l.sp), 0x1234_0000);
        assert_eq!(word(l.sp + 8), 2);
        assert_eq!(text(word(l.sp + 16)), "prog");
        assert_eq!(text(word(l.sp + 24)), "one");
        assert_eq!(word(l.sp + 32), 0);
        assert_eq!(text(word(l.sp + 40)), "A=1");
        assert_eq!(word(l.sp + 48), 0);
        assert_eq!(text(word(l.sp + 56)), "executable_path=/bin/prog");
        let mut at = l.sp + 56;
        let mut seen = Vec::new();
        while word(at) != 0 {
            seen.push(text(word(at)));
            at += 8;
        }
        assert!(seen.iter().any(|s| s == "th_port=0x103"));
        assert!(seen.iter().any(|s| s.starts_with("stack_guard=0x") && s.ends_with("00")));
        assert!(l.low + l.bytes.len() as u64 <= 0x8000_0000 - 16);
        // A static program finds argc at sp.
        let s = build_stack(&Darwin { dynamic: false, ..d }, 0x8000_0000, &args, &env, &apple);
        let word = |a: u64| u64::from_le_bytes(s.bytes[(a - s.low) as usize..][..8].try_into().unwrap());
        assert_eq!(word(s.sp), 2);
    }
}
