// What a check before every store would cost, modelled in the program:
// each store preceded by a look at a byte per 4 KiB page (`map`), as a
// translator's check of a page protection map would be, against the same
// stores without. Three store-heavy loops: a sequential fill, scattered
// stores into a table, and a copy. A compiler barrier after every store in
// both, so neither is vectorised or has its checks hoisted (a translator
// checks each store it translates).
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static uint8_t map[1 << 20];
static volatile uint64_t sink;

static double now(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec + t.tv_nsec / 1e9;
}

#define B() __asm__ volatile("" ::: "memory")
#define CHECK(p) do { if (__builtin_expect(map[((uintptr_t)(p) >> 12) & 0xfffff] == 0, 0)) abort(); } while (0)

__attribute__((noinline)) static void fill(uint64_t *a, size_t n, int checked, uint64_t v) {
    if (checked) for (size_t i = 0; i < n; i++) { CHECK(&a[i]); a[i] = v + i; B(); }
    else for (size_t i = 0; i < n; i++) { a[i] = v + i; B(); }
}

__attribute__((noinline)) static void scatter(uint64_t *a, size_t n, size_t mask, int checked) {
    uint64_t x = 88172645463325252ull;
    if (checked) for (size_t i = 0; i < n; i++) { x ^= x << 13; x ^= x >> 7; x ^= x << 17; uint64_t *p = &a[x & mask]; CHECK(p); *p += i; B(); }
    else for (size_t i = 0; i < n; i++) { x ^= x << 13; x ^= x >> 7; x ^= x << 17; uint64_t *p = &a[x & mask]; *p += i; B(); }
}

__attribute__((noinline)) static void copy(uint32_t *d, const uint32_t *s, size_t n, int checked) {
    if (checked) for (size_t i = 0; i < n; i++) { CHECK(&d[i]); d[i] = s[i] ^ 5; B(); }
    else for (size_t i = 0; i < n; i++) { d[i] = s[i] ^ 5; B(); }
}

int main(int argc, char **argv) {
    int reps = argc > 1 ? atoi(argv[1]) : 5;
    memset(map, 1, sizeof map);
    size_t n = 4 << 20;  // 32 MiB of u64
    uint64_t *a = malloc(n * 8);
    uint32_t *s = malloc(n * 4), *d = malloc(n * 4);
    memset(a, 0, n * 8), memset(s, 1, n * 4), memset(d, 0, n * 4);
    const char *names[] = {"fill", "scatter", "copy"};
    for (int k = 0; k < 3; k++) {
        double best[2] = {1e9, 1e9};
        for (int r = 0; r < reps; r++)
            for (int c = 0; c < 2; c++) {
                double t0 = now();
                if (k == 0) fill(a, n, c, r);
                else if (k == 1) scatter(a, n, n - 1, c);
                else copy(d, s, n, c);
                double t = now() - t0;
                if (t < best[c]) best[c] = t;
            }
        sink += a[7] + d[9];
        printf("%-8s plain %7.2f ns/store, checked %7.2f ns/store: +%.0f%%\n", names[k], best[0] * 1e9 / n, best[1] * 1e9 / n, (best[1] / best[0] - 1) * 100);
    }
    return 0;
}
