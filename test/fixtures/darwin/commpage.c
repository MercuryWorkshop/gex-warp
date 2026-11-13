// The commpage: the page of facts about the machine that every process has
// mapped and libSystem reads instead of asking. On the Mac it is where XNU
// puts it; under gex, where gex does (a userland built for gex is built to
// look there). What it says must agree with what sysctl says.
#include "t.h"

#if defined(__arm64__)
#define XNU_COMMPAGE 0x0000000fffffc000ul
#define XNU_COMMPAGE_RO 0x0000000fffff4000ul
enum { PAGE_SHIFT = 0x25, CPUFAMILY = 0x80, VERSION = 3 };
#else
#define XNU_COMMPAGE 0x00007fffffe00000ul
#define XNU_COMMPAGE_RO XNU_COMMPAGE
enum { PAGE_SHIFT = 0x4e, CPUFAMILY = 0x40, VERSION = 14 };
#endif
#if defined(T_STATIC)
#define COMMPAGE 0x00ffc000ul
#define COMMPAGE_RO (COMMPAGE - (XNU_COMMPAGE - XNU_COMMPAGE_RO))
#else
#define COMMPAGE XNU_COMMPAGE
#define COMMPAGE_RO XNU_COMMPAGE_RO
#endif

static long byname(const char *name, void *old, size_t *oldlen) {
    return SYS(SYS_sysctlbyname, name, t_strlen(name), old, oldlen, 0, 0);
}

#if defined(__x86_64__)
// mach_absolute_time as libSystem computes it on x86-64: the processor's
// counter, scaled by what the page says.
static u64 nanotime(const u8 *page) {
    u32 lo, hi;
    __asm__ volatile("rdtsc" : "=a"(lo), "=d"(hi));
    u64 tsc = ((u64)hi << 32 | lo) - *(const u64 *)(page + 0x50);
    tsc <<= *(const u32 *)(page + 0x5c);
    return (u64)(((__uint128_t)tsc * *(const u32 *)(page + 0x58)) >> 32) + *(const u64 *)(page + 0x60);
}
#endif

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    // (A first call: gex maps the page as a process first enters it.)
    int ncpu = 0, family = 0;
    i64 memsize = 0, pagesize = 0;
    size_t len = sizeof ncpu;
    byname("hw.ncpu", &ncpu, &len);
    len = sizeof memsize;
    byname("hw.memsize", &memsize, &len);
    len = sizeof pagesize;
    byname("hw.pagesize", &pagesize, &len);
    len = sizeof family;
    byname("hw.cpufamily", &family, &len);

    const u8 *page = (const u8 *)COMMPAGE, *ro = (const u8 *)COMMPAGE_RO;
    char signature[17];
    t_memcpy(signature, page, 16);
    signature[16] = 0;
    t_printf("signature \"%s\"\n", signature);
    t_check("the version", *(const u16 *)(page + 0x1e) >= VERSION);
    u64 caps64 = *(const u64 *)(page + 0x10);
    u32 caps = *(const u32 *)(page + 0x20);
    t_check("the capabilities' low half, twice", (u32)caps64 == caps);
    t_check("the thread pointer is user-readable", caps & 0x80);
    t_check("the capabilities count the processors", ((caps >> 16) & 0xff) == (u32)ncpu);
    t_check("so does the page", page[0x22] == ncpu && page[0x36] == ncpu);
    // The page shift is in the read-only page on a new arm64 system, and
    // where it always was on an old one: one of the two says.
    int shift = ro[PAGE_SHIFT] ? ro[PAGE_SHIFT] : page[PAGE_SHIFT];
    t_check("a 64-bit process's page shift is hw.pagesize", shift && (1ll << shift) == pagesize);
    t_check("hw.memsize", *(const u64 *)(page + 0x38) == (u64)memsize);
#if defined(__arm64__)
    // (An x86-64 process's reference is Rosetta, whose page and sysctl name
    // different processors.)
    t_check("hw.cpufamily", *(const u32 *)(page + CPUFAMILY) == (u32)family);
#else
    t_check("the clock is in use", *(const u32 *)(page + 0x68) != 0);
    // gettimeofday's own data (_COMM_PAGE_NEWTIMEOFDAY_DATA): the absolute
    // time it was taken at (none where the page keeps none, and libSystem
    // asks the kernel), the time of day then, a tick's worth of a second
    // in 2^-64ths, and the ticks in a second, past which it is stale. Where
    // it is kept, it says what the kernel says, as libSystem reads it.
    const u64 *tod = (const u64 *)(page + 0xd0);
    u64 at = nanotime(page);
    struct t_timeval kt = {0, 0};
    SYS(SYS_gettimeofday, &kt, 0, 0);
    int agrees = 1;
    if (tod[0] && at - tod[0] < tod[4]) {
        __uint128_t f = (__uint128_t)tod[3] * (at - tod[0]) + tod[2];
        u64 us = (tod[1] + (u64)(f >> 64)) * 1000000ull + (u64)(((__uint128_t)1000000 * (u32)((u64)f >> 32)) >> 32);
        i64 off = (i64)(us - ((u64)kt.tv_sec * 1000000ull + (u64)kt.tv_usec));
        agrees = off > -100000 && off < 100000;
    }
    t_check("the time of day the page keeps is the kernel's", agrees);
    u64 t0 = nanotime(page);
    struct t_timeval wait = {0, 30000};
    SYS(SYS_select, 0, 0, 0, 0, &wait);
    u64 dt = nanotime(page) - t0;
    t_check("and counts nanoseconds", dt >= 30000000ull && dt < 2000000000ull);
#endif
#if defined(__arm64__)
    // A bitmask immediate in the page's range is a mask, not the page.
    u64 m36, m24;
    __asm__ volatile("mov %0, #0xfffffffff" : "=r"(m36));
    __asm__ volatile("mov %0, #0xffffff000" : "=r"(m24));
    t_check("a mask in the page's range is its own value", m36 == 0xffffffffful && m24 == 0xffffff000ul);
#endif
    return 0;
}
