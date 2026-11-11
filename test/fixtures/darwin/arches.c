// A program of one processor that runs a program of the other, in one
// system: an arm64 program and an x86-64 one are this same source, built
// twice, and each is given the other's name. A Mac runs both (the x86-64 one
// under Rosetta), so what the two say there is what they must say here.
//
//   arches OTHER SELF       the checks: OTHER as a child, through a pipe, and
//                           there and back with no fork between
//   arches say WORD...      say who this is and the words, and exit 3
//   arches hop PROGRAM ...  say so, and become PROGRAM with the rest
#include "t.h"

#ifdef __x86_64__
#define ME "x86-64"
#else
#define ME "arm64"
#endif

static long fork_(void) {
    long child_flag = -1;
    long pid = t_syscall2(&child_flag, SYS_fork, 0, 0, 0);
    if (pid < 0) return pid;
    return child_flag ? 0 : pid;
}

static void waited(const char *what, long pid) {
    int status = -1;
    long got = SYS(SYS_wait4, pid, &status, 0, 0);
    t_printf("%s: %s, status %x\n", what, got == pid ? "waited for" : "not waited for", status);
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)apple;
    if (argc > 1 && !t_strcmp(argv[1], "say")) {
        t_printf("%s:", ME);
        for (int i = 2; i < argc; i++) t_printf(" %s", argv[i]);
        t_printf(" (T_VAR %s)\n", t_getenv(envp, "T_VAR"));
        return 3;
    }
    if (argc > 2 && !t_strcmp(argv[1], "hop")) {
        t_printf("%s: becoming another\n", ME);
        long r = SYS(SYS_execve, argv[2], &argv[2], envp);
        t_printf("%s: execve = %ld\n", ME, r);
        return 9;
    }
    if (argc < 3) {
        t_printf("arches OTHER SELF\n");
        return 2;
    }
    char *other = argv[1], *self = argv[2];
    t_printf("%s: the first\n", ME);

    // The other processor's program as a child: its words, its status.
    long pid = fork_();
    if (pid == 0) {
        char *args[] = {other, "say", "a", "child", 0};
        t_printf("execve = %ld\n", SYS(SYS_execve, other, args, envp));
        SYS(SYS_exit, 9);
    }
    waited("the other as a child", pid);

    // What it writes read through a pipe made here: the descriptors go
    // with the process into the other's program.
    long wr = -1;
    long rd = t_syscall2(&wr, SYS_pipe, 0, 0, 0);
    pid = fork_();
    if (pid == 0) {
        SYS(SYS_close, rd);
        SYS(SYS_dup2, wr, 1);
        SYS(SYS_close, wr);
        char *args[] = {other, "say", "through", "a", "pipe", 0};
        SYS(SYS_execve, other, args, envp);
        SYS(SYS_exit, 9);
    }
    SYS(SYS_close, wr);
    char buf[128];
    long n = 0, got;
    while (n < (long)sizeof buf - 1 && (got = SYS(SYS_read, rd, buf + n, sizeof buf - 1 - n)) > 0) n += got;
    buf[n] = 0;
    SYS(SYS_close, rd);
    t_printf("read %ld bytes: %s", n, buf);
    waited("the writer", pid);

    // There and back with no fork between: one process is the other's
    // program, and then this one's again.
    pid = fork_();
    if (pid == 0) {
        char *args[] = {other, "hop", self, "say", "back", "again", 0};
        SYS(SYS_execve, other, args, envp);
        SYS(SYS_exit, 9);
    }
    waited("there and back", pid);
    t_printf("%s: the first, still\n", ME);
    return 0;
}
