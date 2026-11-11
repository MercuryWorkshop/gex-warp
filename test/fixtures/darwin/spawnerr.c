// posix_spawn's answers, which XNU gives before there is a child: a
// directory is no program (EACCES), a directory the file actions move to
// must be one (ENOTDIR, ENOENT); and a child put in a group of its own
// (POSIX_SPAWN_SETPGROUP) is in it when the call returns, not a moment
// later. Each spawn is a line: its answer, and for a child that runs,
// whether it was in its own group at once and later, and how it ended.
#include <errno.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;

static void spawn(const char *prog, char *const argv[], const char *cwd) {
    posix_spawn_file_actions_t fa;
    posix_spawn_file_actions_init(&fa);
    for (int fd = 0; fd < 3; fd++) posix_spawn_file_actions_addinherit_np(&fa, fd);
    if (cwd) posix_spawn_file_actions_addchdir_np(&fa, cwd);
    posix_spawnattr_t at;
    posix_spawnattr_init(&at);
    posix_spawnattr_setflags(&at, POSIX_SPAWN_CLOEXEC_DEFAULT | POSIX_SPAWN_SETPGROUP);
    posix_spawnattr_setpgroup(&at, 0);
    pid_t pid = 0;
    int e = posix_spawn(&pid, prog, &fa, &at, argv, environ);
    printf("%s in %s: %d", prog, cwd ? cwd : "here", e);
    if (!e) {
        int now = getpgid(pid) == pid;
        usleep(200000);
        int later = getpgid(pid) == pid;
        kill(pid, SIGTERM);
        int st = 0;
        waitpid(pid, &st, 0);
        printf(", own group at once %d and later %d, status %d", now, later, st);
    }
    printf("\n");
    posix_spawn_file_actions_destroy(&fa);
    posix_spawnattr_destroy(&at);
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    char *sleep_[] = {"/bin/sleep", "1", NULL};
    spawn("/bin/sleep", sleep_, NULL);
    char *sh[] = {"/bin/sh", "-c", "sleep 1", NULL};
    spawn("/bin/sh", sh, NULL);
    char *usr[] = {"/usr", NULL};
    spawn("/usr", usr, NULL);
    char *echo[] = {"/bin/echo", "ran", NULL};
    spawn("/bin/echo", echo, "/etc/hosts");
    spawn("/bin/echo", echo, "/nonexistent");
    return 0;
}
