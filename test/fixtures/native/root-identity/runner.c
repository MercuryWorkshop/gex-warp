// Runs /prog in two roots, /r1 and /r2 (a child `chroot`ed into each),
// the first kept up until the second has said its sum, and says what each
// said and how each ended.
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

// A child in ROOT running /prog (with `wait`, its input `in`); its output
// is the pipe whose read end is returned.
static int start(const char *root, int in, pid_t *pid) {
    int out[2];
    if (pipe(out) != 0) return -1;
    *pid = fork();
    if (*pid == 0) {
        if (in >= 0) dup2(in, 0);
        dup2(out[1], 1);
        close(out[0]);
        if (chroot(root) != 0 || chdir("/") != 0) {
            perror("chroot");
            _exit(8);
        }
        char *wait_args[] = {"/prog", "wait", 0}, *args[] = {"/prog", 0};
        execv("/prog", in >= 0 ? wait_args : args);
        perror("execv");
        _exit(9);
    }
    close(out[1]);
    return out[0];
}

static void said(const char *root, int fd) {
    char buf[64];
    long n = 0, got;
    while (n < (long)sizeof buf - 1 && (got = read(fd, buf + n, 1)) == 1 && buf[n] != '\n') n++;
    buf[n] = 0;
    printf("%s: %s\n", root, buf);
    fflush(stdout);
}

int main(void) {
    int hold[2];
    if (pipe(hold) != 0) return 1;
    // Its write end is the runner's alone: the first child reads to its end.
    fcntl(hold[1], F_SETFD, FD_CLOEXEC);
    pid_t a, b;
    int fa = start("/r1", hold[0], &a);
    close(hold[0]);
    said("/r1", fa);
    int fb = start("/r2", -1, &b);
    said("/r2", fb);
    int st = 0;
    waitpid(b, &st, 0);
    printf("/r2 ended %d\n", WEXITSTATUS(st));
    fflush(stdout);
    close(hold[1]);
    waitpid(a, &st, 0);
    printf("/r1 ended %d\n", WEXITSTATUS(st));
    return 0;
}
