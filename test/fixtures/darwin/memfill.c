// Filling and copying memory as a Mac program does: libSystem's memset,
// bzero, memmove and memcpy (libplatform's, which gex serves with the
// host's builtins), and the instructions a memset is made of -- `dc zva`
// with its block size from DCZID_EL0 on arm64, `rep stosb` and `rep movsb`
// on x86-64 -- at the edges: unaligned starts and lengths, nothing at all,
// across a page boundary, into a page that is not there or not writable
// (where the process dies of the fault, and what was written before it is
// what was written). Every result is a line, checked against the Mac's own.
//
// `memfill faults`: the same into pages that are not there or not
// writable, where the process dies of the fault and what was written
// before it stays written (in a mapping it shares with its parent).
// `memfill time` prints what each costs, for a person; nothing checks it.
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static size_t page;

// A run of `n` bytes from `p`, as the first byte, how many bytes run on equal
// to it, and so on: "aa*3 00*64 aa*5".
static void runs(const char *label, const unsigned char *p, size_t n) {
    printf("%s:", label);
    for (size_t i = 0; i < n;) {
        size_t j = i;
        while (j < n && p[j] == p[i]) j++;
        printf(" %02x*%zu", p[i], j - i);
        i = j;
    }
    printf("\n");
}

static unsigned char *fresh(size_t pages, int fill) {
    unsigned char *p = mmap(0, pages * page, PROT_READ | PROT_WRITE, MAP_ANON | MAP_SHARED, -1, 0);
    if (p == MAP_FAILED) {
        perror("mmap");
        exit(2);
    }
    memset(p, fill, pages * page);
    return p;
}

// `f` in a child, which is expected to die of a fault: how it ended.
static void dies(const char *label, void (*f)(unsigned char *), unsigned char *p) {
    fflush(stdout);
    pid_t c = fork();
    if (c == 0) {
        f(p);
        _exit(0);
    }
    int st = 0;
    waitpid(c, &st, 0);
    if (WIFSIGNALED(st)) printf("%s: killed by signal %d\n", label, WTERMSIG(st));
    else printf("%s: exited %d\n", label, WEXITSTATUS(st));
}

static void *volatile vmemset = memset;
static void *volatile vbzero = bzero;
static void *volatile vmemmove = memmove;
static void *volatile vmemcpy = memcpy;
#define MEMSET ((void *(*)(void *, int, size_t))vmemset)
#define BZERO ((void (*)(void *, size_t))vbzero)
#define MEMMOVE ((void *(*)(void *, const void *, size_t))vmemmove)
#define MEMCPY ((void *(*)(void *, const void *, size_t))vmemcpy)

static void set_across_end(unsigned char *p) { MEMSET(p + page - 100, 0x5a, 300); }
static void bzero_across_end(unsigned char *p) { BZERO(p + page - 100, 300); }
static void copy_across_end(unsigned char *p) { MEMMOVE(p + page - 100, p, 300); }

#if defined(__aarch64__)
static void zva(void *p) { __asm__ volatile("dc zva, %0" ::"r"(p) : "memory"); }
static void zva_unmapped(unsigned char *p) { zva(p + page + 64); }
#else
static void stosb(void *d, int c, size_t n) { __asm__ volatile("rep stosb" : "+D"(d), "+c"(n) : "a"(c) : "memory"); }
static void movsb(void *d, const void *s, size_t n) { __asm__ volatile("rep movsb" : "+D"(d), "+S"(s), "+c"(n)::"memory"); }
static void stosb_across_end(unsigned char *p) { stosb(p + page - 100, 0x5a, 300); }
static void movsb_across_end(unsigned char *p) { movsb(p + page - 100, p, 300); }
#endif

static double now(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1e9 + t.tv_nsec;
}

static void timing(void) {
    static unsigned char buf[8192] __attribute__((aligned(64)));
    size_t sizes[] = {16, 64, 1100, 4096};
    for (int s = 0; s < 4; s++) {
        size_t n = sizes[s];
        double t0 = now();
        for (int i = 0; i < 20000; i++) MEMSET(buf, i & 1, n);
        double t1 = now();
        for (int i = 0; i < 20000; i++) BZERO(buf, n);
        double t2 = now();
        for (int i = 0; i < 20000; i++) MEMCPY(buf + 4096, buf, n);
        double t3 = now();
        printf("%5zu bytes: memset %8.1f ns  bzero %8.1f ns  memcpy %8.1f ns\n", n, (t1 - t0) / 20000, (t2 - t1) / 20000, (t3 - t2) / 20000);
    }
}

int main(int argc, char **argv) {
    page = (size_t)getpagesize();
    if (argc > 1 && !strcmp(argv[1], "time")) {
        timing();
        return 0;
    }
    int faults = argc > 1 && !strcmp(argv[1], "faults");
    unsigned char *p = fresh(2, 0xaa);
    if (faults) goto faulting;

    // memset: what it returns, and exactly the bytes it was given.
    void *r = MEMSET(p + 3, 0x11, 13);
    printf("memset returns its start: %s\n", r == p + 3 ? "yes" : "no");
    runs("memset 3+13", p, 24);
    MEMSET(p + 5, 0x22, 0);
    runs("memset of nothing", p, 24);
    MEMSET(p + 1, 0x1ff, 2);
    runs("memset takes the low byte", p, 8);
    MEMSET(p + page - 37, 0x33, 77);
    runs("memset across the page boundary", p + page - 40, 80);
    BZERO(p + 7, 1001);
    runs("bzero 7+1001", p, 1024);
    BZERO(p + 2, 0);
    runs("bzero of nothing", p, 8);

    // memmove both ways over itself, memcpy apart.
    for (int i = 0; i < 64; i++) p[1024 + i] = (unsigned char)i;
    MEMMOVE(p + 1024 + 5, p + 1024, 40);
    printf("memmove up: %u %u %u %u\n", p[1029], p[1030], p[1044], p[1068]);
    MEMMOVE(p + 1024, p + 1024 + 9, 40);
    printf("memmove down: %u %u %u %u\n", p[1024], p[1025], p[1063], p[1064]);
    r = MEMCPY(p + 2048, p + 1024, 0);
    printf("memcpy of nothing returns its start: %s\n", r == p + 2048 ? "yes" : "no");
    MEMCPY(p + page - 3, p + 1024, 9);
    printf("memcpy across the page boundary: %u %u %u\n", p[page - 3], p[page], p[page + 5]);

#if defined(__aarch64__)
    uint64_t dczid;
    __asm__("mrs %0, dczid_el0" : "=r"(dczid));
    size_t block = (size_t)4 << (dczid & 15);
    printf("dczid_el0: block %zu bytes, zeroing %s\n", block, dczid & 16 ? "prohibited" : "allowed");
    unsigned char *z = fresh(2, 0xaa);
    // The block holding the address, aligned down, and nothing else.
    zva(z + 3 * block + 17);
    runs("dc zva inside a block", z + 2 * block, 3 * block);
    zva(z + page - 1);
    zva(z + page);
    runs("dc zva either side of the page boundary", z + page - 2 * block, 4 * block);
#else
    unsigned char *x = fresh(2, 0xaa);
    stosb(x + 3, 0x44, 29);
    runs("rep stosb 3+29", x, 40);
    stosb(x + 9, 0x55, 0);
    runs("rep stosb of nothing", x, 40);
    stosb(x + page - 21, 0x66, 50);
    runs("rep stosb across the page boundary", x + page - 24, 56);
    for (int i = 0; i < 64; i++) x[2048 + i] = (unsigned char)i;
    movsb(x + 2048 + 70, x + 2048 + 1, 31);
    printf("rep movsb: %u %u %u\n", x[2048 + 70], x[2048 + 71], x[2048 + 100]);
#endif
    return 0;

faulting:;
    // Into a page that is not there: the process dies, and what it wrote
    // before the fault stays written.
    unsigned char *q = fresh(2, 0xaa);
    munmap(q + page, page);
    dies("memset into an unmapped page", set_across_end, q);
    unsigned char *w = fresh(2, 0xaa);
    mprotect(w + page, page, PROT_READ);
    dies("bzero into a read-only page", bzero_across_end, w);
    runs("the read-only page after", w + page, 16);
    dies("memmove into a read-only page", copy_across_end, w);

#if defined(__aarch64__)
    unsigned char *u = fresh(2, 0xaa);
    munmap(u + page, page);
    dies("dc zva on an unmapped page", zva_unmapped, u);
    unsigned char *ro = fresh(2, 0xaa);
    mprotect(ro + page, page, PROT_READ);
    dies("dc zva on a read-only page", zva_unmapped, ro);
    runs("the read-only page after dc zva", ro + page, 256);
#else
    unsigned char *y = fresh(2, 0xaa);
    mprotect(y + page, page, PROT_READ);
    dies("rep stosb into a read-only page", stosb_across_end, y);
    runs("rep stosb's page before the fault", y + page - 104, 104);
    runs("and the read-only page", y + page, 16);
    unsigned char *v = fresh(2, 0xaa);
    munmap(v + page, page);
    dies("rep movsb into an unmapped page", movsb_across_end, v);
    runs("rep movsb's page before the fault", v + page - 104, 104);
#endif
    return 0;
}
