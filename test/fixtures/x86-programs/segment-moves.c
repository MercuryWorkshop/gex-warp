/* `mov` from a segment register at each width: REX.W 8C writes the whole
 * 64-bit register, the selector zero-extended (Mojave's Finder and Dock have
 * `mov %es, %rdx` among the bytes gex translates, which once made a module
 * the engine refused); 32-bit clears the upper half; 16-bit keeps the rest of
 * the register; a memory destination is two bytes, the neighbours kept. */
#include <stdint.h>
#include <stdio.h>

#define FILL 0x1122334455667788ull

#if defined(__x86_64__)
/* Spelled in bytes: the assembler drops a REX.W it need not keep. 48 8C /r,
 * the register `%rdx` (as Finder's) and `%r9` (REX.WB). */
#define SEL64(seg, n)                                                        \
    static unsigned long long seg##_q(void) {                                \
        register unsigned long long r9 __asm__("r9") = FILL;                 \
        unsigned long long r = FILL;                                         \
        __asm__ volatile(".byte 0x48, 0x8c, 0xc2 | (" #n " << 3)\n"           \
                         ".byte 0x49, 0x8c, 0xc1 | (" #n " << 3)"             \
                         : "+d"(r), "+r"(r9));                               \
        return r == r9 ? r : ~0ull;                                          \
    }
SEL64(es, 0) SEL64(cs, 1) SEL64(ss, 2) SEL64(ds, 3)
#endif

#define SEL32(seg)                                                           \
    static unsigned seg##_l(void) {                                          \
        unsigned r = 0x55667788u;                                            \
        __asm__ volatile("movl %%" #seg ", %0" : "+r"(r));                   \
        return r;                                                            \
    }                                                                        \
    static unsigned seg##_w(void) {                                          \
        unsigned r = 0x55667788u;                                            \
        __asm__ volatile("movw %%" #seg ", %w0" : "+r"(r));                  \
        return r;                                                            \
    }                                                                        \
    static uint32_t seg##_m(void) {                                          \
        volatile uint32_t r = 0x55667788u;                                   \
        __asm__ volatile("movw %%" #seg ", %0" : "=m"(*(volatile uint16_t *)&r)); \
        return r;                                                            \
    }
SEL32(es) SEL32(ds) SEL32(ss) SEL32(cs)

int main(void) {
#define SHOW(seg) printf(#seg ": l=%08x w=%08x m=%08x\n", seg##_l(), seg##_w(), seg##_m())
    SHOW(es);
    SHOW(ds);
    SHOW(ss);
    SHOW(cs);
#if defined(__x86_64__)
    printf("q: es=%016llx ds=%016llx ss=%016llx cs=%016llx\n", es_q(), ds_q(), ss_q(), cs_q());
#endif
    return 0;
}
