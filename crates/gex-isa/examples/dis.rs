//! dis WORD...: what the decoder makes of each instruction word (hex), and
//! whether the slow path (`gex_isa::slow`) would carry it out.
fn main() {
    for a in std::env::args().skip(1) {
        let raw = u32::from_str_radix(a.trim_start_matches("0x"), 16).expect("a hex word");
        let inst = gex_isa::decode(raw, 0);
        let slow = if gex_isa::slow::known(raw) { "  (the slow path's)" } else { "" };
        println!("{raw:08x}  {}{slow}", gex_isa::fmt::disasm(&inst, 0));
    }
}
