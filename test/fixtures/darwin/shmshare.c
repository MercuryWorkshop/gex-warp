// A POSIX shared memory object mapped by two processes that each open it by
// name: a process posix_spawn starts, and a forked child that maps it after
// the fork. Each sees the other's stores while both run (no exit, no msync
// between): each waits, with a time limit, for the other's store to show.
#include <fcntl.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

extern char **environ;

/// Whether `*p` comes to hold `want` within two seconds.
static int comes(volatile char *p, char want) {
    for (int i = 0; i < 2000; i++) {
        if (*p == want) return 1;
        struct timespec t = {0, 1000000};
        nanosleep(&t, 0);
    }
    return 0;
}

static volatile char *open_map(const char *name, int create) {
    int fd = shm_open(name, O_RDWR | (create ? O_CREAT | O_EXCL : 0), 0600);
    if (fd < 0) return 0;
    if (create) ftruncate(fd, 16384);
    volatile char *p = mmap(0, 16384, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    close(fd);
    return p == MAP_FAILED ? 0 : p;
}

/// The other process: it reads what was there, stores 42, and waits to be
/// told (7) that it was seen.
static int other(const char *name) {
    volatile char *q = open_map(name, 0);
    if (!q) return 2;
    int had = q[0];
    q[1] = 42;
    int told = comes(q + 2, 7);
    return had == 5 && told ? 0 : 3;
}

static void run(const char *self, int spawned) {
    char name[64];
    snprintf(name, sizeof name, "/gexshm.%d.%d", (int)getpid(), spawned);
    volatile char *p = open_map(name, 1);
    if (!p) {
        printf("%s: shm_open failed\n", spawned ? "spawned" : "forked");
        return;
    }
    p[0] = 5;
    pid_t pid;
    if (spawned) {
        char *args[] = {(char *)self, "other", name, 0};
        if (posix_spawn(&pid, self, 0, 0, args, environ)) {
            printf("spawned: posix_spawn failed\n");
            return;
        }
    } else {
        pid = fork();
        if (pid == 0) _exit(other(name));
    }
    int seen = comes(p + 1, 42);
    p[2] = 7;
    int st;
    waitpid(pid, &st, 0);
    printf("%s: the other's store seen while it runs: %s; it read ours and saw ours seen: %s\n", spawned ? "spawned" : "forked",
           seen ? "yes" : "no", WIFEXITED(st) && WEXITSTATUS(st) == 0 ? "yes" : "no");
    shm_unlink(name);
}

int main(int argc, char **argv) {
    if (argc == 3 && strcmp(argv[1], "other") == 0) return other(argv[2]);
    setvbuf(stdout, 0, _IONBF, 0);
    run(argv[0], 0);
    run(argv[0], 1);
    return 0;
}
