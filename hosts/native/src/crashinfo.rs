//! What a guest's fatal fault says of where it was: on a signal that ends
//! the process (SIGSEGV, SIGBUS, SIGILL, SIGABRT... with no handler, or one
//! that put the default back), one block on stderr with the signal, the fault
//! address, the guest pc, and for the pc, the link register (A64) or the word
//! at the stack pointer (x86-64) and a backtrace by frame pointer: each
//! address as the loaded image that holds it, the offset in the image, and
//! the nearest symbol before it. A handled signal says nothing.
//!
//! On when `GEX_CRASH=1`; off when `GEX_CRASH=0`; with it unset, on only when
//! stderr is a terminal (a person at a shell; a script that captures stderr
//! keeps what it always had).
//!
//! Addresses are the guest's own: a Darwin process sees them in the window
//! (`gex_abi::guest_addr`), while the kernel's memory map keeps offsets, so
//! the images are found by offset and printed in the window. Symbols come
//! from the image's file (read through the guest's namespace): an ELF's
//! `.dynsym` and `.symtab`, a Mach-O's symbol table and function starts.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::IsTerminal;

use gex_kernel::mem::{prot, Vma};

/// Frames of the backtrace, the first included.
const FRAMES: usize = 16;

thread_local! {
    /// The address the fault this thread is about to die of was at, as the
    /// host saw it (a guest offset), for [`on_end`] to say.
    static FAULT: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// Whether a fatal signal is reported (see the module's text).
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| match std::env::var("GEX_CRASH").as_deref() {
        Ok("0") => false,
        Ok(v) if !v.is_empty() => true,
        _ => std::io::stderr().is_terminal(),
    })
}

/// The fault the calling thread is about to end the process with was at
/// `addr` (a guest offset: what the host's trap gives).
pub fn note_fault(addr: u64) {
    FAULT.with(|f| f.set(Some(addr)));
}

/// What `on_end` takes of the thread's fault note, if there is one.
pub fn take_fault() -> Option<u64> {
    FAULT.with(|f| f.take())
}

/// The registers the report starts from.
pub struct Regs {
    pub pc: u64,
    /// The link register (A64); x86-64 has none.
    pub lr: Option<u64>,
    pub sp: u64,
    pub fp: u64,
}

/// The guest memory a report reads: words, and the file of an image.
pub trait Source {
    fn word(&mut self, addr: u64) -> Option<u64>;
    fn file(&mut self, path: &str) -> Option<Vec<u8>>;
}

/// An image: where it is, and what it names.
struct Image {
    /// The address of its first byte as the map has it less the first
    /// segment's own address: what is added to a file's addresses.
    bias: u64,
    /// The image's address in the map (its first mapping's start), where
    /// "the offset in the image" counts from.
    base: u64,
    /// Sorted by address, unbiased.
    syms: Vec<(u64, u64, String)>,
    /// Where the file has code, unbiased.
    text: Vec<(u64, u64)>,
}

/// What a file says of its addresses: the lowest it asks for, its functions
/// and where its code is; `None` for a file that is neither an ELF nor a
/// Mach-O (or too broken to read).
fn parse(bytes: &[u8]) -> Option<(u64, Vec<(u64, u64, String)>, Vec<(u64, u64)>)> {
    let mut syms: Vec<(u64, u64, String)> = Vec::new();
    let low;
    let text: Vec<(u64, u64)>;
    if gex_ld::macho::is_macho(bytes) {
        let (m, found, _slice) = gex_ld::macho::file_functions(bytes)?;
        low = m.mapped().map(|s| s.vmaddr).min()?;
        syms.extend(found.into_iter().map(|s| (s.value, s.size, s.name)));
        text = m.mapped().filter(|s| s.initprot & 4 != 0).map(|s| (s.vmaddr, s.vmaddr + s.vmsize)).collect();
    } else {
        let img = gex_ld::elf::load(bytes, 0).ok()?;
        low = img.segments.iter().map(|s| s.vaddr & !0xfff).min()?;
        text = img.segments.iter().filter(|s| s.executable).map(|s| (s.vaddr, s.vaddr + s.mem_size.max(s.bytes.len() as u64))).collect();
        let in_text = |a: u64| text.iter().any(|&(lo, hi)| a >= lo && a < hi);
        for d in &img.dynsyms {
            // (A dynamic symbol does not say it is a function: where code is says so.)
            if !d.undefined && !d.tls && d.value != 0 && !d.name.is_empty() && in_text(d.value) {
                syms.push((d.value, d.size, d.name.clone()));
            }
        }
        for s in &img.symbols {
            if s.value != 0 && !s.ifunc && !s.name.is_empty() {
                syms.push((s.value, s.size, s.name.clone()));
            }
        }
    }
    syms.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    syms.dedup_by(|b, a| a.0 == b.0);
    Some((low, syms, text))
}

struct Resolver<'a> {
    vmas: &'a [Vma],
    /// The program's own path: what the mappings the loader called `[image]` are of.
    exe: &'a str,
    /// The program's dynamic linker, which the loader places beside it and
    /// names `[image]` as well, at `gex_abi::INTERP_BASE` and above.
    interp: Option<String>,
    src: &'a mut dyn Source,
    images: HashMap<String, Option<Image>>,
}

/// An address as the report says it.
struct Where {
    /// Whether the file has code there, when it could be read.
    code: Option<bool>,
    path: String,
    off: u64,
    sym: Option<(String, u64)>,
}

impl<'a> Resolver<'a> {
    fn vma(&self, a: u64) -> Option<&'a Vma> {
        self.vmas.iter().find(|v| a >= v.start && a < v.end)
    }

    /// The path of the file a mapping is of: its name when that is one, else
    /// what it was loaded or built from.
    fn path_of<'s>(&'s self, v: &'s Vma) -> Option<&'s str> {
        if v.name.starts_with('/') {
            return Some(&v.name);
        }
        if v.name == "[image]" {
            if let (Some(i), true) = (&self.interp, v.start >= gex_abi::INTERP_BASE) {
                return Some(i);
            }
            if self.exe.starts_with('/') {
                return Some(self.exe);
            }
        }
        v.code.as_ref().or(v.origin.as_ref()).map(|(p, _)| p.as_str()).filter(|p| p.starts_with('/'))
    }

    /// `a`, a guest offset, as an image, an offset and a symbol.
    fn locate(&mut self, a: u64) -> Option<Where> {
        let v = self.vma(a)?;
        let path = self.path_of(v)?.to_string();
        if !self.images.contains_key(&path) {
            let first = self.vmas.iter().filter(|w| self.path_of(w) == Some(path.as_str())).map(|w| w.start).min();
            let img = (|| {
                let bytes = self.src.file(&path)?;
                let (low, syms, text) = parse(&bytes)?;
                let base = first?;
                Some(Image { bias: base.wrapping_sub(low), base, syms, text })
            })();
            self.images.insert(path.clone(), img);
        }
        let img = self.images.get(&path)?.as_ref();
        let Some(img) = img else {
            // An image whose file cannot be read: still its path, and the
            // offset from its first mapping.
            let base = self.vmas.iter().filter(|w| self.path_of(w) == Some(path.as_str())).map(|w| w.start).min()?;
            return Some(Where { code: None, path, off: a - base, sym: None });
        };
        let unbiased = a.wrapping_sub(img.bias);
        let i = img.syms.partition_point(|s| s.0 <= unbiased);
        let sym = i.checked_sub(1).map(|i| &img.syms[i]).map(|s| (s.2.clone(), unbiased - s.0));
        let code = Some(img.text.iter().any(|&(lo, hi)| unbiased >= lo && unbiased < hi));
        Some(Where { code, path, off: a - img.base, sym })
    }
}

/// One address as a report line's tail: `path+0xoff  symbol+0xdist`, or what
/// is known of it.
fn describe(r: &mut Resolver<'_>, a: u64) -> String {
    match r.locate(gex_abi::canon(a)) {
        Some(w) => {
            let mut s = format!("{}+{:#x}", w.path, w.off);
            if let Some((name, dist)) = w.sym {
                let _ = write!(s, "  {name}+{dist:#x}");
            }
            s
        }
        None => match r.vma(gex_abi::canon(a)) {
            Some(v) if !v.name.is_empty() => format!("{} (no image)", v.name),
            Some(_) => "(anonymous memory)".to_string(),
            None => "(unmapped)".to_string(),
        },
    }
}

/// The block for a fatal `signal` (a Linux number) at `fault`, a guest
/// address as the guest sees it, if the signal has one.
pub fn render(label: &str, fault: Option<(u64, &str)>, regs: &Regs, vmas: &[Vma], exe: &str, src: &mut dyn Source) -> String {
    let interp = if vmas.iter().any(|v| v.name == "[image]" && v.start >= gex_abi::INTERP_BASE) && exe.starts_with('/') {
        src.file(exe).and_then(|b| {
            if gex_ld::macho::is_macho(&b) {
                gex_ld::macho::file_functions(&b).and_then(|(m, ..)| m.dylinker)
            } else {
                gex_ld::elf::load(&b, 0).ok().and_then(|i| i.interp)
            }
        })
    } else {
        None
    };
    let mut r = Resolver { vmas, exe, interp, src, images: HashMap::new() };
    let mut out = String::new();
    let _ = write!(out, "gex: fatal {label}");
    if let Some((a, note)) = fault {
        let _ = write!(out, " at fault address {a:#x}{note}");
    }
    out.push('\n');
    let pc = gex_abi::guest_addr(regs.pc);
    let line = |out: &mut String, name: &str, a: u64, r: &mut Resolver<'_>| {
        let _ = writeln!(out, "gex:   {name:<4} {a:#012x}  {}", describe(r, a));
    };
    line(&mut out, "pc", pc, &mut r);
    let exec = |r: &Resolver<'_>, a: u64| r.vma(gex_abi::canon(a)).is_some_and(|v| v.prot & prot::EXEC != 0);
    match regs.lr {
        Some(lr) if lr != 0 => line(&mut out, "lr", lr, &mut r),
        Some(_) => {}
        None => {
            // x86-64: the word at the stack pointer is the return address
            // of a function that has not pushed anything yet (a fault at
            // its first instructions), and is something else past that.
            if let Some(w) = r.src.word(regs.sp).filter(|&w| exec(&r, w)) {
                line(&mut out, "[sp]", w, &mut r);
            }
        }
    }
    // The frame pointer's chain: [fp] the caller's fp, [fp+8] the return
    // address (A64 and x86-64 push them as a pair alike). It ends at the
    // first thing that does not look like a frame.
    let (mut fp, mut n) = (regs.fp, 0);
    let mut last = 0;
    while n < FRAMES && fp != 0 && fp % 8 == 0 && fp > last {
        let c = gex_abi::canon(fp);
        if !r.vma(c).is_some_and(|v| v.prot & (prot::READ | prot::WRITE) == prot::READ | prot::WRITE) {
            break;
        }
        let (Some(next), Some(ra)) = (r.src.word(fp), r.src.word(fp + 8)) else { break };
        // A return address in the code of an image (past the frames of code
        // built without frame pointers, the chain leads into data).
        if ra == 0 || !exec(&r, ra) || r.locate(gex_abi::canon(ra)).is_some_and(|w| w.code == Some(false)) {
            break;
        }
        if n == 0 {
            let _ = writeln!(out, "gex:   backtrace by frame pointer:");
        }
        let _ = writeln!(out, "gex:   #{n:<3} {ra:#012x}  {}", describe(&mut r, ra));
        n += 1;
        last = fp;
        fp = next;
    }
    // The context holds what the translated code last wrote back: the entry
    // of the last block that did, not the instruction. A leaf function
    // called from there faults one frame below what pc says.
    let _ = writeln!(out, "gex:   (pc is the last block entry the translator wrote back: the fault is in it, or in a leaf it called)");
    out
}

/// A Linux signal's name, for the block's first line.
fn name_of(signal: i32) -> String {
    let n = gex_kernel::proc::signal_name(signal as u32);
    if n == "a signal" {
        format!("signal {signal}")
    } else {
        n.to_string()
    }
}

/// Whether the fault address is one the host could not keep: the translated
/// access went past the guest memory and was steered into its last page, so
/// what the guest had in the register is not what the trap saw.
fn steered(offset: u64) -> bool {
    !gex_abi::mem64() && offset >= 0xffff_0000
}

/// The thread of a store ends its process with `signal` at `pc`: say where,
/// when it is a crash and the report is on. `fault` is the address
/// [`note_fault`] had, if it did.
pub fn on_end(signal: i32, ctx_regs: Regs, fault: Option<u64>, vmas: &[Vma], exe: &str, src: &mut dyn Source, pid: i32, tid: i32) {
    let label = format!("{} (pid {pid}, thread {tid})", name_of(signal));
    let fault = fault.map(|o| (gex_abi::guest_addr(o), if steered(o) { " (outside the guest's memory: the access was steered here, the pointer's own value is not kept)" } else { "" }));
    let text = render(&label, fault, &ctx_regs, vmas, exe, src);
    // One write: threads that die together do not interleave their blocks.
    eprint!("{text}");
}
