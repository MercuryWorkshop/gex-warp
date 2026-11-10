// posix_spawn: a new process running a program, with what it is to find
// said beforehand: descriptors opened, closed and moved, its directory,
// the signals it blocks and those put back to their defaults, its group.
#include "t.h"

enum { T_posix_spawn = 244, T_getpgid = 151 };
enum { SETPGROUP = 0x02, SETSIGDEF = 0x04, SETSIGMASK = 0x08, SETEXEC = 0x40, CLOEXEC_DEFAULT = 0x4000 };
enum { ACT_OPEN, ACT_CLOSE, ACT_DUP2, ACT_INHERIT, ACT_CHDIR = 5, ACT_FCHDIR = 6 };

// The attributes, as far as this sets them (the kernel reads more).
struct attr {
    short flags, pad;
    u32 sigdefault, sigmask;
    int pgroup;
    u8 rest[1024];
};
struct action {
    int kind, fd;
    union {
        struct {
            int oflag;
            u16 mode;
            char path[1024];
        } open;
        int newfd;
        char dir[1024];
    };
};
struct actions {
    int allocated, count;
    struct action act[6];
};
struct desc {
    u64 attr_size;
    struct attr *attr;
    u64 actions_size;
    struct actions *actions;
    u64 rest[32];
};

static char *self;
static char *const env[] = {"T_SPAWN=yes", 0};

static void copy(char *to, const char *from) {
    while ((*to++ = *from++)) {}
}

// The file action `i`: its kind, the descriptor it is about, and the other
// descriptor of a dup2.
static struct actions actions;
static struct action *act(int i, int kind, int fd, int newfd) {
    struct action *a = &actions.act[i];
    t_memset(a, 0, sizeof *a);
    a->kind = kind, a->fd = fd, a->newfd = newfd;
    return a;
}

// What the spawned program reports of itself.
static int child(char **argv, char **envp) {
    t_printf("  the child: argument %s, T_SPAWN=%s\n", argv[2] ? argv[2] : "(none)", t_getenv(envp, "T_SPAWN") ? t_getenv(envp, "T_SPAWN") : "(unset)");
    if (!t_strcmp(argv[2], "all")) {
        char buf[16];
        long n = SYS(SYS_read, 5, buf, sizeof buf - 1);
        buf[n > 0 ? n : 0] = 0;
        t_printf("  descriptor 5 reads \"%s\"\n", buf);
        t_show("  the pipe's other end, closed", SYS(SYS_close, (long)(argv[3][0] - '0')));
        long there = SYS(SYS_open, "here", O_RDONLY);
        t_check("  in the directory it was sent to", there >= 0);
        u32 mask = 0;
        SYS(SYS_sigprocmask, SIG_BLOCK, 0, &mask);
        struct {
            void *handler;
            u32 mask;
            int flags;
        } old;
        SYS(SYS_sigaction, SIGUSR2, 0, &old);
        t_printf("  SIGUSR1 is %s, SIGUSR2 %s\n", mask & T_SIGBIT(SIGUSR1) ? "blocked" : "not blocked", old.handler == 0 ? "at its default" : old.handler == (void *)1 ? "ignored" : "caught");
        t_check("  in a group of its own", SYS(T_getpgid, 0) == SYS(SYS_getpid));
    }
    if (!t_strcmp(argv[2], "fds")) t_printf("  descriptor 9 is %s, 8 is %s\n", SYS(SYS_fcntl, 9, 1) >= 0 ? "open" : "closed", SYS(SYS_fcntl, 8, 1) >= 0 ? "open" : "closed");
    return !t_strcmp(argv[2], "all") ? 7 : 0;
}

// Wait for `pid`, and say how it ended.
static void reap(int pid) {
    int status = 0;
    long got = SYS(SYS_wait4, pid, &status, 0, 0);
    t_printf("the child %s, status %x\n", got == pid ? "was waited for" : "was not there", status);
}

// Spawn the program with `argv`, its output this one's: what the call
// answered is said once the child has had its say and ended.
static void spawn(const char *what, void *desc, char **argv) {
    int pid = -1;
    long r = SYS(T_posix_spawn, &pid, self, desc, argv, env);
    int status = 0;
    long got = r == 0 ? SYS(SYS_wait4, pid, &status, 0, 0) : 0;
    t_show(what, r);
    t_printf("a pid came back: %d; the child %s, status %x\n", pid > 0 && pid != SYS(SYS_getpid), got == pid ? "was waited for" : "was not there", status);
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)apple;
    if (argc > 2 && !t_strcmp(argv[1], "child")) return child(argv, envp);
    self = argv[0];
    static struct attr attr;
    static struct desc desc;
    int pid = -1;

    // With nothing said: the program, the caller's descriptors.
    char *plain[] = {self, "child", "plain", 0};
    spawn("spawn, nothing asked", 0, plain);

    // With everything: its output down a pipe, a file as descriptor 5, a
    // directory, signals, a group.
    long fd = SYS(SYS_open, "five", O_CREAT | O_WRONLY, 0644);
    SYS(SYS_write, fd, "five", 4);
    SYS(SYS_close, fd);
    SYS(SYS_mkdir, "d", 0755);
    fd = SYS(SYS_open, "d/here", O_CREAT | O_WRONLY, 0644);
    SYS(SYS_close, fd);
    int ends[2];
    long second = 0;
    ends[0] = (int)t_syscall2(&second, SYS_pipe, 0, 0, 0), ends[1] = (int)second;
    t_sigaction(SIGUSR2, (void *)1, 0, 0, 0);
    attr.flags = SETSIGMASK | SETSIGDEF | SETPGROUP;
    attr.sigmask = T_SIGBIT(SIGUSR1), attr.sigdefault = T_SIGBIT(SIGUSR2), attr.pgroup = 0;
    actions.count = 4;
    act(0, ACT_DUP2, ends[1], 1);
    struct action *o = act(1, ACT_OPEN, 5, 0);
    o->open.oflag = O_RDONLY, copy(o->open.path, "five");
    act(2, ACT_CLOSE, ends[0], 0);
    copy(act(3, ACT_CHDIR, 0, 0)->dir, "d");
    desc.attr_size = sizeof attr, desc.attr = &attr;
    desc.actions_size = 8 + 4 * sizeof(struct action), desc.actions = &actions;
    char end[2] = {(char)('0' + ends[0]), 0};
    char *all[] = {self, "child", "all", end, 0};
    t_printf("an action is %zu bytes\n", sizeof(struct action));
    t_show("spawn, with attributes and file actions", SYS(T_posix_spawn, &pid, self, &desc, all, env));
    SYS(SYS_close, ends[1]);
    static char out[1024];
    long got = 0, n;
    while ((n = SYS(SYS_read, ends[0], out + got, sizeof out - 1 - got)) > 0) got += n;
    out[got] = 0;
    t_printf("down the pipe:\n%s", out);
    SYS(SYS_close, ends[0]);
    reap(pid);

    // Descriptors: the caller's unless said otherwise; with
    // CLOEXEC_DEFAULT, only those said.
    SYS(SYS_dup2, 1, 8), SYS(SYS_dup2, 1, 9);
    char *fds[] = {self, "child", "fds", 0};
    spawn("spawn", 0, fds);
    attr.flags = (short)CLOEXEC_DEFAULT;
    actions.count = 3;
    act(0, ACT_INHERIT, 9, 0), act(1, ACT_INHERIT, 1, 0), act(2, ACT_INHERIT, 2, 0);
    desc.actions_size = 8 + 3 * sizeof(struct action);
    spawn("spawn, only what is said kept open", &desc, fds);

    // What is found wrong before anything is made.
    pid = -5;
    t_show("spawn a program that is not there", SYS(T_posix_spawn, &pid, "no/such/program", 0, plain, env));
    t_check("and no pid", pid == -5);
    // (Asked for by name: the page's files have no modes to refuse by.)
    if (argc > 1 && !t_strcmp(argv[1], "modes")) t_show("spawn a file that may not be run", SYS(T_posix_spawn, &pid, "five", 0, plain, env));
    desc.actions_size = 100;
    t_show("spawn with file actions of the wrong size", SYS(T_posix_spawn, &pid, self, &desc, plain, env));

    // SETEXEC: no new process, the caller becomes the program. (A fork's
    // second result says which of the two this is.)
    long is_child = 0;
    long forked = t_syscall2(&is_child, SYS_fork, 0, 0, 0);
    if (is_child) {
        attr.flags = SETEXEC;
        desc.actions_size = 0, desc.actions = 0;
        char *exec[] = {self, "child", "exec", 0};
        SYS(T_posix_spawn, 0, self, &desc, exec, env);
        t_exit(99);
    }
    t_printf("a fork that spawns in place:\n");
    reap((int)forked);
    return 0;
}
