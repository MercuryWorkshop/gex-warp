/* A signal taken at a loop's back edge resumes with the loop's registers as
 * they were. The loop is several blocks (a branch inside the body), so it is
 * translated as a region, which holds every register it touches in a local
 * from entry to exit; the safepoint on its back edge has to write those
 * locals to the context before a handler frame is built from it. Otherwise
 * sigreturn restores the registers as they were when the region was entered:
 * a counter restarts while the memory the loop wrote does not.
 *
 * spin counts iterations in a register and in memory until enough SIGALRMs
 * have arrived (the timer fires every millisecond); the two counts agree and
 * the odd ones are half of them, rounded down. libobjc's atfork handler is
 * such a loop over its lock tables, and with a stale index it unlocked a
 * lock twice. */
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/time.h>

volatile int signals;

/* spin(&signals, counts, want): counts[0] every iteration, counts[1] the odd
 * ones; returns the register's count. */
long spin(volatile int *sig, long *counts, int want);

#ifdef __x86_64__
__asm__(
    ".text\n"
    ".globl spin\n"
    ".type spin,@function\n"
    "spin:\n"
    "  push %r15\n"
    "  xor %r15d, %r15d\n"
    "1: testb $1, %r15b\n"
    "  jz 2f\n"
    "  incq 8(%rsi)\n"
    "2: incq (%rsi)\n"
    "  inc %r15\n"
    "  cmp %edx, (%rdi)\n"
    "  jl 1b\n"
    "  mov %r15, %rax\n"
    "  pop %r15\n"
    "  ret\n");
#else
__asm__(
    ".text\n"
    ".globl spin\n"
    ".type spin,@function\n"
    "spin:\n"
    "  push %esi\n push %edi\n push %ebx\n"
    "  mov 16(%esp), %edi\n mov 20(%esp), %ebx\n mov 24(%esp), %edx\n"
    "  xor %esi, %esi\n"
    "1: test $1, %esi\n"
    "  jz 2f\n"
    "  incl 4(%ebx)\n"
    "2: incl (%ebx)\n"
    "  inc %esi\n"
    "  cmp %edx, (%edi)\n"
    "  jl 1b\n"
    "  mov %esi, %eax\n"
    "  pop %ebx\n pop %edi\n pop %esi\n"
    "  ret\n");
#endif

static void on_alarm(int s) {
    (void)s;
    signals++;
}

int main(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = on_alarm;
    sigaction(SIGALRM, &sa, 0);
    struct itimerval it = {{0, 1000}, {0, 1000}};
    setitimer(ITIMER_REAL, &it, 0);
    long counts[2] = {0, 0};
    long n = spin(&signals, counts, 50);
    struct itimerval off = {{0, 0}, {0, 0}};
    setitimer(ITIMER_REAL, &off, 0);
    printf("register count matches memory: %s\n", n == counts[0] ? "yes" : "no");
    printf("odd iterations are half: %s\n", counts[1] == counts[0] / 2 ? "yes" : "no");
    if (n != counts[0])
        fprintf(stderr, "register %ld, memory %ld\n", n, counts[0]);
    return n == counts[0] && counts[1] == counts[0] / 2 ? 0 : 1;
}
