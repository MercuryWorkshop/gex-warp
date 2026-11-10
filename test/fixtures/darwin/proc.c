// Processes: fork's two results, wait4's status word with Darwin's signal
// numbers, kill, pipes across a fork, and execve.
#include "t.h"

static long fork_(void) {
    long child_flag = -1;
    long pid = t_syscall2(&child_flag, SYS_fork, 0, 0, 0);
    if (pid < 0) return pid;
    // Both processes get the child's pid; the second result says which is
    // which.
    return child_flag ? 0 : pid;
}

static void waited(const char *what, long pid) {
    int status = -1;
    long got = SYS(SYS_wait4, pid, &status, 0, 0);
    t_printf("%s: wait4 %s, status %x\n", what, got == pid ? "its pid" : "something else", status);
}

// A function that returns `n`, made at one fixed address and called: the
// machine's `mov` of a constant to the register a result is in, and `ret`.
static int code_at(u8 *where, int n) {
    u8 *at = (u8 *)SYS(SYS_mmap, where, 0x4000, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0);
    if (at != where) return -1;
#if defined(__arm64__)
    u32 code[2] = {0x52800000u | (u32)n << 5, 0xd65f03c0u};
#else
    u8 code[6] = {0xb8, (u8)n, 0, 0, 0, 0xc3};
#endif
    for (unsigned i = 0; i < sizeof code; i++) at[i] = ((const u8 *)code)[i];
    if (SYS(SYS_mprotect, at, 0x4000, PROT_READ | PROT_EXEC)) return -2;
    return ((int (*)(void))at)();
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)apple;
    if (argc > 1 && !t_strcmp(argv[1], "child")) {
        t_printf("exec'd: argc %d, argv[2] %s, T_VAR %s, T_NEW %s\n", argc, argc > 2 ? argv[2] : "-", t_getenv(envp, "T_VAR"),
                 t_getenv(envp, "T_NEW"));
        return 9;
    }
    long me = SYS(SYS_getpid);
    t_check("getpid", me > 0);
    t_check("getppid", SYS(SYS_getppid) >= 0);
    t_check("getuid is geteuid", SYS(SYS_getuid) == SYS(SYS_geteuid));
    t_check("getgid is getegid", SYS(SYS_getgid) == SYS(SYS_getegid));
    t_check("getpgrp", SYS(SYS_getpgrp) > 0);

    long pid = fork_();
    if (pid == 0) {
        t_check("child: a new pid", SYS(SYS_getpid) != me);
        t_check("child: its parent", SYS(SYS_getppid) == me);
        t_exit(3);
    }
    // (Said once the child has had its say and gone: the two write to the
    // same place, in whichever order they run.)
    waited("exit 3", pid);
    t_check("fork", pid > 0 && pid != me);

    // A pipe from the child, which the parent reads to the end.
    long wr = -1;
    long rd = t_syscall2(&wr, SYS_pipe, 0, 0, 0);
    pid = fork_();
    if (pid == 0) {
        SYS(SYS_close, rd);
        for (int i = 0; i < 3; i++) t_dprintf((int)wr, "line %d\n", i);
        t_exit(0);
    }
    SYS(SYS_close, wr);
    char buf[64];
    long total = 0, n;
    while ((n = SYS(SYS_read, rd, buf + total, sizeof buf - 1 - total)) > 0) total += n;
    buf[total] = 0;
    t_printf("from the child (%ld bytes):\n%s", total, buf);
    SYS(SYS_close, rd);
    waited("exit 0", pid);

    // Killed by signals, named by Darwin's numbers in the status.
    static const int sigs[] = {SIGTERM, SIGKILL, SIGUSR1, SIGUSR2, SIGBUS, SIGSYS, SIGHUP};
    for (unsigned i = 0; i < sizeof sigs / sizeof sigs[0]; i++) {
        long go = -1;
        long ready = t_syscall2(&go, SYS_pipe, 0, 0, 0);
        pid = fork_();
        if (pid == 0) {
            SYS(SYS_close, ready);
            SYS(SYS_close, go);
            for (;;) SYS(SYS_select, 0, 0, 0, 0, 0);
        }
        SYS(SYS_close, go);
        SYS(SYS_read, ready, buf, 1);
        SYS(SYS_close, ready);
        t_show("kill", SYS(SYS_kill, pid, sigs[i], 1));
        t_printf("signal %d", sigs[i]);
        waited("", pid);
    }
    t_show("kill a signal there is none of", SYS(SYS_kill, me, 99, 1));
    t_show("kill nobody", SYS(SYS_kill, 0x3ffffff, 0, 1));
    t_show("kill myself with 0", SYS(SYS_kill, me, 0, 1));
    int status = -1;
    t_show("wait4 with no child", SYS(SYS_wait4, -1, &status, 0, 0));
    pid = fork_();
    if (pid == 0) {
        for (;;) SYS(SYS_select, 0, 0, 0, 0, 0);
    }
    t_show("wait4 WNOHANG, child running", SYS(SYS_wait4, pid, &status, 1, 0));
    SYS(SYS_kill, pid, SIGKILL, 1);
    waited("killed", pid);

    pid = fork_();
    if (pid == 0) {
        char *args[] = {argv[0], "child", "an argument", 0};
        char *env[] = {"T_VAR=exec", "T_NEW=1", 0};
        long r = SYS(SYS_execve, argv[0], args, env);
        t_printf("execve = %ld\n", r);
        t_exit(1);
    }
    waited("exec", pid);
    pid = fork_();
    if (pid == 0) {
        char *args[] = {"nothing", 0};
        t_show("execve of a missing file", SYS(SYS_execve, "./nothing-here", args, envp));
        t_exit(4);
    }
    waited("failed exec", pid);

    // Code a child maps is the child's: its parent, mapping other code at
    // the same address afterwards, runs its own. (As a shell that loads a
    // module in a subshell and then another itself does: a loader puts a
    // library where the address space has room, the same place in both.)
    // (A place there is room at, found and let go before the two part.)
    u8 *room = (u8 *)SYS(SYS_mmap, 0, 0x4000, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    SYS(SYS_munmap, room, 0x4000);
    pid = fork_();
    if (pid == 0) {
        t_printf("the child's code says %d\n", code_at(room, 11));
        t_exit(0);
    }
    waited("the child with code of its own", pid);
    t_printf("the parent's, at the same address, says %d\n", code_at(room, 22));
    return 0;
}
