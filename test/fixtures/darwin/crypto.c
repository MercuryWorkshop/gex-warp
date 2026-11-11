// The instructions every Apple silicon Mac has and a program for one uses
// without asking -- Go's runtime hashes with AES, and takes PMULL, SHA-1 and
// SHA-256 as given: AES (aese, aesd, aesmc, aesimc), the polynomial
// multiplies (pmull and pmull2, of doublewords and of bytes), SHA-1, SHA-256,
// SHA-512, SHA-3's eor3, bcax, rax1 and xar, JavaScript's conversion
// (fjcvtzs) with the flag it leaves, and the flag manipulations (cfinv, rmif,
// setf8, setf16, axflag, xaflag). Each on the same pseudo-random operands:
// the first results in full, then a sum over many, against the Mac.
#include <arm_neon.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define T __attribute__((target("aes,sha2,sha3,v8.5a,jsconv"), noinline))

static uint64_t seed = 0x9e3779b97f4a7c15ull;
static uint64_t rnd(void) {
    seed ^= seed << 13;
    seed ^= seed >> 7;
    seed ^= seed << 17;
    return seed;
}
typedef struct { uint64_t lo, hi; } v128;
static v128 rv(void) { v128 v = { rnd(), rnd() }; return v; }
static uint8x16_t b(v128 v) { return vreinterpretq_u8_u64(vcombine_u64(vcreate_u64(v.lo), vcreate_u64(v.hi))); }
static uint32x4_t w(v128 v) { return vreinterpretq_u32_u8(b(v)); }
static uint64x2_t d(v128 v) { return vreinterpretq_u64_u8(b(v)); }
static v128 ob(uint8x16_t x) { v128 v = { vgetq_lane_u64(vreinterpretq_u64_u8(x), 0), vgetq_lane_u64(vreinterpretq_u64_u8(x), 1) }; return v; }
static v128 ow(uint32x4_t x) { return ob(vreinterpretq_u8_u32(x)); }
static v128 od(uint64x2_t x) { return ob(vreinterpretq_u8_u64(x)); }
static v128 o16(uint16x8_t x) { return ob(vreinterpretq_u8_u16(x)); }
static v128 one(uint64_t x) { v128 v = { x, 0 }; return v; }
static v128 op(poly128_t x) { return ob(vreinterpretq_u8_p128(x)); }

#define OPS(X) \
    X(aese, ob(vaeseq_u8(b(x), b(y)))) \
    X(aesd, ob(vaesdq_u8(b(x), b(y)))) \
    X(aesmc, ob(vaesmcq_u8(b(x)))) \
    X(aesimc, ob(vaesimcq_u8(b(x)))) \
    X(pmull, op(vmull_p64((poly64_t)x.lo, (poly64_t)y.lo))) \
    X(pmull2, op(vmull_high_p64(vreinterpretq_p64_u8(b(x)), vreinterpretq_p64_u8(b(y))))) \
    X(pmull_bytes, o16(vreinterpretq_u16_p16(vmull_p8(vget_low_p8(vreinterpretq_p8_u8(b(x))), vget_low_p8(vreinterpretq_p8_u8(b(y))))))) \
    X(pmull2_bytes, o16(vreinterpretq_u16_p16(vmull_high_p8(vreinterpretq_p8_u8(b(x)), vreinterpretq_p8_u8(b(y)))))) \
    X(sha1c, ow(vsha1cq_u32(w(x), (uint32_t)y.lo, w(z)))) \
    X(sha1p, ow(vsha1pq_u32(w(x), (uint32_t)y.lo, w(z)))) \
    X(sha1m, ow(vsha1mq_u32(w(x), (uint32_t)y.lo, w(z)))) \
    X(sha1h, one(vsha1h_u32((uint32_t)x.lo))) \
    X(sha1su0, ow(vsha1su0q_u32(w(x), w(y), w(z)))) \
    X(sha1su1, ow(vsha1su1q_u32(w(x), w(y)))) \
    X(sha256h, ow(vsha256hq_u32(w(x), w(y), w(z)))) \
    X(sha256h2, ow(vsha256h2q_u32(w(x), w(y), w(z)))) \
    X(sha256su0, ow(vsha256su0q_u32(w(x), w(y)))) \
    X(sha256su1, ow(vsha256su1q_u32(w(x), w(y), w(z)))) \
    X(sha512h, od(vsha512hq_u64(d(x), d(y), d(z)))) \
    X(sha512h2, od(vsha512h2q_u64(d(x), d(y), d(z)))) \
    X(sha512su0, od(vsha512su0q_u64(d(x), d(y)))) \
    X(sha512su1, od(vsha512su1q_u64(d(x), d(y), d(z)))) \
    X(eor3, ob(veor3q_u8(b(x), b(y), b(z)))) \
    X(bcax, ob(vbcaxq_u8(b(x), b(y), b(z)))) \
    X(rax1, od(vrax1q_u64(d(x), d(y)))) \
    X(xar_0, od(vxarq_u64(d(x), d(y), 0))) \
    X(xar_1, od(vxarq_u64(d(x), d(y), 1))) \
    X(xar_19, od(vxarq_u64(d(x), d(y), 19))) \
    X(xar_32, od(vxarq_u64(d(x), d(y), 32))) \
    X(xar_63, od(vxarq_u64(d(x), d(y), 63)))

#define FN(name, expr) T static v128 f_##name(v128 x, v128 y, v128 z) { (void)y; (void)z; return expr; }
OPS(FN)

// fjcvtzs and the flags after it: the result below, NZCV in the top four bits above.
T static uint64_t jcvt(double v) {
    uint64_t r, f;
    __asm__ volatile("fjcvtzs %w0, %d2\n\tmrs %1, nzcv" : "=&r"(r), "=&r"(f) : "w"(v) : "cc");
    return (f >> 28) << 32 | (uint32_t)r;
}

// FEAT_FlagM and FEAT_FlagM2: NZCV after each, from a given NZCV.
#define FLAGS(name, insn) \
    T static uint64_t name(uint64_t nzcv, uint64_t x) { \
        uint64_t f; \
        __asm__ volatile("msr nzcv, %1\n\t" insn "\n\tmrs %0, nzcv" : "=&r"(f) : "r"(nzcv << 28), "r"(x) : "cc"); \
        return f >> 28; \
    }
FLAGS(cfinv, "cfinv") FLAGS(axflag, "axflag") FLAGS(xaflag, "xaflag")
FLAGS(setf8, "setf8 %w2") FLAGS(setf16, "setf16 %w2")
FLAGS(rmif_0_f, "rmif %2, #0, #15") FLAGS(rmif_3_5, "rmif %2, #3, #5") FLAGS(rmif_62_a, "rmif %2, #62, #10") FLAGS(rmif_17_0, "rmif %2, #17, #0")

static const double special[] = {
    0.0, -0.0, 1.0, -1.0, 0.5, -0.5, 1.5, -1.5, 0.9999999999999999, 2147483647.0, 2147483648.0, -2147483648.0, -2147483649.0,
    2147483647.5, 4294967295.0, 4294967296.0, 4294967297.0, -4294967297.0, 9007199254740993.0, 1e15, -1e15, 1e19, -1e19, 1e30, -1e300,
    18446744073709551616.0, 9223372036854775808.0, -9223372036854775808.0, 4.9e-324, 2.2250738585072014e-308, 1.7976931348623157e308,
    __builtin_inf(), -__builtin_inf(), __builtin_nan(""), 123456789.0, -123456789.25, 6442450944.0, 3.0e9, -3.0e9,
};

int main(void) {
#define RUN(name, expr) { \
        seed = 0x9e3779b97f4a7c15ull; \
        uint64_t sum = 0; \
        for (int i = 0; i < 3000; i++) { \
            v128 x = rv(), y = rv(), z = rv(); \
            v128 r = f_##name(x, y, z); \
            if (i < 3) printf(#name " %d: %016llx%016llx\n", i, (unsigned long long)r.hi, (unsigned long long)r.lo); \
            sum = (sum << 7 | sum >> 57) ^ r.lo ^ (r.hi * 0x100000001b3ull); \
        } \
        printf(#name " sum: %016llx\n", (unsigned long long)sum); \
    }
    OPS(RUN)

    for (unsigned i = 0; i < sizeof special / sizeof *special; i++)
        printf("fjcvtzs %a: %09llx\n", special[i], (unsigned long long)jcvt(special[i]));
    seed = 0x9e3779b97f4a7c15ull;
    uint64_t sum = 0;
    for (int i = 0; i < 20000; i++) {
        uint64_t bits = rnd();
        // Half of them of a size where the answer is not zero.
        if (i & 1) bits = (bits & 0x800fffffffffffffull) | (uint64_t)(1023 - 4 + rnd() % 100) << 52;
        double v;
        memcpy(&v, &bits, 8);
        sum = (sum << 7 | sum >> 57) ^ jcvt(v);
    }
    printf("fjcvtzs sum: %016llx\n", (unsigned long long)sum);
#define SHOWF(name) { \
        printf(#name ":"); \
        seed = 0x9e3779b97f4a7c15ull; \
        for (uint64_t f = 0; f < 16; f++) printf(" %llx", (unsigned long long)name(f, rnd() >> (f * 4 % 64))); \
        printf("\n"); \
    }
    SHOWF(cfinv) SHOWF(axflag) SHOWF(xaflag) SHOWF(setf8) SHOWF(setf16) SHOWF(rmif_0_f) SHOWF(rmif_3_5) SHOWF(rmif_62_a) SHOWF(rmif_17_0)
    printf("all of it: done\n");
    return 0;
}
