// A directory bound at a path whose parent is a symlink in the root
// (/tmp -> private/tmp): what each call sees of it.
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
#include <stdlib.h>
int main(int argc, char **argv) {
    const char *d = argc > 1 ? argv[1] : "/tmp/bound";
    char a[256], b[256];
    snprintf(a, sizeof a, "%s/a.tmp", d), snprintf(b, sizeof b, "%s/b.txt", d);
    DIR *dir = opendir(d);
    printf("opendir %s: %s\n", d, dir ? "ok" : strerror(errno));
    if (dir) closedir(dir);
    struct stat st;
    printf("stat: %s\n", stat(d, &st) == 0 ? "ok" : strerror(errno));
    int fd = open(a, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    printf("open O_CREAT %s: %s\n", a, fd >= 0 ? "ok" : strerror(errno));
    if (fd >= 0) write(fd, "x\n", 2), close(fd);
    printf("rename to %s: %s\n", b, rename(a, b) == 0 ? "ok" : strerror(errno));
    char real[1024];
    printf("realpath: %s\n", realpath(d, real) ? real : strerror(errno));
    printf("access W_OK: %s\n", access(d, W_OK) == 0 ? "ok" : strerror(errno));
    return 0;
}
