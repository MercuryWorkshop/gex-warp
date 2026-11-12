# A Mac's address space: the Darwin window

On a 64-bit Mac the first 4 GiB of every process is `__PAGEZERO`. Swift on
both processors takes every value below 4 GiB for a spare bit pattern of a
reference (`SWIFT_ABI_DARWIN_*_LEAST_VALID_POINTER`): an `Optional`'s `nil`,
compiled into every Xcode-built program's value witnesses as well as into the
runtime. Under gex every guest address was below 4 GiB, so unspecialized code
read a live reference as `nil`, skipped its retain and over-released it.

## The design

A Darwin process keeps the memory, the layout and the offsets it had. What
changes is the address the guest is given for an offset `o`:
`o + DARWIN_BASE` (4 GiB), `gex_abi::guest_addr`. Its addresses are in the
window `[4 GiB, 8 GiB)`.

- **Inside gex an address stays the offset**: the kernel's memory map, the
  loader's placement, the dispatch directory, a block's identity and every
  pc gex keeps (ctx::PC, call returns, restartable ranges), AOT and cache
  keys, the page's image and closure lookups, recorded sessions. None of them
  changes, and nothing of a Linux process does: its base is 0 and both
  conversions are the identity.
- **It becomes the guest's where the guest sees it**: the lifters' values
  computed from the pc (A64 `adr`/`adrp` and the link register of `bl`/`blr`;
  x86-64 RIP-relative addresses and the return address `call` pushes), the
  initial stack (sp, argv/envp/apple pointers, the main executable's header,
  `main_stack=`), what `mmap`, `mach_vm_allocate`/`mach_vm_map`/`remap`,
  `mach_vm_region` and the deferred-reclamation ring answer, a work queue
  thread's stack, a signal frame's pc and fault address.
- **It is the offset again where the guest hands one in**
  (`gex_abi::canon`): every translated access, every kernel copy
  (`GuestMem`), the mapping calls (`MapIo`), the memory map's lookups
  (`vma_at`, `check_access`), futex keys, an indirect branch's target (one
  `and 0xffffffff` in the IR, `indirect_target`), a thread's start pc, a
  handler's trampoline, `sigreturn`'s pc, a host cache operation's address.

A translated access already took an i64 address to a wasm32 offset by its low
32 bits, behind a guard `a <u GUEST_LIMIT`; the window only raises the guard's
limit to `DARWIN_BASE + GUEST_LIMIT` (`gex_abi::windowed_limit`), so the
lowering costs nothing: the same compare and `i32.wrap_i64`, a constant
address folded to its low half. The C units' guard (`gex-cgen`) is the same
text with the same macro. The window is lenient: an address below it is its
own offset, as before, so what gex's userland was built to read by constant
(the commpage at `0xffc000`) is still there, and a value the conversion
missed still reaches the right bytes. Where a pc in the window reaches a
dispatch unconverted, the hosts' miss handlers resume at its offset.

Which processes get it: a Mach-O program gex places (position-independent),
or one whose `__PAGEZERO` is a Mac's 4 GiB (`gex_abi::darwin_windowed`,
`gex_ld::macho::Darwin::windowed`). One linked to run below 4 GiB with a
small zero page runs where it is linked, with no window, as a Mac runs it.
The base is set with the Darwin personality (`gex_abi::set_guest_base`):
natively per host process, on the page per process (its shim, and the
`gex_compat` bit a child's and thread's Workers adopt). It is in the
translation key (`gex_emit::tier1::knobs`), and in the C units' header.

## What is not done

- A memory64 guest memory (the x86-64 page's `?mem64=1`) has no window: its
  accesses take the guest address as it stands, and a window there would
  cost a subtraction in every one. Its Darwin processes keep their addresses
  below 4 GiB.
- The commpage is still at gex's address below 4 GiB (pyrus's headers name
  `0xffc000`); it is reachable in the window too (`0x1_00ffc000`), and a
  userland rebuilt with that address would have nothing below 4 GiB at all.
- `/dev/gex-gl`'s rings carry the guest's own pointers, which the device
  reads as offsets; no Darwin userland uses the device.
