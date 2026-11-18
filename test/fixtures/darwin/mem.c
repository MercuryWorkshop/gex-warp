// Memory: mmap in Darwin's flags and on Darwin's page, and Mach's own
// virtual memory traps.
#include "t.h"

#if defined(__arm64__)
#define PAGE 16384L
#else
#define PAGE 4096L
#endif

static long mmap_(void *addr, long len, int prot, int flags, int fd, long off) {
    return SYS(SYS_mmap, addr, len, prot, flags, fd, off);
}

static long fork_(void) {
    long child_flag = 0;
    long pid = t_syscall2(&child_flag, SYS_fork, 0, 0, 0);
    if (pid < 0) return pid;
    return child_flag ? 0 : pid;
}

// How many of `n` bytes at `p` are not zero.
static long nonzero(const u8 *p, long n) {
    long k = 0;
    for (long i = 0; i < n; i++) k += p[i] != 0;
    return k;
}

// A store into `p` in a child: how the child ended (its wait status).
static int store_in_child(u8 *p) {
    long pid = fork_();
    if (pid == 0) {
        *p = 1;
        SYS(SYS_exit, 0);
    }
    int status = -1;
    SYS(SYS_wait4, pid, &status, 0, 0);
    return status;
}

// Anonymous memory made where memory was written reads as zero, and keeps
// the protection it was made with, whatever was there: a fixed mapping over
// written memory, read-only; one made after the written memory was
// unmapped a page at a time; a reservation (no access) over written
// memory, made readable later. (Sixteen pages, so that some of them are
// whole pages of a host with larger ones.) `faults`: and that a store into
// the read-only memory and into the reservation faults (not on the page's
// runtime, whose memory has no protections).
static void reused(int faults) {
    const long n = 16 * PAGE;
    u8 *w = (u8 *)mmap_(0, n, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    t_memset(w, 0x5a, n);
    long r = mmap_(w, n, PROT_READ, MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0);
    t_check("read-only over written memory, where asked", r == (long)w);
    t_printf("  bytes of it not zero: %ld\n", nonzero(w, n));
    int st;
    if (faults) {
        st = store_in_child(w + 5 * PAGE);
        t_printf("  a store into it: %s\n", st == 0 ? "went through" : (st & 0x7f) == SIGBUS || (st & 0x7f) == SIGSEGV ? "a fault" : "something else");
    }
    t_show("munmap", SYS(SYS_munmap, w, n));

    u8 *v = (u8 *)mmap_(0, n, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    t_memset(v, 0x6b, n);
    long gone = 0;
    for (long i = 1; i < 15; i++) gone += SYS(SYS_munmap, v + i * PAGE, PAGE) == 0;
    t_printf("  unmapped a page at a time: %ld of 14\n", gone);
    long again = mmap_(v + PAGE, 14 * PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0);
    t_check("mapped there again", again == (long)(v + PAGE));
    t_printf("  bytes of it not zero: %ld; the page before %x, after %x\n", nonzero(v + PAGE, 14 * PAGE), v[PAGE - 1], v[15 * PAGE]);
    SYS(SYS_munmap, v, n);

    u8 *x = (u8 *)mmap_(0, n, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    t_memset(x, 0x7c, n);
    long none = mmap_(x, n, PROT_NONE, MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0);
    t_check("a reservation over written memory", none == (long)x);
    if (faults) {
        st = store_in_child(x + 8 * PAGE);
        t_printf("  a store into the reservation: %s\n", st == 0 ? "went through" : (st & 0x7f) == SIGBUS || (st & 0x7f) == SIGSEGV ? "a fault" : "something else");
    }
    t_show("mprotect it readable", SYS(SYS_mprotect, x, n, PROT_READ | PROT_WRITE));
    t_printf("  bytes of it not zero: %ld\n", nonzero(x, n));
    SYS(SYS_munmap, x, n);

    // And Mach's: vm_allocate over written memory (VM_FLAGS_OVERWRITE).
    u64 m = 0;
    long task = TRAP(TRAP_task_self);
    TRAP(TRAP_mach_vm_allocate, task, &m, n, 1);
    t_memset((void *)m, 0x2d, n);
    u64 over = m;
    t_show("mach_vm_allocate over written memory", TRAP(TRAP_mach_vm_allocate, task, &over, n, 0x4000));
    t_printf("  where asked: %s; bytes of it not zero: %ld\n", over == m ? "yes" : "no", nonzero((u8 *)m, n));
    TRAP(TRAP_mach_vm_deallocate, task, m, n);
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    long a = mmap_(0, 100, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    t_check("mmap anonymous", a > 0);
    t_check("on a page boundary", a % PAGE == 0);
    char *p = (char *)a;
    int zero = 1;
    for (long i = 0; i < PAGE; i++) zero &= p[i] == 0;
    t_check("zero to the end of its page", zero);
    p[0] = 'x', p[PAGE - 1] = 'y';
    long b = mmap_(0, 3 * PAGE + 1, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    t_check("a second, apart from the first", b > 0 && b % PAGE == 0 && (b + 4 * PAGE <= a || b >= a + PAGE));
    ((char *)b)[3 * PAGE] = 1;
    t_show("munmap", SYS(SYS_munmap, b, 3 * PAGE + 1));
    t_show("munmap unaligned", SYS(SYS_munmap, a + 1, PAGE));
    t_show("mmap of no length", mmap_(0, 0, PROT_READ, MAP_PRIVATE | MAP_ANON, -1, 0));
    t_check("mmap neither shared nor private", mmap_(0, PAGE, PROT_READ, MAP_ANON, -1, 0) > 0);
    t_show("mmap a bad descriptor", mmap_(0, PAGE, PROT_READ, MAP_PRIVATE, 99, 0));
    // With a tag where the descriptor goes, as libmalloc passes one.
    long tagged = mmap_(0, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, (int)(7u << 24), 0);
    t_check("mmap with a VM tag", tagged > 0 && tagged % PAGE == 0);

    // A fixed mapping replaces what is there.
    long big = mmap_(0, 8 * PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    t_memset((void *)big, 0x55, 8 * PAGE);
    long fixed = mmap_((void *)(big + 2 * PAGE), PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0);
    t_check("MAP_FIXED where asked", fixed == big + 2 * PAGE);
    t_check("and zero there", ((u8 *)big)[2 * PAGE] == 0 && ((u8 *)big)[3 * PAGE - 1] == 0);
    t_check("its neighbours kept", ((u8 *)big)[2 * PAGE - 1] == 0x55 && ((u8 *)big)[3 * PAGE] == 0x55);
    t_show("MAP_FIXED off a page", mmap_((void *)(big + 100), PAGE, PROT_READ, MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0));
    t_show("mprotect", SYS(SYS_mprotect, big, PAGE, PROT_READ));
    t_show("mprotect back", SYS(SYS_mprotect, big, PAGE, PROT_READ | PROT_WRITE));
    t_show("madvise", SYS(SYS_madvise, big, PAGE, 4));
    t_check("madvise kept the page", ((u8 *)big)[0] == 0x55);
    // libmalloc's free of a large block: MADV_FREE_REUSABLE (its pages the
    // kernel's to drop) and MADV_FREE_REUSE when it is used again; the
    // memory is the process's to write and read throughout.
    t_show("madvise reusable", SYS(SYS_madvise, big + 4 * PAGE, 4 * PAGE, 7));
    ((u8 *)big)[5 * PAGE + 3] = 0x77;
    t_check("written after reusable", ((u8 *)big)[5 * PAGE + 3] == 0x77);
    t_show("madvise reuse", SYS(SYS_madvise, big + 4 * PAGE, 4 * PAGE, 8));
    t_check("reuse keeps what was written since", ((u8 *)big)[5 * PAGE + 3] == 0x77 && ((u8 *)big)[3 * PAGE] == 0x55);

    // A file's bytes, privately and shared.
    long fd = SYS(SYS_open, "m.dat", O_RDWR | O_CREAT | O_TRUNC, 0644);
    for (int i = 0; i < 5; i++) SYS(SYS_write, fd, "0123456789abcdef", 16);
    long f = mmap_(0, 80, PROT_READ | PROT_WRITE, MAP_PRIVATE, (int)fd, 0);
    t_check("mmap a file", f > 0 && f % PAGE == 0);
    t_check("its bytes", !t_memcmp((void *)f, "0123456789abcdef0123", 20));
    t_check("zero past its end", ((u8 *)f)[80] == 0 && ((u8 *)f)[PAGE - 1] == 0);
    ((char *)f)[0] = 'X';
    char c = 0;
    SYS(SYS_pread, fd, &c, 1, 0);
    t_check("a private write stays private", c == '0');
    long s = mmap_(0, 80, PROT_READ | PROT_WRITE, MAP_SHARED, (int)fd, 0);
    ((char *)s)[1] = 'Y';
    t_show("msync", SYS(65, s, 80, 0x10));
    SYS(SYS_munmap, s, 80);
    SYS(SYS_pread, fd, &c, 1, 1);
    t_check("a shared write reaches the file", c == 'Y');
    SYS(SYS_close, fd);
    SYS(SYS_unlink, "m.dat");

    // Mach's traps: the kernel's answers are kern_return_t codes.
    long task = TRAP(TRAP_task_self);
    t_check("mach_task_self", task > 0);
    t_check("mach_thread_self", TRAP(TRAP_thread_self) > 0);
    t_check("mach_host_self", TRAP(TRAP_host_self) > 0);
    long r1 = TRAP(TRAP_mach_reply_port), r2 = TRAP(TRAP_mach_reply_port);
    t_check("mach_reply_port, twice", r1 > 0 && r2 > 0 && r1 != r2);
    u64 addr = 0;
    t_show("mach_vm_allocate anywhere", TRAP(TRAP_mach_vm_allocate, task, &addr, 5 * PAGE + 3, 1));
    t_check("on a page, zeroed", addr && addr % PAGE == 0 && ((u8 *)addr)[6 * PAGE - 1] == 0);
    ((u8 *)addr)[0] = 1;
    u64 again = addr;
    t_show("mach_vm_allocate over it", TRAP(TRAP_mach_vm_allocate, task, &again, PAGE, 0));
    t_show("mach_vm_protect", TRAP(TRAP_mach_vm_protect, task, addr, PAGE, 0, PROT_READ));
    t_show("mach_vm_deallocate", TRAP(TRAP_mach_vm_deallocate, task, addr, 6 * PAGE));
    again = addr;
    t_show("mach_vm_allocate there again", TRAP(TRAP_mach_vm_allocate, task, &again, PAGE, 0));
    t_check("at that address, zeroed", again == addr && ((u8 *)addr)[0] == 0);
    t_show("mach_vm_deallocate", TRAP(TRAP_mach_vm_deallocate, task, addr, PAGE));
    t_show("mach_vm_allocate in no task", TRAP(TRAP_mach_vm_allocate, 0, &addr, PAGE, 1));
    // (target, &address, size, mask, flags, protection): aligned to 1 MiB.
    u64 al = 0;
    t_show("mach_vm_map aligned", TRAP(TRAP_mach_vm_map, task, &al, 3 * PAGE, 0xfffff, 1, PROT_READ | PROT_WRITE));
    t_check("on a megabyte", al && (al & 0xfffff) == 0);
    ((u8 *)al)[3 * PAGE - 1] = 1;
    t_show("mach_vm_deallocate of nothing", TRAP(TRAP_mach_vm_deallocate, task, al, 0));

    // MADV_ZERO: the pages read as zero once it has returned, which is what
    // libmalloc's calloc takes its success to mean.
    u8 *z = (u8 *)SYS(SYS_mmap, 0, 4 * PAGE, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    t_memset(z, 0xa5, 4 * PAGE);
    t_show("madvise to zero the middle of it", SYS(SYS_madvise, z + PAGE, 2 * PAGE, 11));
    long left = 0;
    for (long i = PAGE; i < 3 * PAGE; i++) left += z[i] != 0;
    t_printf("  bytes of it not zero: %ld; before it %x, after it %x\n", left, z[PAGE - 1], z[3 * PAGE]);

    reused(argc > 1 && !t_strcmp(argv[1], "faults"));
    return 0;
}
