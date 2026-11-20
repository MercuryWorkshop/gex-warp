// A Mac's __PAGEZERO is its program's whole first 4 GiB, and nothing can
// be mapped there: a fixed mapping asked for below 4 GiB fails, an
// address there given only as a hint is not where the mapping goes, and a
// call that names memory there acts on none. gex gives a Darwin process
// its addresses 4 GiB above its memory's offsets (the window); an address
// below the window must not name the window's memory under it. Each line
// is a result the Mac prints the same, and gex must print as it does.
#include <errno.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/mman.h>
#include <unistd.h>

#define LOW 0x800000UL
#define WINDOW 0x100000000UL

// The mapping at `p` as the kernel has it: there with its protection, or
// gone (asked of the map, not by touching it).
static void region(const void *p) {
    mach_vm_address_t a = (mach_vm_address_t)(uintptr_t)p;
    mach_vm_size_t size = 0;
    vm_region_basic_info_data_64_t info;
    mach_msg_type_number_t count = VM_REGION_BASIC_INFO_COUNT_64;
    mach_port_t object = MACH_PORT_NULL;
    kern_return_t kr = mach_vm_region(mach_task_self(), &a, &size, VM_REGION_BASIC_INFO_64, (vm_region_info_t)&info, &count, &object);
    // (the region holding it, which may begin below it, joined to memory beside)
    if (kr != KERN_SUCCESS || a > (mach_vm_address_t)(uintptr_t)p || a + size <= (mach_vm_address_t)(uintptr_t)p) printf("  then it: GONE\n");
    else printf("  then it: there, protection %d\n", info.protection);
}

static void said(const char *what, int r) {
    if (r == 0) printf("%s: 0\n", what);
    else printf("%s: errno %d\n", what, errno);
}

int main(void) {
    size_t page = (size_t)getpagesize();
    mach_port_t task = mach_task_self();
    kern_return_t kr;

    // Fixed, below 4 GiB.
    vm_address_t va = LOW;
    kr = vm_allocate(task, &va, page, VM_FLAGS_FIXED);
    printf("vm_allocate fixed at 8 MiB: %d\n", kr);
    mach_vm_address_t ma = LOW;
    kr = mach_vm_allocate(task, &ma, page, VM_FLAGS_FIXED);
    printf("mach_vm_allocate fixed at 8 MiB: %d\n", kr);
    ma = LOW;
    kr = mach_vm_map(task, &ma, page, 0, VM_FLAGS_FIXED, MACH_PORT_NULL, 0, FALSE, VM_PROT_DEFAULT, VM_PROT_ALL, VM_INHERIT_DEFAULT);
    printf("mach_vm_map fixed at 8 MiB: %d\n", kr);
    void *p = mmap((void *)LOW, page, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE | MAP_FIXED, -1, 0);
    if (p == MAP_FAILED) printf("mmap MAP_FIXED at 8 MiB: failed, errno %d\n", errno);
    else printf("mmap MAP_FIXED at 8 MiB: mapped\n");
    // Fixed, across 4 GiB: from below it up into it.
    p = mmap((void *)(WINDOW - page), 2 * page, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE | MAP_FIXED, -1, 0);
    if (p == MAP_FAILED) printf("mmap MAP_FIXED across 4 GiB: failed, errno %d\n", errno);
    else printf("mmap MAP_FIXED across 4 GiB: mapped\n");
    ma = WINDOW - page;
    kr = mach_vm_allocate(task, &ma, 2 * page, VM_FLAGS_FIXED);
    printf("mach_vm_allocate fixed across 4 GiB: %d\n", kr);

    // Below 4 GiB as a hint only: somewhere at or above it.
    p = mmap((void *)LOW, page, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    printf("mmap hinted at 8 MiB: %s\n", p == MAP_FAILED ? "failed" : (uintptr_t)p >= WINDOW ? "at or above 4 GiB" : "BELOW 4 GiB");
    ma = LOW;
    kr = mach_vm_allocate(task, &ma, page, VM_FLAGS_ANYWHERE);
    printf("mach_vm_allocate anywhere from 8 MiB: %d, %s\n", kr, ma >= WINDOW ? "at or above 4 GiB" : "BELOW 4 GiB");

    // Memory of the program's own at an address 4 GiB up, and the same
    // address less 4 GiB, below it: the calls on that one leave this be.
    char *own = mmap(NULL, page, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    if (own == MAP_FAILED || (uintptr_t)own < WINDOW) {
        printf("no memory of its own above 4 GiB\n");
        return 1;
    }
    own[0] = 'x';
    void *under = (void *)((uintptr_t)own - WINDOW);
    said("mprotect of it less 4 GiB", mprotect(under, page, PROT_NONE));
    region(own);
    said("msync of it less 4 GiB", msync(under, page, MS_SYNC));
    said("madvise of it less 4 GiB", madvise(under, page, MADV_FREE));
    region(own);
    kr = mach_vm_protect(task, (mach_vm_address_t)(uintptr_t)under, page, FALSE, VM_PROT_NONE);
    printf("mach_vm_protect of it less 4 GiB: %d\n", kr);
    region(own);
    said("munmap of it less 4 GiB", munmap(under, page));
    region(own);
    kr = mach_vm_deallocate(task, (mach_vm_address_t)(uintptr_t)under, page);
    printf("mach_vm_deallocate of it less 4 GiB: %d\n", kr);
    region(own);
    return 0;
}
