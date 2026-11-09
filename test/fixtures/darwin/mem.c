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
    return 0;
}
