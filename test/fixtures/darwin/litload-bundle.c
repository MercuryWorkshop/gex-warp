// The bundle litload.c loads (test/darwin/litload.test.js): globals it
// writes from one function and reads back from another, through a load at
// a pc-relative constant address. On AArch64 that is `ldr x0, sym` (LDR
// literal), which ld64 makes of an `adrp`+`ldr` pair whose target is near
// enough: the test rewrites each `adrp` here into a `nop` and its `ldr`
// into the literal load, as Ruby's fiddle.bundle has them. On x86-64 a
// rip-relative `mov` is the same thing as the assembler writes it.
//
// lit_common is zero-filled (__DATA,__common, -fcommon), lit_data starts
// as 7 in the file (__DATA,__data): neither may read back as it was when
// the code was translated.

long lit_common;
long lit_data = 7;

void lit_set(long common, long data) {
    lit_common = common;
    lit_data = data;
}

// lit_load_common, lit_load_data: the global alone. lit_swap: a store, then
// the literal load of the same global, in one block.
#if defined(__aarch64__)
__asm__(
    ".text\n"
    ".globl _lit_load_common\n"
    ".p2align 2\n"
    "_lit_load_common:\n"
    "  adrp x8, _lit_common@PAGE\n"
    "  ldr x0, [x8, _lit_common@PAGEOFF]\n"
    "  ret\n"
    ".globl _lit_load_data\n"
    ".p2align 2\n"
    "_lit_load_data:\n"
    "  adrp x8, _lit_data@PAGE\n"
    "  ldr x0, [x8, _lit_data@PAGEOFF]\n"
    "  ret\n"
    ".globl _lit_swap\n"
    ".p2align 2\n"
    "_lit_swap:\n"
    "  adrp x9, _lit_common@PAGE\n"
    "  add x9, x9, _lit_common@PAGEOFF\n"
    "  str x0, [x9]\n"
    "  adrp x8, _lit_common@PAGE\n"
    "  ldr x0, [x8, _lit_common@PAGEOFF]\n"
    "  ret\n");
#else
__asm__(
    ".text\n"
    ".globl _lit_load_common\n"
    "_lit_load_common:\n"
    "  movq _lit_common(%rip), %rax\n"
    "  ret\n"
    ".globl _lit_load_data\n"
    "_lit_load_data:\n"
    "  movq _lit_data(%rip), %rax\n"
    "  ret\n"
    ".globl _lit_swap\n"
    "_lit_swap:\n"
    "  movq %rdi, _lit_common(%rip)\n"
    "  movq _lit_common(%rip), %rax\n"
    "  ret\n");
#endif
