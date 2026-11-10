// The commpage: the page of facts about the machine that every process has
// mapped and libSystem reads instead of asking. On the Mac it is where XNU
// puts it; under gex, where gex does (a userland built for gex is built to
// look there). What it says must agree with what sysctl says.
#include "t.h"

#if defined(T_STATIC)
#define COMMPAGE 0x00ffc000ul
#define COMMPAGE_RO 0x00ff4000ul
#else
#define COMMPAGE 0x0000000fffffc000ul
#define COMMPAGE_RO 0x0000000fffff4000ul
#endif

static long byname(const char *name, void *old, size_t *oldlen) {
    return SYS(SYS_sysctlbyname, name, t_strlen(name), old, oldlen, 0, 0);
}

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
    t_printf("version %d\n", *(const u16 *)(page + 0x1e));
    u64 caps64 = *(const u64 *)(page + 0x10);
    u32 caps = *(const u32 *)(page + 0x20);
    t_check("the capabilities' low half, twice", (u32)caps64 == caps);
    t_check("the thread pointer is user-readable, and there is Advanced SIMD", (caps & 0x80) && (caps & 0x100));
    t_check("the capabilities count the processors", ((caps >> 16) & 0xff) == (u32)ncpu);
    t_check("so does the page", page[0x22] == ncpu && page[0x36] == ncpu);
    // The page shifts are in the read-only page on a new system, and where
    // they always were on an old one: one of the two says.
    int shift = ro[0x25] ? ro[0x25] : page[0x25];
    t_printf("a 64-bit process's page shift %d\n", shift);
    t_check("which is hw.pagesize", (1ll << shift) == pagesize);
    t_check("hw.memsize", *(const u64 *)(page + 0x38) == (u64)memsize);
    t_check("hw.cpufamily", *(const u32 *)(page + 0x80) == (u32)family);
    return 0;
}
