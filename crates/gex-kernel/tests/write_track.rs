//! Page write tracking (`gex_abi::set_write_map`): what the kernel and the
//! shims write into guest memory through `GuestMem` marks its pages, as a
//! translated store does, among them DC ZVA's block (the shims zero it and
//! mark it with `GuestMem::written`). Its own process: the map is global.

use gex_kernel::GuestMem;

const MAP: u64 = 0x10_0000;

fn marked(mem: &[u8]) -> Vec<u64> {
    (0..gex_abi::write_map_bytes()).filter(|&p| mem[(MAP + p) as usize] != 0).collect()
}

#[test]
fn guest_mem_writes_mark_their_pages() {
    let mut mem = vec![0u8; 0x100_0000];
    // Off: nothing is marked.
    GuestMem::new(&mut mem).write(0x80_0ffc, &[1; 8]).unwrap();
    assert!(marked(&mem).is_empty());

    gex_abi::set_write_map(MAP);
    let mut gm = GuestMem::new(&mut mem);
    gm.write(0x80_0ffc, &[1; 8]).unwrap(); // across a page boundary
    gm.zero(0x80_3000, 0x2001).unwrap();
    gm.copy_within(0x80_0000, 0x80_6ff0, 0x20).unwrap();
    gm.zero_dirty(0x80_9000, 0x10).unwrap();
    // DC ZVA: the shims zero the 64-byte block and mark it.
    let block = gex_abi::DCZID_BLOCK_BYTES;
    gm.read_mut(0x80_a040, block as usize).unwrap().fill(0);
    gm.written(0x80_a040, block);
    // A read into a guest buffer: the caller fills a `read_mut` slice, then marks what it filled.
    gm.read_mut(0x80_b000, 16).unwrap().fill(7);
    gm.written(0x80_b000, 16);
    // Nothing written, nothing marked.
    gm.written(0x80_c000, 0);
    assert_eq!(marked(&mem), vec![0x800, 0x801, 0x803, 0x804, 0x805, 0x806, 0x807, 0x809, 0x80a, 0x80b]);
}
