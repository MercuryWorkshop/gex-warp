// Which stores fault: into a page unmapped, PROT_NONE, read-only; a load
// from PROT_NONE; near-null stores; a write barrier that unprotects in its
// handler and lets the store go again; a stack overflow caught on an
// alternate stack. Each in a child, so that what does not fault where it
// should cannot hurt the next.
#define _GNU_SOURCE
#include <setjmp.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

static sigjmp_buf back;
static volatile void *want;
static volatile int sig, code, addr_ok, barrier_faults;
static long page;

static void on_fault(int s, siginfo_t *si, void *uc) {
    (void)uc;
    sig = s, code = si->si_code, addr_ok = si->si_addr == want;
    siglongjmp(back, 1);
}

static void report(const char *what, void (*fn)(void)) {
    fflush(stdout);
    pid_t c = fork();
    if (c == 0) {
        alarm(10);
        struct sigaction sa = {0};
        sa.sa_sigaction = on_fault, sa.sa_flags = SA_SIGINFO;
        sigaction(SIGSEGV, &sa, 0), sigaction(SIGBUS, &sa, 0);
        if (sigsetjmp(back, 1)) {
            printf("%s: %s, code %d, si_addr %s\n", what, sig == SIGSEGV ? "SIGSEGV" : sig == SIGBUS ? "SIGBUS" : "?", code, addr_ok ? "the address" : "another");
            _exit(0);
        }
        fn();
        printf("%s: no fault\n", what);
        _exit(0);
    }
    int st;
    waitpid(c, &st, 0);
    if (!WIFEXITED(st)) printf("%s: the child ended by signal %d\n", what, WIFSIGNALED(st) ? WTERMSIG(st) : -1);
}

static char *fresh(int prot) {
    char *p = mmap(0, 3 * page, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    mprotect(p + page, page, prot);
    return p + page;
}

static void unmapped(void) { char *p = fresh(PROT_READ | PROT_WRITE); munmap(p, page); want = p + 8; *(volatile char *)(p + 8) = 1; }
static void none(void) { char *p = fresh(PROT_NONE); want = p + 16; *(volatile int *)(p + 16) = 1; }
static void readonly(void) { char *p = fresh(PROT_READ); want = p + 24; *(volatile long *)(p + 24) = 1; }
static void load_none(void) { char *p = fresh(PROT_NONE); want = p + 4; sig = *(volatile int *)(p + 4); }
static void null_store(void) { want = (void *)0x10; *(volatile int *)0x10 = 1; }
static void past_first_page(void) { want = (void *)0x1010; *(volatile int *)0x1010 = 1; }

// A write barrier: the page read-only until a store, which the handler
// lets through by making it writable and returning (no longjmp).
static char *wb;
static void wb_fault(int s, siginfo_t *si, void *uc) {
    (void)s, (void)uc;
    barrier_faults++;
    mprotect((void *)((uintptr_t)si->si_addr & ~(page - 1)), page, PROT_READ | PROT_WRITE);
}
static void barrier(void) {
    struct sigaction sa = {0};
    sa.sa_sigaction = wb_fault, sa.sa_flags = SA_SIGINFO;
    sigaction(SIGSEGV, &sa, 0), sigaction(SIGBUS, &sa, 0);
    wb = fresh(PROT_READ);
    wb[1] = 7, wb[2] = 8;
    mprotect(wb, page, PROT_READ);
    wb[3] = 9;
    printf("write barrier: %d faults for 3 stores, the stores %s\n", barrier_faults, wb[1] == 7 && wb[2] == 8 && wb[3] == 9 ? "made" : "lost");
    _exit(0);
}

// A stack overflow on a stack with a guard page below it, caught on an
// alternate stack.
static volatile long depth;
static long recurse(long n) {
    volatile char pad[512];
    pad[0] = (char)n;
    depth = n;
    return recurse(n + 1) + pad[0];
}
static void overflow(void) {
    static char alt[65536];
    stack_t ss = {.ss_sp = alt, .ss_size = sizeof alt};
    sigaltstack(&ss, 0);
    struct sigaction sa = {0};
    sa.sa_sigaction = on_fault, sa.sa_flags = SA_SIGINFO | SA_ONSTACK;
    sigaction(SIGSEGV, &sa, 0), sigaction(SIGBUS, &sa, 0);
    want = 0;

    recurse(0);
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    page = sysconf(_SC_PAGESIZE);
    report("a store to an unmapped page", unmapped);
    report("a store to a PROT_NONE page", none);
    report("a store to a read-only page", readonly);
    report("a load from a PROT_NONE page", load_none);
    report("a store at 0x10", null_store);
    report("a store at 0x1010", past_first_page);
    fflush(stdout);
    pid_t c = fork();
    if (c == 0) { alarm(10); barrier(); }
    int st;
    waitpid(c, &st, 0);
    if (!WIFEXITED(st)) printf("write barrier: the child ended by signal %d\n", WIFSIGNALED(st) ? WTERMSIG(st) : -1);
    fflush(stdout);
    c = fork();
    if (c == 0) {
        alarm(10);
        if (sigsetjmp(back, 1)) { printf("stack overflow: caught on the alternate stack (%s)\n", sig == SIGSEGV ? "SIGSEGV" : "SIGBUS"); _exit(0); }
        overflow();
        _exit(0);
    }
    waitpid(c, &st, 0);
    if (!WIFEXITED(st)) printf("stack overflow: the child ended by signal %d\n", WIFSIGNALED(st) ? WTERMSIG(st) : -1);
    return 0;
}
