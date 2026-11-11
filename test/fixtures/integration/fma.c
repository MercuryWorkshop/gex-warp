/* Fused multiply-add against the hardware: fmadd/fmsub/fnmadd must round
 * exactly once. Every value is printed as its bit pattern, since an FMA
 * differs from an unfused product in the last place and `%f` would hide it.
 * Inputs cover the extremes (huge, tiny, non-finite, zeros), where the inline
 * implementation's exponent guard would show a mistake. */
#include <arm_neon.h>
#include <stdint.h>
#include <stdio.h>

/* Kept out of sight of the optimiser, so it cannot fold these at compile
 * time and leave the instruction untested. */
static volatile double dv;
static volatile float fv;

static double fma_d(double a, double b, double c) {
    double r;
    __asm__("fmadd %d0, %d1, %d2, %d3" : "=w"(r) : "w"(a), "w"(b), "w"(c));
    return r;
}
static double fmsub_d(double a, double b, double c) {
    double r;
    __asm__("fmsub %d0, %d1, %d2, %d3" : "=w"(r) : "w"(a), "w"(b), "w"(c));
    return r;
}
static double fnmadd_d(double a, double b, double c) {
    double r;
    __asm__("fnmadd %d0, %d1, %d2, %d3" : "=w"(r) : "w"(a), "w"(b), "w"(c));
    return r;
}
static float fma_f(float a, float b, float c) {
    float r;
    __asm__("fmadd %s0, %s1, %s2, %s3" : "=w"(r) : "w"(a), "w"(b), "w"(c));
    return r;
}
static float fmsub_f(float a, float b, float c) {
    float r;
    __asm__("fmsub %s0, %s1, %s2, %s3" : "=w"(r) : "w"(a), "w"(b), "w"(c));
    return r;
}

/* One three-operand vector instruction, `acc` both read and written, its
 * result stored to `out`. Written out as the instruction, since which of
 * `n` and `m` a compiler puts first is its own affair and shows in a NaN. */
#define V3(out, store, type, acc, n, m, insn) \
    do { \
        type v3_ = (acc); \
        __asm__(insn : "+w"(v3_) : "w"(n), "w"(m)); \
        store(out, v3_); \
    } while (0)

static uint64_t dbits(double x) {
    union { double d; uint64_t u; } v = {.d = x};
    return v.u;
}
static uint32_t fbits(float x) {
    union { float f; uint32_t u; } v = {.f = x};
    return v.u;
}
static double dfrom(uint64_t u) {
    union { uint64_t u; double d; } v = {.u = u};
    return v.d;
}
static float ffrom(uint32_t u) {
    union { uint32_t u; float f; } v = {.u = u};
    return v.f;
}

static uint64_t rnd(uint64_t *s) {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    return *s;
}

int main(void) {
    /* Ordinary magnitudes, where the fast path runs. */
    uint64_t s = 0x243f6a8885a308d3ULL;
    uint64_t acc = 0;
    for (int i = 0; i < 20000; i++) {
        double a = (double)(int64_t)rnd(&s) / (double)(1ULL << 40);
        double b = (double)(int64_t)rnd(&s) / (double)(1ULL << 40);
        double c = (double)(int64_t)rnd(&s) / (double)(1ULL << 40);
        dv = a;
        acc ^= dbits(fma_d(dv, b, c));
        acc = acc * 31 + dbits(fmsub_d(a, dv, c));
        acc ^= dbits(fnmadd_d(a, b, dv));
    }
    printf("double, ordinary: %016llx\n", (unsigned long long)acc);

    /* Random bit patterns, which reach the extremes the guard rejects and so
     * exercise the fallback. */
    acc = 0;
    for (int i = 0; i < 20000; i++) {
        double a = dfrom(rnd(&s));
        double b = dfrom(rnd(&s));
        double c = dfrom(rnd(&s));
        dv = a;
        acc ^= dbits(fma_d(dv, b, c));
        acc = acc * 31 + dbits(fmsub_d(a, dv, c));
    }
    printf("double, bit patterns: %016llx\n", (unsigned long long)acc);

    /* And the boundary itself: operands placed just inside and just outside
     * the exponent band, where a guard that is off by one shows up. */
    acc = 0;
    for (int e = 515; e <= 531; e++) {
        for (int f = 1515; f <= 1531; f++) {
            for (int k = 0; k < 8; k++) {
                double a = dfrom(((uint64_t)e << 52) | (rnd(&s) >> 12));
                double b = dfrom(((uint64_t)f << 52) | (rnd(&s) >> 12));
                double c = dfrom(((uint64_t)1023 << 52) | (rnd(&s) >> 12));
                dv = a;
                acc ^= dbits(fma_d(dv, b, c));
            }
        }
    }
    printf("double, band edges: %016llx\n", (unsigned long long)acc);

    /* Zeros, which the guard lets through by a special case. */
    acc = 0;
    for (int i = 0; i < 2000; i++) {
        double x = (double)(int64_t)rnd(&s) / (double)(1ULL << 40);
        dv = x;
        acc ^= dbits(fma_d(0.0, dv, x));
        acc ^= dbits(fma_d(dv, -0.0, x));
        acc ^= dbits(fma_d(dv, x, 0.0));
        acc ^= dbits(fma_d(dv, x, -0.0));
    }
    printf("double, zeros: %016llx\n", (unsigned long long)acc);

    /* Single precision, where the product is exact and only the narrowing
     * can go wrong. */
    uint32_t facc = 0;
    for (int i = 0; i < 20000; i++) {
        float a = ffrom((uint32_t)rnd(&s));
        float b = ffrom((uint32_t)rnd(&s));
        float c = ffrom((uint32_t)rnd(&s));
        fv = a;
        facc ^= fbits(fma_f(fv, b, c));
        facc = facc * 31 + fbits(fmsub_f(a, fv, c));
    }
    printf("float, bit patterns: %08x\n", facc);

    facc = 0;
    for (int i = 0; i < 20000; i++) {
        float a = (float)((int32_t)rnd(&s)) / 65536.0f;
        float b = (float)((int32_t)rnd(&s)) / 65536.0f;
        float c = (float)((int32_t)rnd(&s)) / 65536.0f;
        fv = a;
        facc ^= fbits(fma_f(fv, b, c));
        facc = facc * 31 + fbits(fmsub_f(a, fv, c));
    }
    printf("float, ordinary: %08x\n", facc);

    /* The vector forms, `fmla` and `fmls`, round once a lane as well: whole
     * registers, the two-lane single form, and the forms that take one operand
     * from a chosen lane. A compiler contracts `a*x + c*y` over two coordinates
     * into one `fmla.2d`, so this is what any affine transform is made of. */
    acc = 0;
    for (int i = 0; i < 20000; i++) {
        double a[2], b[2], c[2], r[2];
        for (int k = 0; k < 2; k++) {
            double *into[3] = {&a[k], &b[k], &c[k]};
            for (int o = 0; o < 3; o++) {
                uint64_t bits = rnd(&s);
                /* One NaN a lane at most: which of two is the answer is not this test's
                 * business. */
                if ((i + k) % 3 != o && (bits >> 52 & 0x7ff) == 0x7ff) bits &= ~(1ULL << 62);
                *into[o] = i & 1 ? dfrom(bits) : (double)(int64_t)bits / (double)(1ULL << 40);
            }
        }
        dv = a[0];
        a[0] = dv;
        float64x2_t va = vld1q_f64(a), vb = vld1q_f64(b), vc = vld1q_f64(c);
        V3(r, vst1q_f64, float64x2_t, vc, va, vb, "fmla %0.2d, %1.2d, %2.2d");
        acc = acc * 31 + (dbits(r[0]) ^ dbits(r[1]) * 3);
        V3(r, vst1q_f64, float64x2_t, vc, va, vb, "fmls %0.2d, %1.2d, %2.2d");
        acc = acc * 31 + (dbits(r[0]) ^ dbits(r[1]) * 3);
        V3(r, vst1q_f64, float64x2_t, vc, va, vb, "fmla %0.2d, %1.2d, %2.d[1]");
        acc = acc * 31 + (dbits(r[0]) ^ dbits(r[1]) * 3);
        V3(r, vst1q_f64, float64x2_t, vc, va, vb, "fmls %0.2d, %1.2d, %2.d[1]");
        acc = acc * 31 + (dbits(r[0]) ^ dbits(r[1]) * 3);
        /* One lane alone, which leaves nothing above it. */
        V3(r, vst1q_f64, float64x2_t, vc, va, vb, "fmla %d0, %d1, %2.d[1]");
        acc = acc * 31 + (dbits(r[0]) ^ dbits(r[1]) * 3);
        V3(r, vst1q_f64, float64x2_t, vc, va, vb, "fmls %d0, %d1, %2.d[1]");
        acc = acc * 31 + (dbits(r[0]) ^ dbits(r[1]) * 3);
    }
    printf("double, vectors: %016llx\n", (unsigned long long)acc);

    facc = 0;
    for (int i = 0; i < 20000; i++) {
        float a[4], b[4], c[4], r[4];
        for (int k = 0; k < 4; k++) {
            float *into[3] = {&a[k], &b[k], &c[k]};
            for (int o = 0; o < 3; o++) {
                uint32_t bits = (uint32_t)rnd(&s);
                /* The lane the chosen-lane forms multiply by holds no NaN, so that those
                 * have one a lane at most as well. */
                if (((i + k) % 3 != o || (o == 1 && k >= 2)) && (bits >> 23 & 0xff) == 0xff) bits &= ~(1u << 30);
                *into[o] = i & 1 ? ffrom(bits) : (float)(int32_t)bits / 65536.0f;
            }
        }
        fv = a[0];
        a[0] = fv;
        float32x4_t va = vld1q_f32(a), vb = vld1q_f32(b), vc = vld1q_f32(c);
        V3(r, vst1q_f32, float32x4_t, vc, va, vb, "fmla %0.4s, %1.4s, %2.4s");
        for (int k = 0; k < 4; k++) facc = facc * 31 + fbits(r[k]);
        V3(r, vst1q_f32, float32x4_t, vc, va, vb, "fmls %0.4s, %1.4s, %2.4s");
        for (int k = 0; k < 4; k++) facc = facc * 31 + fbits(r[k]);
        V3(r, vst1q_f32, float32x4_t, vc, va, vb, "fmla %0.4s, %1.4s, %2.s[3]");
        for (int k = 0; k < 4; k++) facc = facc * 31 + fbits(r[k]);
        /* Two lanes: the upper half of the register comes back zero. */
        V3(r, vst1q_f32, float32x4_t, vc, va, vb, "fmla %0.2s, %1.2s, %2.2s");
        for (int k = 0; k < 4; k++) facc = facc * 31 + fbits(r[k]);
        V3(r, vst1q_f32, float32x4_t, vc, va, vb, "fmls %0.2s, %1.2s, %2.2s");
        for (int k = 0; k < 4; k++) facc = facc * 31 + fbits(r[k]);
        V3(r, vst1q_f32, float32x4_t, vc, va, vb, "fmla %s0, %s1, %2.s[2]");
        for (int k = 0; k < 4; k++) facc = facc * 31 + fbits(r[k]);
    }
    printf("float, vectors: %08x\n", facc);

    /* Bullet advances its fixed-step clock with exactly this cancellation.
     * Keep it separate from the checksums: the result must be positive zero,
     * and a stale temporary on the emitter's operand stack once turned it
     * into 0xffffffff (a NaN), hiding every 3D object in SuperTuxKart. */
    fv = 0.008333333333333333f;
    printf("float, exact cancellation: %08x\n",
           fbits(fmsub_f(1.0f, fv, fv)));
    return 0;
}
