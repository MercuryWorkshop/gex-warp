//! Page write tracking (`gex_abi::set_write_map`, `gex_emit::wtrack`).
//!
//! Off, tier-1's output is byte-identical to a gex without the feature: the
//! digests below were taken from the emitter before it existed. On, every kind
//! of guest write a translation makes marks the pages it wrote, and a write
//! across a page boundary marks both, run in wasmtime.

use gex_ir::*;
use std::collections::BTreeSet;

/// A block with every kind of guest write the IR has, at the addresses in
/// `x0` (scalar stores), `x1` (vector stores), `x2` (atomics), `x3` (bulk).
fn writes_block(start: u64, target: u64) -> Block {
    let mut b = Builder::new(start);
    let a0 = b.read(GuestReg::X(0));
    let a1 = b.read(GuestReg::X(1));
    let a2 = b.read(GuestReg::X(2));
    let a3 = b.read(GuestReg::X(3));
    let v = b.read(GuestReg::X(4));
    let v32 = b.def(Expr::Wrap(v));
    for (k, sz) in [MemSz::B1, MemSz::B2, MemSz::B4, MemSz::B8].into_iter().enumerate() {
        let (val, _ty) = if sz == MemSz::B8 { (v, Ty::I64) } else { (v32, Ty::I32) };
        b.stmt(Stmt::Store { addr: a0, offset: 8 * k as i64, val, sz, order: Order::Relaxed });
    }
    // A pair store is two stores, the second at +8.
    b.stmt(Stmt::Store { addr: a0, offset: 0x40, val: v, sz: MemSz::B8, order: Order::Relaxed });
    b.stmt(Stmt::Store { addr: a0, offset: 0x48, val: v, sz: MemSz::B8, order: Order::Relaxed });
    // Release store.
    b.stmt(Stmt::Store { addr: a0, offset: 0x50, val: v, sz: MemSz::B8, order: Order::SeqCst });
    let vec = b.def(Expr::VSplat { a: v, lane_bytes: 8 });
    for (k, bytes) in [16u32, 8, 4, 2, 1].into_iter().enumerate() {
        b.stmt(Stmt::VStore { addr: a1, offset: 16 * k as i64, val: vec, bytes });
    }
    let old = b.def(Expr::Rmw { op: RmwOp::Add, addr: a2, val: v, sz: MemSz::B8, ty: Ty::I64 });
    b.write(GuestReg::X(5), old);
    let a2b = b.const_i64(0x10);
    let a2b = b.bin(BinOp::Add, Ty::I64, a2, a2b);
    let old32 = b.def(Expr::Wrap(old));
    let old2 = b.def(Expr::CmpXchg { addr: a2b, expected: v32, replacement: old32, sz: MemSz::B4, ty: Ty::I32 });
    let old2 = b.def(Expr::Extend { a: old2, signed: false });
    b.write(GuestReg::X(6), old2);
    let len = b.read(GuestReg::X(7));
    b.stmt(Stmt::MemFill { dst: a3, val: v, len });
    let src = b.read(GuestReg::X(8));
    b.stmt(Stmt::MemCopy { dst: a3, src, len });
    b.finish(Term::Goto { target })
}

fn module_of(blocks: Vec<Block>) -> Vec<u8> {
    let mut m = gex_emit::ModuleBuilder::new(0x1000, 0x1000);
    for b in blocks {
        m.add_block(b);
    }
    m.finish().wasm
}

fn module_bytes() -> Vec<u8> {
    // A block that falls through, and one that loops on itself (a region).
    module_of(vec![writes_block(0x1000, 0x1010), writes_block(0x1010, 0x1010)])
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// The digests of `module_bytes` from the emitter before write tracking
/// existed: (wasm32, memory64). The memory64 one is of a module whose
/// dispatch tables are at 8 GiB, where they have been since the wasm32 one
/// was taken (it names the directory's address; the wasm32 one does not).
const BEFORE: (u64, u64) = (0x26fc_412b_ca1f_47c6, 0x302e_1a07_cb15_324d);

/// Where the tests put the map.
const MAP: u64 = 0x0200_0000;

/// One block whose statements `body` writes, run from the registers `regs`
/// (x0..), in a memory of this flavour; the pages the map has marked after.
fn marked_by(regs: &[u64], body: impl Fn(&mut Builder, &[Value])) -> BTreeSet<u64> {
    let mut b = Builder::new(0x1000);
    let vals: Vec<Value> = (0..regs.len() as u8).map(|n| b.read(GuestReg::X(n))).collect();
    body(&mut b, &vals);
    // To nowhere: dispatch misses, and the test's miss stops the run.
    let block = b.finish(Term::Goto { target: 0x5000 });
    let wasm = module_of(vec![block]);
    gex_native::validate_module(&wasm).expect("the module validates");

    let mut config = wasmtime::Config::new();
    config.wasm_threads(true).wasm_memory64(true).shared_memory(true);
    // A shared memory is a static allocation: the reservation holds all of it (gex-native's engine).
    if gex_abi::mem64() {
        config.memory_reservation(gex_abi::LIMIT64);
    }
    let engine = wasmtime::Engine::new(&config).unwrap();
    let module = wasmtime::Module::new(&engine, &wasm).unwrap();
    let mut store = wasmtime::Store::new(&engine, ());
    let mut linker = wasmtime::Linker::new(&engine);
    let mut memory = None;
    for i in module.imports() {
        if let wasmtime::ExternType::Memory(mt) = i.ty() {
            // All of it: an image module writes its dispatch directory, at the
            // top of a memory64.
            let max = mt.maximum().unwrap_or(mt.minimum());
            let mt = if mt.is_64() { wasmtime::MemoryTypeBuilder::new().memory64(true).shared(true).min(max).max(Some(max)).build().unwrap() } else { mt };
            let m = wasmtime::SharedMemory::new(&engine, mt).unwrap();
            linker.define(&store, i.module(), i.name(), m.clone()).unwrap();
            memory = Some(m);
        }
    }
    linker.define_unknown_imports_as_traps(&module).unwrap();
    let memory = memory.expect("the module imports its memory");
    let data = memory.data();
    let put = |at: u64, v: u64| {
        for (k, byte) in v.to_le_bytes().into_iter().enumerate() {
            // SAFETY: this thread is the only one using the memory.
            unsafe { *data[at as usize + k].get() = byte };
        }
    };
    let ctx = gex_abi::ctx_addr(0);
    for (n, &v) in regs.iter().enumerate() {
        put(ctx + gex_abi::ctx::x(n as u8) as u64, v);
    }
    let inst = linker.instantiate(&mut store, &module).unwrap();
    let enter = inst.get_typed_func::<(i32, i64), ()>(&mut store, "enter").unwrap();
    // The miss at the end traps; what the block wrote is in memory by then.
    let _ = enter.call(&mut store, (ctx as i32, 0x1000));
    let mut pages = BTreeSet::new();
    for p in 0..gex_abi::write_map_bytes() {
        // SAFETY: as above.
        if unsafe { *data[(MAP + p) as usize].get() } != 0 {
            pages.insert(p);
        }
    }
    pages
}

fn set(pages: &[u64]) -> BTreeSet<u64> {
    pages.iter().copied().collect()
}

/// Every kind of write marks exactly the pages it wrote, both where it
/// crosses a page boundary.
fn check_marks() {
    let st = |sz: MemSz, order: Order| {
        move |b: &mut Builder, r: &[Value]| {
            let v = if sz == MemSz::B8 { r[1] } else { b.def(Expr::Wrap(r[1])) };
            b.stmt(Stmt::Store { addr: r[0], offset: 0, val: v, sz, order });
        }
    };
    // A plain store, each width; a store across a page boundary marks both.
    assert_eq!(marked_by(&[0x0300_0010, 7], st(MemSz::B1, Order::Relaxed)), set(&[0x3000]));
    assert_eq!(marked_by(&[0x0300_0ffe, 7], st(MemSz::B2, Order::Relaxed)), set(&[0x3000]));
    assert_eq!(marked_by(&[0x0300_0ffe, 7], st(MemSz::B4, Order::Relaxed)), set(&[0x3000, 0x3001]));
    assert_eq!(marked_by(&[0x0300_0ffc, 7], st(MemSz::B8, Order::Relaxed)), set(&[0x3000, 0x3001]));
    // A store with a static offset: the page of the effective address.
    assert_eq!(
        marked_by(&[0x0300_0ff0, 7], |b, r| b.stmt(Stmt::Store { addr: r[0], offset: 0x2010, val: r[1], sz: MemSz::B8, order: Order::Relaxed })),
        set(&[0x3003])
    );
    // A release store (an atomic one).
    assert_eq!(marked_by(&[0x0300_4ff8, 7], st(MemSz::B8, Order::SeqCst)), set(&[0x3004]));
    // A pair store (stp): two stores, the second on the next page.
    assert_eq!(
        marked_by(&[0x0300_5ff8, 7], |b, r| {
            b.stmt(Stmt::Store { addr: r[0], offset: 0, val: r[1], sz: MemSz::B8, order: Order::Relaxed });
            b.stmt(Stmt::Store { addr: r[0], offset: 8, val: r[1], sz: MemSz::B8, order: Order::Relaxed });
        }),
        set(&[0x3005, 0x3006])
    );
    // SIMD stores: a whole q register across a page boundary, and a lane.
    assert_eq!(
        marked_by(&[0x0300_7ff8, 7], |b, r| {
            let v = b.def(Expr::VSplat { a: r[1], lane_bytes: 8 });
            b.stmt(Stmt::VStore { addr: r[0], offset: 0, val: v, bytes: 16 });
        }),
        set(&[0x3007, 0x3008])
    );
    assert_eq!(
        marked_by(&[0x0300_9100, 7], |b, r| {
            let v = b.def(Expr::VSplat { a: r[1], lane_bytes: 8 });
            b.stmt(Stmt::VStore { addr: r[0], offset: 0, val: v, bytes: 4 });
        }),
        set(&[0x3009])
    );
    // Atomics: a read-modify-write (ldadd, swp) and a compare-and-swap (cas,
    // and stxr, which the lifter makes one), each aligned on its page.
    assert_eq!(
        marked_by(&[0x0300_aff8, 7], |b, r| {
            let o = b.def(Expr::Rmw { op: RmwOp::Add, addr: r[0], val: r[1], sz: MemSz::B8, ty: Ty::I64 });
            b.write(GuestReg::X(9), o);
        }),
        set(&[0x300a])
    );
    assert_eq!(
        marked_by(&[0x0300_b004, 7], |b, r| {
            let v = b.def(Expr::Wrap(r[1]));
            let o = b.def(Expr::CmpXchg { addr: r[0], expected: v, replacement: v, sz: MemSz::B4, ty: Ty::I32 });
            let o = b.def(Expr::Extend { a: o, signed: false });
            b.write(GuestReg::X(9), o);
        }),
        set(&[0x300b])
    );
    // Bulk writes (a memset or memcpy the translator recognised): every page.
    assert_eq!(
        marked_by(&[0x0300_c800, 7, 0x2001], |b, r| b.stmt(Stmt::MemFill { dst: r[0], val: r[1], len: r[2] })),
        set(&[0x300c, 0x300d, 0x300e])
    );
    assert_eq!(
        marked_by(&[0x0300_fff8, 0x0310_0000, 0x10], |b, r| b.stmt(Stmt::MemCopy { dst: r[0], src: r[1], len: r[2] })),
        set(&[0x300f, 0x3010])
    );
    // Nothing written, nothing marked.
    assert_eq!(marked_by(&[0x0301_1000, 7, 0], |b, r| b.stmt(Stmt::MemFill { dst: r[0], val: r[1], len: r[2] })), set(&[]));

    // Every start in the last 16 bytes of a page, including the exact-fit
    // boundary. Separate four-page spans make an extra or missing mark
    // visible. Offsets include a carry from the base's low bits and a
    // negative offset (emit_address must fold that into the kept address).
    let bases: &[u64] = if gex_abi::mem64() { &[0x0302_0ff8, 0xffff_fff8, 0x1_0302_0ff8] } else { &[0x0302_0ff8] };
    for &base in bases {
        for shift in [0i64, 0x2010, -0x2010] {
            for bytes in [1u32, 2, 4, 8, 16] {
                let mut expected = BTreeSet::new();
                let offsets: Vec<i64> = (-16i64..=1).enumerate().map(|(i, delta)| {
                    let at = (base & !0xfff) + 0x10000 + i as u64 * 0x4000 + 0x1000;
                    let at = at.wrapping_add_signed(delta);
                    expected.extend(at >> 12..=(at + bytes as u64 - 1) >> 12);
                    at as i64 - base as i64 - shift
                }).collect();
                let got = marked_by(&[base.wrapping_add_signed(shift), 7], |b, r| {
                    let v = b.def(Expr::VSplat { a: r[1], lane_bytes: 8 });
                    for &offset in &offsets {
                        if bytes == 16 {
                            b.stmt(Stmt::VStore { addr: r[0], offset, val: v, bytes });
                        } else {
                            let sz = match bytes { 1 => MemSz::B1, 2 => MemSz::B2, 4 => MemSz::B4, _ => MemSz::B8 };
                            let val = if bytes == 8 { r[1] } else { b.def(Expr::Wrap(r[1])) };
                            b.stmt(Stmt::Store { addr: r[0], offset, val, sz, order: Order::Relaxed });
                        }
                    }
                });
                assert_eq!(got, expected, "base {base:#x}, shift {shift}, width {bytes}");
            }
        }
    }
    // Negative static displacement, and a real 4 GiB crossing in memory64.
    assert_eq!(marked_by(&[0x0302_1004, 7], |b, r| {
        b.stmt(Stmt::Store { addr: r[0], offset: -8, val: r[1], sz: MemSz::B8, order: Order::Relaxed });
    }), set(&[0x3020, 0x3021]));
    if gex_abi::mem64() {
        assert_eq!(marked_by(&[0xffff_fffc, 7], st(MemSz::B8, Order::Relaxed)), set(&[0xfffff, 0x100000]));
        assert_eq!(marked_by(&[0xffff_fff8, 7], |b, r| {
            b.stmt(Stmt::Store { addr: r[0], offset: 16, val: r[1], sz: MemSz::B8, order: Order::Relaxed });
        }), set(&[0x100000]));
    }
    // A store that traps before completing must not leave a mark, even
    // when the first byte alone would have been inside the memory.
    let end = if gex_abi::mem64() { gex_abi::LIMIT64 } else { gex_abi::GUEST_MEM_INITIAL_BYTES };
    assert_eq!(marked_by(&[end - 1, 7], st(MemSz::B8, Order::Relaxed)), set(&[]));
}

/// One test, in order: the flavour and the map are process-wide.
#[test]
fn write_tracking() {
    gex_abi::set_write_map(0);
    gex_abi::set_mem_flavor(gex_abi::MemFlavor::M32);
    // Off: the bytes of before.
    let m32 = module_bytes();
    gex_native::validate_module(&m32).expect("the wasm32 module validates");
    assert_eq!(fnv(&m32), BEFORE.0, "tier-1's wasm32 output changed with write tracking off");
    gex_abi::set_mem_flavor(gex_abi::MemFlavor::M64);
    let m64 = module_bytes();
    gex_native::validate_module(&m64).expect("the memory64 module validates");
    assert_eq!(fnv(&m64), BEFORE.1, "tier-1's memory64 output changed with write tracking off");

    // On: memory64, then wasm32.
    gex_abi::set_write_map(MAP);
    let on64 = module_bytes();
    gex_native::validate_module(&on64).expect("the memory64 module with marks validates");
    assert_ne!(fnv(&on64), BEFORE.1);
    check_marks();
    gex_abi::set_mem_flavor(gex_abi::MemFlavor::M32);
    gex_native::validate_module(&module_bytes()).expect("the wasm32 module with marks validates");
    check_marks();
}

/// What the marks cost guest code: a loop of `stores` 8-byte stores and
/// `alu` loads and adds an iteration, run in wasmtime (Cranelift) with
/// tracking off and on. Run separately from `write_tracking`: the emitter's
/// module IDs and flavour are process-wide and its off digests assume a fresh
/// process. `cargo test --release -p gex-native --test write_track store_cost
/// -- --ignored --exact --nocapture`. Set GEX_WRITE_BENCH_DIR to export the
/// loops for `hosts/browser/js/bench-write-track.mjs` (V8, timed after warmup).
#[test]
#[ignore]
fn store_cost() {
    gex_abi::set_mem_flavor(gex_abi::MemFlavor::M64);
    const N: u64 = 20_000_000;
    let lp = |stores: usize, alu: usize| {
        let mut b = Builder::new(0x1000);
        let base = b.read(GuestReg::X(0));
        let i = b.read(GuestReg::X(1));
        let mask = b.const_i64(0x3fff);
        let k = b.bin(BinOp::And, Ty::I64, i, mask);
        let six = b.const_i64(6);
        let off = b.bin(BinOp::Shl, Ty::I64, k, six);
        let a = b.bin(BinOp::Add, Ty::I64, base, off);
        let mut acc = b.read(GuestReg::X(2));
        for n in 0..alu {
            let v = b.def(Expr::Load { addr: a, offset: 8 * (n % 8) as i64, sz: MemSz::B8, signed: false, ty: Ty::I64, order: Order::Relaxed });
            acc = b.bin(BinOp::Add, Ty::I64, acc, v);
        }
        for n in 0..stores {
            b.stmt(Stmt::Store { addr: a, offset: 8 * n as i64, val: acc, sz: MemSz::B8, order: Order::Relaxed });
        }
        b.write(GuestReg::X(2), acc);
        let one = b.const_i64(1);
        let i2 = b.bin(BinOp::Sub, Ty::I64, i, one);
        b.write(GuestReg::X(1), i2);
        let zero = b.const_i64(0);
        let more = b.cmp(CmpOp::Ne, Ty::I64, i2, zero);
        b.finish(Term::Branch { cond: more, taken: 0x1000, not_taken: 0x5000 })
    };
    for (stores, alu) in [(8, 0), (1, 6)] {
        let mut t = [0f64; 2];
        for (k, on) in [false, true].into_iter().enumerate() {
            if on { gex_abi::set_write_map(MAP); } else { gex_abi::set_write_map(0); }
            if let Some(dir) = std::env::var_os("GEX_WRITE_BENCH_DIR") {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(std::path::Path::new(&dir).join(format!("stores-{stores}-alu-{alu}-{}.wasm", if on { "on" } else { "off" })), module_of(vec![lp(stores, alu)])).unwrap();
            }
            let best = (0..3).map(|_| {
                let start = std::time::Instant::now();
                let _ = run_counted(lp(stores, alu), N);
                start.elapsed().as_secs_f64()
            }).fold(f64::MAX, f64::min);
            t[k] = best;
        }
        eprintln!("{stores} stores + {alu} loads/adds an iteration: off {:.2} ns, on {:.2} ns an iteration (+{:.0}%)",
            t[0] * 1e9 / N as f64, t[1] * 1e9 / N as f64, (t[1] / t[0] - 1.0) * 100.0);
    }
}

/// Run a loop block from x0 = 0x0300_0000, x1 = `n` to its exit.
fn run_counted(block: Block, n: u64) -> () {
    let wasm = module_of(vec![block]);
    let mut config = wasmtime::Config::new();
    config.wasm_threads(true).wasm_memory64(true).shared_memory(true).memory_reservation(gex_abi::LIMIT64);
    let engine = wasmtime::Engine::new(&config).unwrap();
    let module = wasmtime::Module::new(&engine, &wasm).unwrap();
    let mut store = wasmtime::Store::new(&engine, ());
    let mut linker = wasmtime::Linker::new(&engine);
    let mut memory = None;
    for i in module.imports() {
        if let wasmtime::ExternType::Memory(mt) = i.ty() {
            let max = mt.maximum().unwrap_or(mt.minimum());
            let mt = wasmtime::MemoryTypeBuilder::new().memory64(true).shared(true).min(max).max(Some(max)).build().unwrap();
            let m = wasmtime::SharedMemory::new(&engine, mt).unwrap();
            linker.define(&store, i.module(), i.name(), m.clone()).unwrap();
            memory = Some(m);
        }
    }
    linker.func_wrap("gex", "safepoint", |_: i32, _: i64| {}).unwrap();
    linker.define_unknown_imports_as_traps(&module).unwrap();
    let memory = memory.unwrap();
    let data = memory.data();
    let ctx = gex_abi::ctx_addr(0);
    let put = |at: u64, v: u64| for (k, byte) in v.to_le_bytes().into_iter().enumerate() { unsafe { *data[at as usize + k].get() = byte } };
    put(ctx + gex_abi::ctx::x(0) as u64, 0x0300_0000);
    put(ctx + gex_abi::ctx::x(1) as u64, n);
    let inst = linker.instantiate(&mut store, &module).unwrap();
    let enter = inst.get_typed_func::<(i32, i64), ()>(&mut store, "enter").unwrap();
    let _ = enter.call(&mut store, (ctx as i32, 0x1000));
}
