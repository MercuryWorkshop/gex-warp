//! unknown FILE OFFSET SIZE...: the words of a file's code (a Mach-O's or an
//! ELF's text, by file offset and size) this decoder has no instruction for,
//! each with how often it occurs, most frequent first -- masked to the
//! instruction's class (register fields cleared) and with one example. For
//! finding what real programs use that is not implemented yet. A word the slow
//! path carries out (`gex_isa::slow`) is implemented, and is not counted;
//! `unknown --slow FILE OFFSET SIZE...` counts those instead: what runs, but
//! at the cost of a host call each time.
use std::collections::HashMap;

fn main() {
    let mut a: Vec<String> = std::env::args().collect();
    let slow = a.get(1).is_some_and(|x| x == "--slow");
    if slow {
        a.remove(1);
    }
    if a.len() < 4 || (a.len() - 2) % 2 != 0 {
        eprintln!("usage: unknown FILE OFFSET SIZE [OFFSET SIZE]...");
        std::process::exit(2);
    }
    let bytes = std::fs::read(&a[1]).expect("reading the file");
    let num = |s: &str| if let Some(h) = s.strip_prefix("0x") { u64::from_str_radix(h, 16) } else { s.parse() }.expect("a number") as usize;
    let mut seen: HashMap<u32, (u64, u32)> = HashMap::new();
    let mut words = 0u64;
    for pair in a[2..].chunks(2) {
        let (off, size) = (num(&pair[0]), num(&pair[1]));
        for at in (off..(off + size).min(bytes.len().saturating_sub(3))).step_by(4) {
            let raw = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            words += 1;
            if matches!(gex_isa::decode(raw, 0), gex_isa::Inst::Unknown { .. }) && gex_isa::slow::known(raw) == slow {
                // The register fields (Rd, Rn, and Rm where it is one) out of the way.
                let e = seen.entry(raw & 0xFFE0_FC00).or_insert((0, raw));
                e.0 += 1;
            }
        }
    }
    let mut all: Vec<_> = seen.into_iter().collect();
    all.sort_by(|x, y| y.1 .0.cmp(&x.1 .0).then(x.0.cmp(&y.0)));
    let total: u64 = all.iter().map(|(_, (n, _))| n).sum();
    println!("{words} words, {total} {} in {} classes", if slow { "by the slow path" } else { "unknown" }, all.len());
    for (class, (n, example)) in all {
        println!("{n:8} {class:08x} e.g. {example:08x}");
    }
}
