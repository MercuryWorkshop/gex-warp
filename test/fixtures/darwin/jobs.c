// Job control: a child a signal stops goes no further until it is
// continued, and its parent's wait is told of each when it asks.
#include "t.h"

enum { WNOHANG = 1, WUNTRACED = 2, WCONTINUED = 0x10 };

static void nap(long us) {
    struct t_timeval tv = {0, us};
    SYS(SYS_select, 0, 0, 0, 0, &tv);
}

static long rd;

// How many bytes the child has written since this was last asked.
static long written(void) {
    char buf[256];
    long n, total = 0;
    while ((n = SYS(SYS_read, rd, buf, sizeof buf)) > 0) total += n;
    return total;
}

// Wait up to two seconds for word of the child that `how` asks after.
static void heard(const char *what, long pid, long how) {
    int status = -1;
    long r = 0;
    for (int i = 0; i < 200 && r == 0; i++) {
        r = SYS(SYS_wait4, pid, &status, how | WNOHANG, 0);
        if (r == 0) nap(10000);
    }
    t_printf("%s: %s", what, r == pid ? "the child" : r == 0 ? "nothing" : "an error");
    if (r == pid) t_printf(", status 0x%x", status);
    t_printf("\n");
}

static void runs(const char *what, int should) {
    written();
    nap(150000);
    long n = written();
    t_printf("%s: %s\n", what, (n > 0) == should ? (should ? "it runs" : "it does nothing") : (should ? "FAIL it does nothing" : "FAIL it runs"));
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    long wr = -1;
    rd = t_syscall2(&wr, SYS_pipe, 0, 0, 0);
    SYS(SYS_fcntl, rd, F_SETFL, O_NONBLOCK);
    long second = 0;
    long pid = t_syscall2(&second, SYS_fork, 0, 0, 0);
    if (second) {
        // The job: in a group of its own, which its parent controls, saying
        // that it runs every hundredth of a second.
        SYS(SYS_setpgid, 0, 0);
        for (;;) {
            SYS(SYS_write, wr, ".", 1);
            nap(10000);
        }
    }
    SYS(SYS_setpgid, pid, pid);
    runs("a child", 1);
    int status = -1;
    t_show("a wait that asks after one stopped, of one that runs", SYS(SYS_wait4, pid, &status, WUNTRACED | WNOHANG, 0));

    t_show("stop it", SYS(SYS_kill, pid, SIGSTOP));
    heard("a wait that asks after one stopped", pid, WUNTRACED);
    t_show("  asked again", SYS(SYS_wait4, pid, &status, WUNTRACED | WNOHANG, 0));
    runs("stopped", 0);
    t_show("stop it again", SYS(SYS_kill, pid, SIGSTOP));
    nap(50000);
    t_show("  a wait that does not ask after that", SYS(SYS_wait4, pid, &status, WNOHANG, 0));

    t_show("continue it", SYS(SYS_kill, pid, SIGCONT));
    runs("continued", 1);
    heard("a wait that asks after one continued", pid, WCONTINUED);
    t_show("  asked again", SYS(SYS_wait4, pid, &status, WCONTINUED | WUNTRACED | WNOHANG, 0));

    // The signal a terminal sends for ^Z, which a process may catch and
    // this one does not.
    t_show("the terminal's stop", SYS(SYS_kill, pid, SIGTSTP));
    heard("a wait that asks", pid, WUNTRACED);
    runs("stopped", 0);
    t_show("continue it", SYS(SYS_kill, pid, SIGCONT));
    runs("continued", 1);
    // (Not asked after, the news of its continuing is not kept from a wait
    // for the next stop.)
    t_show("stop it", SYS(SYS_kill, pid, SIGSTOP));
    heard("a wait that asks after one stopped", pid, WUNTRACED);

    // Stopped, it can still be killed.
    t_show("kill it as it is", SYS(SYS_kill, pid, SIGKILL));
    status = -1;
    t_check("it is waited for", SYS(SYS_wait4, pid, &status, 0, 0) == pid);
    t_printf("  status 0x%x\n", status);
    return 0;
}
