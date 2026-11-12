//! Loading a dynamically linked program (*cooperative mode*).
//!
//! Places the executable and its own interpreter and enters the interpreter,
//! letting it run translated. This is what musl needs (its loader state has no
//! symbolic interface), and the always-correct fallback for an unrecognised
//! `PT_INTERP`. [`crate::link`] is the alternative for glibc and static-PIE.
//!
//! The auxiliary vector must be exact: the interpreter is entered rather than
//! the program, so it needs `AT_BASE` to find itself, `AT_PHDR`/`AT_PHENT`/
//! `AT_PHNUM` describing the *executable*, and `AT_ENTRY` to jump to when done.

use crate::elf::{load, Image, LoadError};

/// A program and, when it needs one, its interpreter.
#[derive(Clone, Debug)]
pub struct Program {
    /// The executable.
    pub exe: Image,
    /// The dynamic linker, already placed.
    pub interp: Option<Image>,
    /// A Mach-O program: what its Darwin process is started with
    /// ([`crate::macho`]). `interp` is then its dynamic linker, dyld.
    pub darwin: Option<crate::macho::Darwin>,
}

impl Program {
    /// Where execution begins: the interpreter when there is one.
    pub fn entry(&self) -> u64 {
        match &self.interp {
            Some(i) => i.entry,
            None => self.exe.entry,
        }
    }

    /// Whether this is a 32-bit (i386) program: its code runs in
    /// compatibility mode and it speaks the i386 system-call ABI.
    pub fn compat(&self) -> bool {
        self.exe.class32
    }

    /// Whether this is a Darwin program: it speaks XNU's system-call ABI.
    pub fn is_darwin(&self) -> bool {
        self.darwin.is_some()
    }

    /// The guest base this program's process runs with
    /// ([`gex_abi::guest_base`]): a Darwin program with a Mac's address space
    /// gets the window ([`crate::macho::Darwin::windowed`]), every other
    /// none.
    pub fn guest_base(&self) -> u64 {
        if self.darwin.as_ref().is_some_and(|d| d.windowed) && !gex_abi::mem64() { gex_abi::DARWIN_BASE } else { 0 }
    }

    /// Load bias of the interpreter, for `AT_BASE`.
    pub fn interp_base(&self) -> u64 {
        self.interp.as_ref().map(|i| i.bias).unwrap_or(0)
    }

    /// Every segment that must be placed in guest memory.
    pub fn segments(&self) -> impl Iterator<Item = &crate::elf::Segment> {
        self.exe
            .segments
            .iter()
            .chain(self.interp.iter().flat_map(|i| i.segments.iter()))
    }

    /// Every segment to place, paired with the interpreter's path when the
    /// segment is the interpreter's. [`Self::segments`] loses that distinction,
    /// which makes two processes fail to recognise `ld.so` as shared code and
    /// re-lift the whole dynamic linker per executable.
    pub fn placements(&self) -> impl Iterator<Item = (&crate::elf::Segment, Option<&str>)> {
        let interp = self.exe.interp.as_deref();
        self.exe
            .segments
            .iter()
            .map(|s| (s, None))
            .chain(self.interp.iter().flat_map(move |i| {
                i.segments.iter().map(move |s| (s, interp))
            }))
    }

    /// First address above everything loaded, where `brk` starts.
    pub fn brk(&self) -> u64 {
        let interp_end = self.interp.as_ref().map(|i| i.brk).unwrap_or(0);
        self.exe.brk.max(interp_end)
    }
}

/// Room for an image the fixed layout has none for: asked for `len` bytes,
/// the address (64 KiB aligned) of a free stretch of the arena at least that
/// long, which is the caller's from then on. A position-independent
/// executable too big for the space below the interpreter goes there
/// ([`gex_abi::exe_bias`]), and so does the interpreter of a fixed-address
/// executable that reaches its address.
pub type Arena<'a> = &'a mut dyn FnMut(u64) -> u64;

/// The [`Arena`] of an address space nothing has been taken from: up from
/// [`gex_abi::arena_bottom`], each answer 64 KiB past the last one's end.
/// Its first answer is [`gex_abi::exe_bias`]'s. A native host's guest memory
/// holds nothing of its own there, so this is where every image goes
/// natively, and where a build ahead of time expects it.
pub fn fresh_arena() -> impl FnMut(u64) -> u64 {
    const SLACK: u64 = 0x1_0000;
    let mut next = gex_abi::arena_bottom();
    move |len| {
        let at = next.next_multiple_of(SLACK);
        next = at + len.next_multiple_of(SLACK) + SLACK;
        at
    }
}

/// An arena that is never asked: for a caller whose program must fit the
/// fixed layout, or that only reads an image's headers. Answers the bottom
/// of the arena, which nothing then holds.
pub fn no_arena(_len: u64) -> u64 {
    gex_abi::arena_bottom()
}

/// Whether an executable whose mappings reach `span` bytes above its bias
/// fits at `pie_base`, below an interpreter at `interp_base`
/// ([`gex_abi::exe_fits`] for gex's own layout).
fn fits(pie_base: u64, interp_base: u64, span: u64) -> bool {
    pie_base.checked_add(span).is_some_and(|end| end <= interp_base)
}

/// A fixed-address image that reaches gex's stack or the runtime's gap above
/// it, `[STACK_TOP - STACK_SIZE, MMAP_BOTTOM)`, is refused: loaded, it would
/// be overwritten by the initial stack and corrupt it in turn. (Below the
/// stack it fits; its interpreter, if it reaches that, goes in the arena.)
pub(crate) fn check_clear_of_stack(exe: &Image) -> Result<(), LoadError> {
    let (lo, hi) = (gex_abi::STACK_TOP - gex_abi::STACK_SIZE, gex_abi::MMAP_BOTTOM);
    for s in &exe.segments {
        let end = s.vaddr + s.mem_size;
        if s.mem_size != 0 && s.vaddr < hi && end > lo {
            let start = exe.segments.iter().map(|s| s.vaddr).min().unwrap_or(s.vaddr);
            let end = exe.segments.iter().map(|s| s.vaddr + s.mem_size).max().unwrap_or(end);
            return Err(LoadError::ReachesStack { start, end });
        }
    }
    Ok(())
}

/// The bias an executable goes at: `pie_base`, or for a position-independent
/// one that does not fit below the interpreter there, the arena's answer.
/// From the image as placed at `pie_base`, which says its extent.
fn exe_base(at_pie_base: &Image, pie_base: u64, interp_base: u64, arena: Arena) -> u64 {
    let span = at_pie_base.brk.saturating_sub(pie_base);
    if at_pie_base.pie && !fits(pie_base, interp_base, span) {
        arena(span)
    } else {
        pie_base
    }
}

/// Where the interpreter goes: `interp_base`, or the arena when the
/// executable (a fixed-address one: a position-independent executable never
/// reaches it) runs into its range, `[interp_base, interp_base + span)`.
fn interp_place(exe: &Image, interp_base: u64, interp_span: u64, arena: Arena) -> u64 {
    let (lo, hi) = (interp_base, interp_base + interp_span);
    if exe.segments.iter().any(|s| s.vaddr < hi && s.vaddr + s.mem_size > lo) {
        arena(interp_span)
    } else {
        interp_base
    }
}

/// Load a program, following `PT_INTERP` if it has one.
///
/// `read` resolves a guest path to file contents, so the interpreter comes
/// from the guest's own namespace — the sysroot when one is mounted — rather
/// than from the host. A position-independent executable goes at `pie_base`
/// and its interpreter at `interp_base`, unless the fixed layout has no room
/// for one of them: then `arena` places it ([`Arena`]; [`fresh_arena`] for a
/// new address space).
pub fn load_program(
    bytes: &[u8],
    pie_base: u64,
    interp_base: u64,
    arena: Arena,
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Result<Program, LoadError> {
    if crate::macho::is_macho(bytes) {
        return crate::macho::load_program(bytes, pie_base, interp_base, arena, read);
    }
    // The headers alone say how far the image reaches, before any bytes are
    // copied (or relocated) at a base it might not keep.
    let (head, _) = crate::elf::plan(&|o, b| read_slice(bytes, o, b), pie_base)?;
    let base = exe_base(&head, pie_base, interp_base, arena);
    let exe = load(bytes, base)?;
    check_clear_of_stack(&exe)?;
    let Some(path) = exe.interp.clone() else {
        return Ok(Program { exe, interp: None, darwin: None });
    };
    let Some(interp_bytes) = read(&path) else {
        return Err(LoadError::NeedsInterpreter(path));
    };
    // The interpreter is position-independent and goes wherever we say, well
    // clear of the executable so neither can overlap the other's mappings.
    let (ihead, _) = crate::elf::plan(&|o, b| read_slice(&interp_bytes, o, b), interp_base)?;
    let ibase = interp_place(&exe, interp_base, ihead.brk.saturating_sub(interp_base), arena);
    let interp = load(&interp_bytes, ibase)?;
    check_overlap(&exe, &interp)?;
    Ok(Program {
        exe,
        interp: Some(interp),
        darwin: None,
    })
}

/// `read_at` over bytes in hand.
fn read_slice(bytes: &[u8], off: u64, out: &mut [u8]) -> Option<usize> {
    let from = usize::try_from(off).ok()?.min(bytes.len());
    let n = out.len().min(bytes.len() - from);
    out[..n].copy_from_slice(&bytes[from..from + n]);
    Some(n)
}

/// [`load_program`] for `execve`, from headers alone ([`crate::elf::plan`]):
/// the program with its segments' bytes left in the files, and where in
/// each file they are -- `exe`'s ranges, then the interpreter's, in
/// [`Program::placements`] order. `read_at` reads a range of a guest path.
pub fn plan_program(
    exe: &str,
    pie_base: u64,
    interp_base: u64,
    arena: Arena,
    read_at: &dyn Fn(&str, u64, &mut [u8]) -> Option<usize>,
) -> Result<(Program, crate::elf::FileRanges), LoadError> {
    let mut magic = [0u8; 8];
    if read_at(exe, 0, &mut magic) == Some(8) && crate::macho::is_macho(&magic) {
        return crate::macho::plan_program(exe, pie_base, interp_base, arena, read_at);
    }
    let (head, ranges) = crate::elf::plan(&|o, b| read_at(exe, o, b), pie_base)?;
    let base = exe_base(&head, pie_base, interp_base, arena);
    let (exe_image, mut ranges) = if base == pie_base {
        (head, ranges)
    } else {
        crate::elf::plan(&|o, b| read_at(exe, o, b), base)?
    };
    check_clear_of_stack(&exe_image)?;
    let Some(path) = exe_image.interp.clone() else {
        return Ok((Program { exe: exe_image, interp: None, darwin: None }, ranges));
    };
    let (interp, interp_ranges) = match crate::elf::plan(&|o, b| read_at(&path, o, b), interp_base) {
        Ok(p) => p,
        // Not there at all, as `load_program` says it.
        Err(_) if read_at(&path, 0, &mut [0u8; 1]).is_none() => {
            return Err(LoadError::NeedsInterpreter(path));
        }
        Err(e) => return Err(e),
    };
    let ibase = interp_place(&exe_image, interp_base, interp.brk.saturating_sub(interp_base), arena);
    let (interp, interp_ranges) = if ibase == interp_base {
        (interp, interp_ranges)
    } else {
        crate::elf::plan(&|o, b| read_at(&path, o, b), ibase)?
    };
    check_overlap(&exe_image, &interp)?;
    ranges.extend(interp_ranges);
    Ok((Program { exe: exe_image, interp: Some(interp), darwin: None }, ranges))
}

/// Two independently placed images must not claim the same addresses.
fn check_overlap(exe: &crate::elf::Image, interp: &crate::elf::Image) -> Result<(), LoadError> {
    for a in &exe.segments {
        for b in &interp.segments {
            let a_end = a.vaddr + a.mem_size;
            let b_end = b.vaddr + b.mem_size;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `execve`'s plan places what `load_program` places, from the files.
    #[test]
    fn a_plan_is_the_program_without_its_bytes() {
        let exe = minimal_elf(3, Some("/interp"));
        let interp = minimal_elf(3, None);
        let file = |path: &str| match path {
            "/exe" => Some(exe.clone()),
            "/interp" => Some(interp.clone()),
            _ => None,
        };
        let read_at = |path: &str, off: u64, out: &mut [u8]| {
            let f = file(path)?;
            let from = (off as usize).min(f.len());
            let n = out.len().min(f.len() - from);
            out[..n].copy_from_slice(&f[from..from + n]);
            Some(n)
        };
        let loaded = load_program(&exe, 0x2000_0000, 0x3000_0000, &mut no_arena, &|p| file(p)).unwrap();
        let (planned, ranges) = plan_program("/exe", 0x2000_0000, 0x3000_0000, &mut no_arena, &read_at).unwrap();
        assert_eq!(planned.entry(), loaded.entry());
        assert_eq!(planned.brk(), loaded.brk());
        assert_eq!(planned.exe.phdr_vaddr, loaded.exe.phdr_vaddr);
        assert_eq!(planned.interp_base(), loaded.interp_base());
        let segs: Vec<_> = loaded.placements().collect();
        let plan: Vec<_> = planned.placements().collect();
        assert_eq!(segs.len(), plan.len());
        assert_eq!(ranges.len(), plan.len());
        let files = [&exe, &interp];
        for (i, ((l, lf), (p, pf))) in segs.iter().zip(&plan).enumerate() {
            assert_eq!((l.vaddr, l.mem_size, lf), (p.vaddr, p.mem_size, pf));
            assert!(p.bytes.is_empty());
            let f = files[usize::from(pf.is_some())];
            let (o, n) = ranges[i];
            assert_eq!(&f[o as usize..(o + n) as usize], &l.bytes[..]);
        }
        let missing = plan_program("/nope", 0x2000_0000, 0x3000_0000, &mut no_arena, &read_at);
        assert!(missing.is_err());
    }

    #[test]
    fn a_static_program_needs_no_interpreter() {
        // A minimal ET_EXEC header with one PT_LOAD.
        let img = minimal_elf(2, None);
        let p = load_program(&img, 0x2000_0000, 0x3000_0000, &mut no_arena, &|_| None).unwrap();
        assert!(p.interp.is_none());
        assert_eq!(p.entry(), p.exe.entry);
        assert_eq!(p.interp_base(), 0);
    }

    #[test]
    fn a_missing_interpreter_is_reported_by_name() {
        let img = minimal_elf(3, Some("/lib/ld-musl-aarch64.so.1"));
        let err = load_program(&img, 0x2000_0000, 0x3000_0000, &mut no_arena, &|_| None).unwrap_err();
        assert_eq!(
            err,
            LoadError::NeedsInterpreter("/lib/ld-musl-aarch64.so.1".into())
        );
    }

    #[test]
    fn the_interpreter_is_entered_not_the_program() {
        let exe = minimal_elf(3, Some("/interp"));
        let interp = minimal_elf(3, None);
        let p = load_program(&exe, 0x2000_0000, 0x3000_0000, &mut no_arena, &|path| {
            (path == "/interp").then(|| interp.clone())
        })
        .unwrap();
        assert!(p.interp.is_some());
        assert_eq!(p.interp_base(), 0x3000_0000);
        // Entry is the interpreter's, and it is placed at its own base.
        assert_eq!(p.entry(), 0x3000_0000 + 0x1000);
        assert_ne!(p.entry(), p.exe.entry);
        // Both images' segments must be placed.
        assert_eq!(p.segments().count(), 2);
    }

    /// A position-independent executable too big for the space below the
    /// interpreter goes in the arena, with its break after it; the
    /// interpreter and the stack stay where they are. One that fits stays at
    /// `PIE_BASE`.
    #[test]
    fn a_pie_too_big_for_the_fixed_layout_goes_in_the_arena() {
        let room = gex_abi::INTERP_BASE - gex_abi::PIE_BASE;
        let interp = minimal_elf(3, None);
        let read = |path: &str| (path == "/interp").then(|| interp.clone());
        for (memsz, dynamic) in [(96u64 << 20, true), (96 << 20, false), (room, true), (room + 0x1000, false)] {
            let exe = sized_elf(3, dynamic.then_some("/interp"), 0, memsz);
            let mut asked = Vec::new();
            let mut arena = |len: u64| {
                asked.push(len);
                gex_abi::exe_bias(len)
            };
            let p = load_program(&exe, gex_abi::PIE_BASE, gex_abi::INTERP_BASE, &mut arena, &read).unwrap();
            let fits = gex_abi::exe_fits(memsz);
            let want = if fits { gex_abi::PIE_BASE } else { gex_abi::arena_bottom() };
            assert_eq!(p.exe.bias, want, "{memsz:#x}");
            assert_eq!(asked, if fits { vec![] } else { vec![memsz] });
            assert_eq!(p.exe.brk, want + memsz);
            assert_eq!(gex_abi::brk_limit(p.exe.brk), if fits { gex_abi::BRK_TOP } else { gex_abi::MMAP_TOP });
            if dynamic {
                assert_eq!(p.interp_base(), gex_abi::INTERP_BASE);
            }
            let read_at = |path: &str, off: u64, out: &mut [u8]| {
                let f = if path == "/exe" { exe.clone() } else { read(path)? };
                read_slice(&f, off, out)
            };
            let (planned, _) = plan_program("/exe", gex_abi::PIE_BASE, gex_abi::INTERP_BASE, &mut fresh_arena(), &read_at).unwrap();
            assert_eq!((planned.exe.bias, planned.entry()), (p.exe.bias, p.entry()));
        }
    }

    /// A fixed-address executable that reaches the interpreter's place has
    /// its interpreter put in the arena; one that reaches the stack is
    /// refused, saying so.
    #[test]
    fn a_fixed_image_moves_its_interpreter_and_never_reaches_the_stack() {
        let interp = minimal_elf(3, None);
        let read = |path: &str| (path == "/interp").then(|| interp.clone());
        let base = gex_abi::EXEC_BASE;
        // Into the interpreter's place, short of the stack.
        let exe = sized_elf(2, Some("/interp"), base, gex_abi::INTERP_BASE + 0x40_0000 - base);
        let p = load_program(&exe, gex_abi::PIE_BASE, gex_abi::INTERP_BASE, &mut fresh_arena(), &read).unwrap();
        assert_eq!(p.interp_base(), gex_abi::arena_bottom());
        assert_eq!(p.exe.bias, 0);
        assert_eq!(gex_abi::brk_limit(p.exe.brk), gex_abi::STACK_TOP - gex_abi::STACK_SIZE);
        // A static one there needs nothing moved.
        let exe = sized_elf(2, None, base, gex_abi::INTERP_BASE + 0x40_0000 - base);
        assert!(load_program(&exe, gex_abi::PIE_BASE, gex_abi::INTERP_BASE, &mut no_arena, &read).is_ok());
        // Over the stack: refused, static or not, and planned the same.
        for dynamic in [false, true] {
            let exe = sized_elf(2, dynamic.then_some("/interp"), base, 96 << 20);
            let err = load_program(&exe, gex_abi::PIE_BASE, gex_abi::INTERP_BASE, &mut fresh_arena(), &read).unwrap_err();
            assert_eq!(err, LoadError::ReachesStack { start: base, end: base + (96 << 20) });
            assert!(err.to_string().contains("reaches gex's stack"), "{err}");
            let read_at = |path: &str, off: u64, out: &mut [u8]| {
                let f = if path == "/exe" { exe.clone() } else { read(path)? };
                read_slice(&f, off, out)
            };
            let planned = plan_program("/exe", gex_abi::PIE_BASE, gex_abi::INTERP_BASE, &mut fresh_arena(), &read_at);
            assert_eq!(planned.unwrap_err(), err);
        }
    }

    /// A tiny but valid aarch64 ELF: one `PT_LOAD`, optionally a `PT_INTERP`.
    fn minimal_elf(e_type: u16, interp: Option<&str>) -> Vec<u8> {
        // An ET_EXEC sits at its own address, which must be above gex's
        // reserved low region; an ET_DYN is linked at zero.
        let base = if e_type == 2 { gex_abi::EXEC_BASE } else { 0 };
        sized_elf(e_type, interp, base, 0x2000)
    }

    /// [`minimal_elf`] linked at `base`, its one `PT_LOAD` `memsz` long.
    fn sized_elf(e_type: u16, interp: Option<&str>, base: u64, memsz: u64) -> Vec<u8> {
        let phnum = if interp.is_some() { 2 } else { 1 };
        let phoff = 64usize;
        let interp_off = phoff + phnum * 56;
        let mut v = vec![0u8; 0x2000];
        v[0..4].copy_from_slice(b"\x7fELF");
        v[4] = 2; // ELFCLASS64
        v[5] = 1; // little endian
        v[6] = 1; // version
        v[16..18].copy_from_slice(&e_type.to_le_bytes());
        v[18..20].copy_from_slice(&gex_abi::ELF_MACHINE.to_le_bytes());
        v[24..32].copy_from_slice(&(base + 0x1000).to_le_bytes()); // e_entry
        v[32..40].copy_from_slice(&(phoff as u64).to_le_bytes());
        v[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
        v[56..58].copy_from_slice(&(phnum as u16).to_le_bytes());

        // PT_LOAD, executable, covering the whole file.
        let p = phoff;
        v[p..p + 4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
        v[p + 4..p + 8].copy_from_slice(&5u32.to_le_bytes()); // PF_R | PF_X
        v[p + 8..p + 16].copy_from_slice(&0u64.to_le_bytes()); // p_offset
        v[p + 16..p + 24].copy_from_slice(&base.to_le_bytes()); // p_vaddr
        v[p + 32..p + 40].copy_from_slice(&0x2000u64.to_le_bytes()); // p_filesz
        v[p + 40..p + 48].copy_from_slice(&memsz.to_le_bytes()); // p_memsz

        if let Some(path) = interp {
            let p = phoff + 56;
            v[p..p + 4].copy_from_slice(&3u32.to_le_bytes()); // PT_INTERP
            v[p + 8..p + 16].copy_from_slice(&(interp_off as u64).to_le_bytes());
            v[p + 32..p + 40].copy_from_slice(&((path.len() + 1) as u64).to_le_bytes());
            v[interp_off..interp_off + path.len()].copy_from_slice(path.as_bytes());
        }
        v
    }
}
