//! Page write tracking: every guest store a translation makes also marks the
//! 4 KiB page it wrote ([`gex_abi::WRITE_PAGE_SHIFT`]), in a byte map the
//! program's runtime set up before anything was translated
//! ([`gex_abi::set_write_map`]). A runtime that keeps a copy of guest memory
//! elsewhere (a GPU buffer, a texture) reads and clears a page's mark where it
//! would otherwise compare the page with what it last took.
//!
//! Off unless a runtime turns it on; off, nothing here emits a byte, and every
//! tier's output is what it was without the feature.
//!
//! The mark is `map[ea >> 12] = 1`, on the effective address the access
//! took, and the same for its last byte's page only when the access crosses
//! a page. A low-bits check keeps the second release store off the common
//! path without reading the map. It comes after the access, not before: a
//! consumer clears a mark and then reads the page, so a store that lands between the two marks
//! the page again and is seen at the next look, where a mark made before the
//! store could be cleared, and the page read, before the store landed. An
//! access that traps marks nothing. A compare-and-swap that fails, and a
//! store-exclusive sent to the scratch word, mark a page they did not write:
//! a mark may be spurious, never missing.

use crate::wasm::{FuncBody, ValType};

/// Whether translations mark the pages they write.
#[inline]
pub fn on() -> bool {
    gex_abi::write_map().is_some()
}

/// The map's address.
fn map() -> u64 {
    gex_abi::write_map().expect("write tracking is on")
}

const I64_ADD: u8 = 0x7c;
const I64_SUB: u8 = 0x7d;
const I64_SHR_U: u8 = 0x88;
const I64_NE: u8 = 0x52;
const I64_EXTEND_I32_U: u8 = 0xad;
const I32_WRAP_I64: u8 = 0xa7;
const I32_ATOMIC_STORE8: u32 = 0x19;
const MEMORY_FILL: u32 = 11;

/// The type of a local that keeps an address: the memory's index type.
pub fn addr_type() -> ValType {
    if gex_abi::mem64() { ValType::I64 } else { ValType::I32 }
}

/// `local` (the memory's index type) as an i64.
fn get_i64(f: &mut FuncBody, local: u32) {
    f.local_get(local);
    if !gex_abi::mem64() {
        f.byte(I64_EXTEND_I32_U);
    }
}

/// The i64 on the stack as the memory's index type.
fn to_index(f: &mut FuncBody) {
    if !gex_abi::mem64() {
        f.byte(I32_WRAP_I64);
    }
}

/// `(local + k) >> 12`, an i64.
fn page_of(f: &mut FuncBody, local: u32, k: u64) {
    get_i64(f, local);
    if k != 0 {
        f.i64_const(k as i64);
        f.byte(I64_ADD);
    }
    f.i64_const(gex_abi::WRITE_PAGE_SHIFT as i64);
    f.byte(I64_SHR_U);
}

/// `map[page] = 1`, for the i64 page index on the stack. The map's address
/// rides in the memarg: one store, an atomic one, so that on a weakly
/// ordered host (AArch64) the guest's store is visible before its mark is: a
/// consumer that sees the mark and then reads the page reads the store. A
/// plain byte store was cheaper by a tenth of the mark (`store_cost`).
fn mark_top(f: &mut FuncBody) {
    to_index(f);
    f.i32_const(1);
    f.byte(crate::wasm::op::ATOMIC_PREFIX);
    f.u32(I32_ATOMIC_STORE8);
    f.u32(0); // align
    crate::wasm::write_u64(&mut f.code, map());
}

/// After an access of `bytes` bytes at `addr + static_off`, where `addr` is
/// the local the access's address was kept in ([`addr_type`]): mark its first
/// page, and its last only when it crosses a page boundary.
pub fn mark_access(f: &mut FuncBody, addr: u32, static_off: u32, bytes: u32) {
    let page_bytes = 1u32 << gex_abi::WRITE_PAGE_SHIFT;
    debug_assert!(bytes > 0 && bytes <= page_bytes);
    page_of(f, addr, static_off as u64);
    mark_top(f);
    if bytes > 1 {
        // Only the low page bits decide whether the access crosses a page:
        // ((addr + static_off) & (page_bytes - 1)) > page_bytes - bytes.
        // i32 arithmetic is enough even above 4 GiB, and its wrapping add
        // preserves these bits. The last page still uses the full address.
        f.local_get(addr);
        if gex_abi::mem64() {
            f.byte(I32_WRAP_I64);
        }
        if static_off & (page_bytes - 1) != 0 {
            f.i32_const((static_off & (page_bytes - 1)) as i32);
            f.byte(0x6a); // i32.add
        }
        f.i32_const((page_bytes - 1) as i32);
        f.byte(0x71); // i32.and
        f.i32_const((page_bytes - bytes) as i32);
        f.byte(0x4b); // i32.gt_u
        f.if_void();
        page_of(f, addr, static_off as u64 + bytes as u64 - 1);
        mark_top(f);
        f.end();
    }
}

/// After a bulk write of `len` bytes from `dst`: every page of it, one
/// `memory.fill` over the map. `dst` is an i32 local when `dst32`, else an
/// i64 one; `len` an i64 local; `scratch` an i64 local this overwrites.
pub fn mark_range(f: &mut FuncBody, dst: u32, dst32: bool, len: u32, scratch: u32) {
    let get_dst = |f: &mut FuncBody| {
        f.local_get(dst);
        if dst32 {
            f.byte(I64_EXTEND_I32_U);
        }
    };
    get_dst(f);
    f.i64_const(gex_abi::WRITE_PAGE_SHIFT as i64);
    f.byte(I64_SHR_U);
    f.local_set(scratch);
    // Where: the first page's byte of the map.
    f.local_get(scratch);
    f.i64_const(map() as i64);
    f.byte(I64_ADD);
    to_index(f);
    // What.
    f.i32_const(1);
    // How many: ((dst + len - 1) >> 12) - first + 1, and none for no bytes.
    get_dst(f);
    f.local_get(len);
    f.byte(I64_ADD);
    f.i64_const(1);
    f.byte(I64_SUB);
    f.i64_const(gex_abi::WRITE_PAGE_SHIFT as i64);
    f.byte(I64_SHR_U);
    f.local_get(scratch);
    f.byte(I64_SUB);
    f.i64_const(1);
    f.byte(I64_ADD);
    f.i64_const(0);
    f.local_get(len);
    f.i64_const(0);
    f.byte(I64_NE);
    f.select();
    to_index(f);
    f.byte(crate::wasm::op::MISC_PREFIX);
    f.u32(MEMORY_FILL);
    f.byte(0);
}
