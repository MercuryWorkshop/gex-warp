// Where a Mac program's addresses are: every kind it can be given, each
// said with whether it is at or above 4 GiB, as on a Mac, whose whole
// first 4 GiB is __PAGEZERO (Swift takes every value below it for a spare
// bit pattern of a reference). darwin/high runs it on a Mac, natively and
// on a page, on both processors.
#include <dlfcn.h>
#include <errno.h>
#include <mach-o/dyld.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <pthread.h>
#include <setjmp.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/sysctl.h>
#include <unistd.h>

#define G4 0x100000000ull

static int failed;
static int global = 1;
static const char literal[] = "a literal";
extern char **environ;

static void say(const char *what, const void *p) {
    uint64_t a = (uint64_t)(uintptr_t)p;
    int high = a >= G4;
    if (!high) failed++;
    printf("%s: %s\n", what, high ? "at or above 4 GiB" : "BELOW 4 GiB");
}

static void *thread_main(void *arg) {
    int local;
    say("a thread's stack", &local);
    say("a thread's own pthread_t", pthread_self());
    say("a thread's stack (pthread_get_stackaddr_np)", pthread_get_stackaddr_np(pthread_self()));
    return arg;
}

static sigjmp_buf back;
static volatile uint64_t fault_at, fault_pc;

static void on_fault(int sig, siginfo_t *si, void *uc) {
    (void)sig;
    fault_at = (uint64_t)(uintptr_t)si->si_addr;
    ucontext_t *u = uc;
#if defined(__arm64__)
    fault_pc = u->uc_mcontext->__ss.__pc;
#else
    fault_pc = u->uc_mcontext->__ss.__rip;
#endif
    siglongjmp(back, 1);
}

static __attribute__((noinline)) void *where_called_from(void) {
    return __builtin_return_address(0);
}

int main(int argc, char **argv) {
    int local;
    (void)argc;
    say("main's stack", &local);
    say("argv", argv);
    say("argv[0]", argv[0]);
    say("environ", environ);
    say("a function of the program", (void *)main);
    say("a global of the program", &global);
    say("a literal of the program", literal);
    say("a return address", where_called_from());
    say("the program's header (_dyld_get_image_header)", _dyld_get_image_header(0));
    say("libSystem's printf", (void *)printf);
    say("dlsym(malloc)", dlsym(RTLD_DEFAULT, "malloc"));
    Dl_info info;
    if (dladdr((void *)printf, &info)) say("libSystem's header (dladdr)", info.dli_fbase);
    else { printf("dladdr: failed\n"); failed++; }
    say("errno's place", &errno);

    // dyld's own image, as the kernel names it to the task.
    struct task_dyld_info dyld;
    mach_msg_type_number_t count = TASK_DYLD_INFO_COUNT;
    if (task_info(mach_task_self(), TASK_DYLD_INFO, (task_info_t)&dyld, &count) == KERN_SUCCESS && dyld.all_image_info_addr) {
        say("dyld's image list", (void *)(uintptr_t)dyld.all_image_info_addr);
    } else {
        printf("dyld's image list: not told\n");
    }
    for (uint32_t i = 0, n = _dyld_image_count(); i < n; i++) {
        const struct mach_header *h = _dyld_get_image_header(i);
        if ((uint64_t)(uintptr_t)h < G4) {
            printf("image %u (%s): BELOW 4 GiB\n", i, _dyld_get_image_name(i));
            failed++;
        }
    }
    printf("every image's header: checked\n");

    void *small = malloc(16), *mid = malloc(4000), *big = malloc(1 << 20), *huge = malloc(64 << 20);
    say("malloc(16)", small);
    say("malloc(4000)", mid);
    say("malloc(1 MiB)", big);
    say("malloc(64 MiB)", huge);
    void *cz = calloc(100, 100);
    say("calloc", cz);
    void *re = realloc(small, 100000);
    say("realloc", re);
    void *al = NULL;
    if (posix_memalign(&al, 65536, 100) == 0) say("posix_memalign", al);
    free(re);
    free(mid);
    free(big);
    free(huge);
    free(cz);
    free(al);

    void *anon = mmap(NULL, 1 << 16, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    say("mmap", anon);
    void *none = mmap(NULL, 1 << 20, PROT_NONE, MAP_ANON | MAP_PRIVATE, -1, 0);
    say("mmap(PROT_NONE)", none);
    vm_address_t va = 0;
    if (vm_allocate(mach_task_self(), &va, 1 << 16, VM_FLAGS_ANYWHERE) == KERN_SUCCESS) say("vm_allocate", (void *)va);
    else { printf("vm_allocate: failed\n"); failed++; }
    mach_vm_address_t mva = 0;
    if (mach_vm_map(mach_task_self(), &mva, 1 << 16, 0, VM_FLAGS_ANYWHERE, MEMORY_OBJECT_NULL, 0, 0, VM_PROT_READ, VM_PROT_ALL, VM_INHERIT_COPY) == KERN_SUCCESS) say("mach_vm_map", (void *)mva);
    else { printf("mach_vm_map: failed\n"); failed++; }
    // The region the first mapping is in, as the task is told it.
    mach_vm_address_t ra = (mach_vm_address_t)(uintptr_t)anon;
    mach_vm_size_t rs = 0;
    vm_region_basic_info_data_64_t ri;
    mach_msg_type_number_t rc = VM_REGION_BASIC_INFO_COUNT_64;
    mach_port_t obj;
    if (mach_vm_region(mach_task_self(), &ra, &rs, VM_REGION_BASIC_INFO_64, (vm_region_info_t)&ri, &rc, &obj) == KERN_SUCCESS) {
        say("mach_vm_region", (void *)ra);
        uint64_t asked = (uint64_t)(uintptr_t)anon;
        printf("mach_vm_region: %s\n", ra <= asked && asked < ra + rs ? "holds the mapping asked about" : "ANOTHER region");
    } else {
        printf("mach_vm_region: failed\n");
        failed++;
    }

    // MAP_FIXED at 4 GiB and above: where a mapping was, and an address
    // chosen in the window.
    char *was = mmap(NULL, 1 << 16, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    munmap(was, 1 << 16);
    char *fixed = mmap(was, 1 << 16, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE | MAP_FIXED, -1, 0);
    printf("MAP_FIXED where a mapping was: %s\n", fixed == was ? "there" : "ELSEWHERE");
    if (fixed != was) failed++;
    if (fixed != MAP_FAILED) {
        fixed[0] = 'x';
        fixed[(1 << 16) - 1] = 'y';
        printf("MAP_FIXED there: written and read back: %s\n", fixed[0] == 'x' && fixed[(1 << 16) - 1] == 'y' ? "yes" : "NO");
    }
    char *chosen = (char *)(uintptr_t)0x130000000ull;
    char *at = mmap(chosen, 1 << 16, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE | MAP_FIXED, -1, 0);
    printf("MAP_FIXED at 0x130000000: %s\n", at == chosen ? "there" : "ELSEWHERE");
    if (at != chosen) failed++;
    if (at == chosen) {
        strcpy(at, "high");
        printf("MAP_FIXED at 0x130000000: %s\n", strcmp(at, "high") == 0 ? "written and read back" : "NOT READ BACK");
        munmap(at, 1 << 16);
    }

    // The commpage, read where XNU puts it, by constant, as libSystem
    // reads it: its signature, and its count of processors hw.ncpu's.
#if defined(__arm64__)
    const volatile char *cp = (const volatile char *)(uintptr_t)0x0000000fffffc000ull;
#else
    const volatile char *cp = (const volatile char *)(uintptr_t)0x00007fffffe00000ull;
#endif
    char sig[16];
    for (int i = 0; i < 15; i++) sig[i] = cp[i];
    sig[15] = 0;
    printf("the commpage at XNU's address: \"%s\"\n", sig);
    int ncpu = 0;
    size_t ncpu_len = sizeof ncpu;
    sysctlbyname("hw.ncpu", &ncpu, &ncpu_len, NULL, 0);
    printf("the commpage's processors: %s\n", cp[0x22] == ncpu ? "hw.ncpu's" : "NOT hw.ncpu's");

    pthread_t t;
    pthread_create(&t, NULL, thread_main, NULL);
    pthread_join(t, NULL);

    // A fault's address and pc, as a handler is told them.
    char *guard = mmap(NULL, 1 << 16, PROT_NONE, MAP_ANON | MAP_PRIVATE, -1, 0);
    struct sigaction sa = {0};
    sa.sa_sigaction = on_fault;
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGSEGV, &sa, NULL);
    sigaction(SIGBUS, &sa, NULL);
    if (sigsetjmp(back, 1) == 0) {
        *(volatile char *)(guard + 0x100) = 1;
        printf("a store into PROT_NONE: no fault\n");
    } else {
        printf("a fault's address: %s\n", fault_at == (uint64_t)(uintptr_t)(guard + 0x100) ? "the one stored to" : fault_at >= G4 ? "another, at or above 4 GiB" : "BELOW 4 GiB");
        say("a fault's pc", (void *)(uintptr_t)fault_pc);
    }

    printf("%s\n", failed ? "SOME BELOW 4 GiB" : "nothing below 4 GiB");
    return failed != 0;
}
