// Signals: actions and masks in Darwin's layouts and numbers, a handler
// entered through its trampoline with XNU's frame, and what a signal does to
// a call that is waiting.
#include "t.h"

static volatile int got, got_sig, in_mask_ok, info_ok, on_alt;
static char alt[65536];

static u32 mask_now(void) {
    u32 m = 0;
    SYS(SYS_sigprocmask, SIG_BLOCK, 0, &m);
    return m;
}

static void handler(int sig, struct t_siginfo *si, struct t_ucontext *uc) {
    got++;
    got_sig = sig;
    info_ok = si && si->si_signo == sig && uc && uc->uc_mcsize >= sizeof(struct t_mcontext) && uc->uc_mcontext != 0;
    // The signal itself and the action's mask are blocked while it runs.
    u32 m = mask_now();
    in_mask_ok = (m & T_SIGBIT(sig)) != 0 && (m & T_SIGBIT(SIGHUP)) != 0;
    char here;
    on_alt = &here >= alt && &here < alt + sizeof alt;
}

#if defined(__arm64__)
static void skip(int sig, struct t_siginfo *si, struct t_ucontext *uc) {
    (void)si;
    got++;
    got_sig = sig;
    // Past the trapping instruction, with a value for the caller.
    uc->uc_mcontext->pc += 4;
    uc->uc_mcontext->x[0] = 1234;
}
#endif

static void plain(int sig) {
    got++;
    got_sig = sig;
}

static long fork_(void) {
    long flag = -1;
    long pid = t_syscall2(&flag, SYS_fork, 0, 0, 0);
    return pid < 0 ? pid : flag ? 0 : pid;
}

// The status a child ends with, whatever signals arrive meanwhile.
static int wait_for(long pid) {
    int status = -1;
    while (SYS(SYS_wait4, pid, &status, 0, 0) == -4) {
    }
    return status;
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    long me = SYS(SYS_getpid);
    struct t_sigaction old;

    t_show("sigaction", t_sigaction(SIGUSR1, handler, SA_SIGINFO, T_SIGBIT(SIGHUP), &old));
    t_check("it was the default", old.handler == 0);
    t_show("sigaction again", t_sigaction(SIGUSR1, handler, SA_SIGINFO, T_SIGBIT(SIGHUP), &old));
    t_check("the old action is the new one", old.handler == (void *)handler && old.mask == T_SIGBIT(SIGHUP) && old.flags == SA_SIGINFO);
    t_show("sigaction SIGKILL", t_sigaction(SIGKILL, handler, 0, 0, 0));
    t_show("sigaction of no signal", t_sigaction(0, handler, 0, 0, 0));
    t_show("sigaction past the last", t_sigaction(32, handler, 0, 0, 0));

    t_show("kill myself", SYS(SYS_kill, me, SIGUSR1, 1));
    t_printf("handled %d of signal %d\n", got, got_sig);
    t_check("its siginfo and context", info_ok);
    t_check("its mask while it ran", in_mask_ok);
    t_check("the mask is back", mask_now() == 0);
    t_check("on the thread's own stack", !on_alt);

    // Blocked, it waits; unblocked, it is delivered.
    u32 set = T_SIGBIT(SIGUSR1) | T_SIGBIT(SIGUSR2), was = 99;
    t_show("sigprocmask block", SYS(SYS_sigprocmask, SIG_BLOCK, &set, &was));
    t_show("the mask before", was);
    t_show("the mask now", mask_now());
    SYS(SYS_kill, me, SIGUSR1, 1);
    u32 pending = 0;
    t_show("sigpending", SYS(SYS_sigpending, &pending));
    t_printf("handled %d, pending %x\n", got, pending);
    set = T_SIGBIT(SIGUSR1);
    t_show("sigprocmask unblock", SYS(SYS_sigprocmask, SIG_UNBLOCK, &set, 0));
    t_printf("handled %d, mask %x\n", got, mask_now());
    set = T_SIGBIT(SIGKILL) | T_SIGBIT(SIGSTOP) | T_SIGBIT(SIGTERM);
    t_show("sigprocmask set", SYS(SYS_sigprocmask, SIG_SETMASK, &set, 0));
    t_show("SIGKILL and SIGSTOP cannot be blocked", mask_now());
    set = 0;
    SYS(SYS_sigprocmask, SIG_SETMASK, &set, 0);
    t_show("sigprocmask of no kind", SYS(SYS_sigprocmask, 9, &set, 0));

    // Ignored, and a handler without SA_SIGINFO.
    t_sigaction(SIGUSR2, (void *)1, 0, 0, 0);
    SYS(SYS_kill, me, SIGUSR2, 1);
    t_printf("ignored: handled %d\n", got);
    t_sigaction(SIGUSR2, plain, 0, 0, 0);
    SYS(SYS_kill, me, SIGUSR2, 1);
    t_printf("handled %d of signal %d\n", got, got_sig);

    // On the alternate stack.
    struct {
        void *sp;
        u64 size;
        int flags;
    } ss = {alt, sizeof alt, 0}, oss = {0, 0, 0};
    t_show("sigaltstack", SYS(SYS_sigaltstack, &ss, &oss));
    t_check("there was none", oss.flags == 4);
    t_sigaction(SIGUSR1, handler, SA_SIGINFO | SA_ONSTACK, T_SIGBIT(SIGHUP), 0);
    SYS(SYS_kill, me, SIGUSR1, 1);
    t_check("the handler ran on it", on_alt);
    t_show("sigaltstack query", SYS(SYS_sigaltstack, 0, &oss));
    t_check("it is the one set", oss.sp == alt && oss.size == sizeof alt && oss.flags == 0);

#if defined(__arm64__)
    // A trap the handler steps over, changing the context it returns to.
    t_sigaction(SIGTRAP, skip, SA_SIGINFO, 0, 0);
    t_sigaction(SIGILL, skip, SA_SIGINFO, 0, 0);
    got = 0;
    register long x0 __asm__("x0") = 1;
    __asm__ volatile("brk #7" : "+r"(x0));
    long after = x0;
    t_printf("brk: %d trap, signal %d, x0 %ld\n", got, got_sig, after);
#endif

    // A signal ends a wait: the itimer's SIGALRM interrupts select.
    t_sigaction(SIGALRM, plain, 0, 0, 0);
    struct {
        struct t_timeval interval, value;
    } it = {{0, 0}, {0, 30000}}, oit;
    got = 0;
    t_show("setitimer", SYS(SYS_setitimer, 0, &it, &oit));
    struct t_timeval tv = {5, 0};
    t_show("select, interrupted", SYS(SYS_select, 0, 0, 0, 0, &tv));
    t_printf("handled %d of signal %d\n", got, got_sig);
    t_show("getitimer", SYS(SYS_getitimer, 0, &oit));
    t_check("the timer is spent", oit.value.tv_sec == 0 && oit.value.tv_usec == 0);

    // sigsuspend waits for one, with a mask of its own for the wait.
    got = 0;
    it.value.tv_usec = 20000;
    SYS(SYS_setitimer, 0, &it, 0);
    t_show("sigsuspend", SYS(SYS_sigsuspend, T_SIGBIT(SIGUSR1)));
    t_printf("handled %d of signal %d, mask %x\n", got, got_sig, mask_now());

    // A write to a pipe nobody reads: SIGPIPE, then EPIPE.
    long wr = -1;
    long rd = t_syscall2(&wr, SYS_pipe, 0, 0, 0);
    SYS(SYS_close, rd);
    t_sigaction(SIGPIPE, plain, 0, 0, 0);
    got = 0;
    t_show("write a pipe with no reader", SYS(SYS_write, wr, "x", 1));
    t_printf("handled %d of signal %d\n", got, got_sig);

    // A child's end is SIGCHLD.
    t_sigaction(SIGCHLD, plain, 0, 0, 0);
    got = 0;
    long pid = fork_();
    if (pid == 0) t_exit(0);
    int status = wait_for(pid);
    t_printf("a child ended: handled %d of signal %d, status %x\n", got, got_sig, status);
    t_sigaction(SIGCHLD, 0, 0, 0, 0);

    // SA_RESETHAND: the second one is the default action's, which kills.
    pid = fork_();
    if (pid == 0) {
        t_sigaction(SIGUSR1, plain, SA_RESETHAND, 0, 0);
        got = 0;
        long self = SYS(SYS_getpid);
        SYS(SYS_kill, self, SIGUSR1, 1);
        t_printf("child: handled %d\n", got);
        SYS(SYS_kill, self, SIGUSR1, 1);
        t_printf("child: still here\n");
        t_exit(0);
    }
    t_printf("the child's status %x\n", wait_for(pid));
    // And a fault with no handler ends the process with its signal.
    pid = fork_();
    if (pid == 0) {
        t_sigaction(SIGSEGV, 0, 0, 0, 0);
        t_sigaction(SIGBUS, 0, 0, 0, 0);
        // (far outside anything mapped)
        *(volatile int *)0x700000000000 = 1;
        t_exit(0);
    }
    t_printf("a wild write's status %x\n", wait_for(pid) & 0x7f);
    return 0;
}
