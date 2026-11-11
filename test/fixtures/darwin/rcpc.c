// FEAT_LRCPC2's loads and stores, as a compiler for Apple silicon emits them
// for an atomic field: LDAPUR and its byte, halfword, word and sign-extending
// forms, STLUR and its forms, at unscaled offsets from -256 to 255 (gex had
// no decoding for them: ripgrep stopped at its first search). What each
// reads and writes, against the Mac. (Each access lies within 16 bytes
// aligned so: an ordered access that crosses such a line is a bus error on
// Apple silicon, FEAT_LSE2's rule.)
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#if defined(__aarch64__)
#define LD(name, insn, reg, off) \
    static uint64_t name(const void *p) { \
        uint64_t r = 0xdeadbeefdeadbeefull; \
        __asm__ volatile(insn " %" reg "0, [%1, #" #off "]" : "+r"(r) : "r"(p) : "memory"); \
        return r; \
    }
#define ST(name, insn, reg, off) \
    static void name(void *p, uint64_t v) { __asm__ volatile(insn " %" reg "1, [%0, #" #off "]" : : "r"(p), "r"(v) : "memory"); }

#define LOADS(off, tag) \
    LD(ub_##tag, "ldapurb", "w", off) LD(uh_##tag, "ldapurh", "w", off) LD(uw_##tag, "ldapur", "w", off) LD(ux_##tag, "ldapur", "x", off) \
    LD(sbw_##tag, "ldapursb", "w", off) LD(sbx_##tag, "ldapursb", "x", off) LD(shw_##tag, "ldapursh", "w", off) LD(shx_##tag, "ldapursh", "x", off) \
    LD(swx_##tag, "ldapursw", "x", off)
LOADS(-256, m256) LOADS(-8, m8) LOADS(0, z) LOADS(8, p8) LOADS(248, p248)
// And where only a byte fits: the odd offsets.
LD(ub_m3, "ldapurb", "w", -3) LD(sbx_p1, "ldapursb", "x", 1) LD(ub_p255, "ldapurb", "w", 255) LD(sbw_m255, "ldapursb", "w", -255)

#define SHOW(off, tag) \
    printf("at %4d: b %llx h %llx w %llx x %llx  sb %llx %llx sh %llx %llx sw %llx\n", off, \
           (unsigned long long)ub_##tag(p), (unsigned long long)uh_##tag(p), (unsigned long long)uw_##tag(p), (unsigned long long)ux_##tag(p), \
           (unsigned long long)sbw_##tag(p), (unsigned long long)sbx_##tag(p), (unsigned long long)shw_##tag(p), (unsigned long long)shx_##tag(p), \
           (unsigned long long)swx_##tag(p));

ST(stb_m255, "stlurb", "w", -255) ST(sth_m6, "stlurh", "w", -6) ST(stw_p4, "stlur", "w", 4) ST(stx_p248, "stlur", "x", 248)
ST(stx_m256, "stlur", "x", -256) ST(stb_z, "stlurb", "w", 0)

static void bytes(const char *what, const unsigned char *b, int n) {
    printf("%s:", what);
    for (int i = 0; i < n; i++) printf(" %02x", b[i]);
    printf("\n");
}

int main(void) {
    static _Alignas(16) unsigned char buf[608];
    for (int i = 0; i < 608; i++) buf[i] = (unsigned char)(i * 37 + 0x8b);
    const unsigned char *p = buf + 304;
    SHOW(-256, m256) SHOW(-8, m8) SHOW(0, z) SHOW(8, p8) SHOW(248, p248)
    printf("bytes: at -3 %llx, at 1 %llx, at 255 %llx, at -255 %llx\n", (unsigned long long)ub_m3(p), (unsigned long long)sbx_p1(p),
           (unsigned long long)ub_p255(p), (unsigned long long)sbw_m255(p));
    unsigned char *q = buf + 304;
    stb_m255(q, 0x1122334455667788ull);
    sth_m6(q, 0x1122334455667788ull);
    stw_p4(q, 0x1122334455667788ull);
    stx_p248(q, 0x1122334455667788ull);
    stx_m256(q, 0xa1a2a3a4a5a6a7a8ull);
    stb_z(q, 0xfe);
    bytes("around -256", buf + 304 - 258, 12);
    bytes("around -6", buf + 304 - 8, 6);
    bytes("around 0", buf + 304 - 1, 3);
    bytes("around 4", buf + 304 + 3, 6);
    bytes("around 248", buf + 304 + 247, 10);
    unsigned long sum = 0;
    for (int i = 0; i < 608; i++) sum = sum * 31 + buf[i];
    printf("all of it: %lx\n", sum);
    return 0;
}
#else
int main(void) {
    printf("an arm64 program's\n");
    return 0;
}
#endif
