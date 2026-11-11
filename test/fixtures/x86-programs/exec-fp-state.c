// A new program's floating point is the processor's at power-on, whatever
// the program before it left: the x87 control word 0x37f (and MXCSR
// 0x1f80), not the old program's and not zero, which masks no exception
// and ends the program at its first inexact x87 result -- musl's strtod,
// or a float printed. And a child ended by a fault of its own (an
// instruction there is none of, one it may not run) is waited for like any
// other, its status the signal's.
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/wait.h>

static unsigned control(void) {
    unsigned short cw = 0;
    // (volatile: with no input, two of these are one to the compiler)
    __asm__ volatile("fnstcw %0" : "=m"(cw));
    return cw;
}

#ifdef __x86_64__
static unsigned mxcsr(void) {
    unsigned v = 0;
    __asm__ volatile("stmxcsr %0" : "=m"(v));
    return v;
}
#endif

static void ended(const char *what, pid_t pid) {
    int st = -1;
    pid_t got = waitpid(pid, &st, 0);
    printf("%s: %s, ", what, got == pid ? "waited for" : "not waited for");
    if (WIFSIGNALED(st)) printf("signal %d\n", WTERMSIG(st));
    else printf("exit %d\n", WEXITSTATUS(st));
}

int main(int argc, char **argv) {
    setvbuf(stdout, 0, _IONBF, 0);
    if (argc > 1) {
        volatile long double a = 1.5L, b = 3.0L;
        printf("a new program: control word %#x, 1.5/3 = %Lg, \"%s\" = %g\n", control(), a / b, argv[1], strtod(argv[1], 0));
#ifdef __x86_64__
        printf("a new program: mxcsr %#x\n", mxcsr() & 0xffc0);
#endif
        return 0;
    }
    printf("the first program: control word %#x\n", control());
    pid_t pid = fork();
    if (pid == 0) {
        // Single precision, rounding down, everything masked; and SSE
        // rounding toward zero.
        unsigned short odd = 0x47f;
        __asm__ volatile("fldcw %0" : : "m"(odd));
#ifdef __x86_64__
        unsigned sse = 0x7f80;
        __asm__ volatile("ldmxcsr %0" : : "m"(sse));
#endif
        printf("the child's own: control word %#x\n", control());
        execl("/proc/self/exe", argv[0], "0.1", (char *)0);
        _exit(9);
    }
    ended("the program it became", pid);
    printf("its parent's: control word %#x\n", control());

    pid = fork();
    if (pid == 0) {
        __asm__("ud2");
        _exit(0);
    }
    ended("an instruction there is none of", pid);
    pid = fork();
    if (pid == 0) {
        __asm__("hlt");
        _exit(0);
    }
    ended("an instruction it may not run", pid);
    return 0;
}
