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
    t_check("fork", pid > 0 && pid != me);
    waited("exit 3", pid);

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
    return 0;
}
