// core/mount-fork: the mounts a process made are its children's, across a
// fork and an exec (on the page each process has a kernel of its own, so
// what a child has of its parent's table is what it is told). The process
// mounts the system's proc and binds its /dev into a directory that has a
// stray regular file where /dev/null should be, binds the directory this
// program is in, mounts a tmpfs, and forks a child that chroots there: its
// /dev has the standard nodes and its /proc works; so does a child of its
// own's, and the program it executes (this one, with an argument).
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <dirent.h>
#include <unistd.h>

#ifndef MS_REC
#define MS_REC 16384
#endif

static int mounted(const char *at, const char *type) {
    char line[512];
    FILE *f = fopen("/proc/self/mounts", "r");
    if (!f) return -1;
    int seen = 0;
    while (fgets(line, sizeof line, f)) {
        char src[128], where[256], ty[64];
        if (sscanf(line, "%127s %255s %63s", src, where, ty) == 3 && !strcmp(where, at) && !strcmp(ty, type)) seen = 1;
    }
    fclose(f);
    return seen;
}

static const char *kind(const char *path) {
    struct stat st;
    if (lstat(path, &st)) return strerror(errno);
    if (S_ISCHR(st.st_mode)) return "char device";
    if (S_ISLNK(st.st_mode)) return "symlink";
    if (S_ISDIR(st.st_mode)) return "directory";
    return S_ISREG(st.st_mode) ? "regular file" : "other";
}

// What a process in the jail has: the standard nodes, and a /proc that is
// the system's.
static void check(const char *who) {
    unsigned char b[16];
    int fd;
    for (const char *n = "null"; n; n = !strcmp(n, "null") ? "zero" : !strcmp(n, "zero") ? "full" : !strcmp(n, "full") ? "random" : !strcmp(n, "random") ? "urandom" : !strcmp(n, "urandom") ? "tty" : !strcmp(n, "tty") ? "ptmx" : 0) {
        char p[64];
        snprintf(p, sizeof p, "/dev/%s", n);
        printf("%s: %s is a %s\n", who, p, kind(p));
    }
    char link[64] = "";
    ssize_t n = readlink("/dev/fd", link, sizeof link - 1);
    if (n > 0) link[n] = 0;
    printf("%s: /dev/fd -> %s\n", who, link);
    memset(b, 0, sizeof b);
    fd = open("/dev/urandom", O_RDONLY);
    n = fd >= 0 ? read(fd, b, sizeof b) : -1;
    int any = 0;
    for (int i = 0; i < 16; i++) any |= b[i];
    printf("%s: urandom read %ld bytes, not all zero: %s\n", who, (long)n, any ? "yes" : "no");
    if (fd >= 0) close(fd);
    memset(b, 0xff, sizeof b);
    fd = open("/dev/zero", O_RDONLY);
    n = fd >= 0 ? read(fd, b, sizeof b) : -1;
    int zero = 1;
    for (int i = 0; i < 16; i++) zero &= b[i] == 0;
    printf("%s: zero read %ld bytes, all zero: %s\n", who, (long)n, zero ? "yes" : "no");
    if (fd >= 0) close(fd);
    fd = open("/dev/full", O_WRONLY);
    n = fd >= 0 ? write(fd, "x", 1) : -1;
    printf("%s: full write: %s\n", who, n < 0 ? strerror(errno) : "ok");
    if (fd >= 0) close(fd);
    DIR *pts = opendir("/dev/pts");
    printf("%s: /dev/pts: %s\n", who, pts ? "opens" : strerror(errno));
    if (pts) closedir(pts);
    char line[256] = "";
    FILE *f = fopen("/proc/self/status", "r");
    if (f && fgets(line, sizeof line, f)) line[strcspn(line, "\n")] = 0;
    else strcpy(line, f ? "empty" : strerror(errno));
    printf("%s: /proc/self/status: %s\n", who, line);
    if (f) fclose(f);
    printf("%s: mounts: proc %d, devtmpfs %d, tmpfs %d\n", who, mounted("/proc", "proc"), mounted("/dev", "devtmpfs"), mounted("/tmp", "tmpfs"));
    fflush(stdout);
}

int main(int argc, char **argv) {
    if (argc > 1) {
        // The exec'd one, in the jail already.
        check("exec");
        return 0;
    }
    mkdir("/mnt", 0755);
    mkdir("/mnt/jail", 0755);
    mkdir("/mnt/jail/dev", 0755);
    mkdir("/mnt/jail/proc", 0755);
    mkdir("/mnt/jail/tmp", 0755);
    mkdir("/mnt/jail/g", 0755);
    // The stray file a tar of a /dev leaves where /dev/null was.
    int fd = open("/mnt/jail/dev/null", O_CREAT | O_WRONLY, 0644);
    if (fd >= 0) close(fd);
    printf("before: /mnt/jail/dev/null is a %s\n", kind("/mnt/jail/dev/null"));
    long r = syscall(SYS_mount, "proc", "/mnt/jail/proc", "proc", 0L, (void *)0);
    printf("mount proc: %ld %s\n", r, r ? strerror(errno) : "ok");
    r = syscall(SYS_mount, "/dev", "/mnt/jail/dev", (void *)0, (long)(MS_BIND | MS_REC), (void *)0);
    printf("bind /dev: %ld %s\n", r, r ? strerror(errno) : "ok");
    r = syscall(SYS_mount, "/g", "/mnt/jail/g", (void *)0, (long)MS_BIND, (void *)0);
    printf("bind /g: %ld %s\n", r, r ? strerror(errno) : "ok");
    r = syscall(SYS_mount, "none", "/mnt/jail/tmp", "tmpfs", 0L, (void *)0);
    printf("mount tmpfs: %ld %s\n", r, r ? strerror(errno) : "ok");
    fflush(stdout);
    pid_t pid = fork();
    if (pid == 0) {
        if (chroot("/mnt/jail") || chdir("/")) {
            printf("chroot: %s\n", strerror(errno));
            _exit(1);
        }
        check("child");
        pid_t grand = fork();
        if (grand == 0) {
            check("grandchild");
            _exit(0);
        }
        int st = 0;
        waitpid(grand, &st, 0);
        printf("grandchild ended %d\n", WEXITSTATUS(st));
        fflush(stdout);
        execl("/g/mount-fork", "mount-fork", "again", (char *)0);
        printf("exec: %s\n", strerror(errno));
        _exit(2);
    }
    int st = 0;
    waitpid(pid, &st, 0);
    printf("child ended %d\n", WEXITSTATUS(st));
    return 0;
}
