// Which system this is a program of and where it stands; the note the
// last program left in /tmp and in the working directory, and its own
// left in their place; then the program it is given, run as a child.
// Built twice from this one file, for Linux and for a Mac
// (test/darwin/root.test.js).
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <unistd.h>

static const char *note(const char *path, const char *mine) {
    static char said[2][32];
    static int turn;
    char *b = said[turn++ & 1];
    int fd = open(path, O_RDONLY);
    long n = fd < 0 ? -1 : read(fd, b, 31);
    if (fd >= 0) close(fd);
    b[n < 0 ? 0 : n] = 0;
    fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd >= 0) dprintf(fd, "%s", mine), close(fd);
    return n < 0 ? "none" : b;
}

int main(int argc, char **argv) {
    struct utsname u;
    struct stat st;
    char cwd[256] = "nowhere";
    uname(&u);
    getcwd(cwd, sizeof cwd);
    int null = open("/dev/null", O_RDWR);
    printf("%s: in %s; a Mac's linker %s; /Volumes/Linux %s; /dev/null %s", u.sysname, cwd, stat("/usr/lib/dyld", &st) == 0 ? "yes" : "no",
           stat("/Volumes/Linux", &st) == 0 ? "yes" : "no", null >= 0 ? "yes" : "no");
    printf("; the note in /tmp: %s", note("/tmp/note", u.sysname));
    printf("; the note here: %s\n", note("note", u.sysname));
    fflush(stdout);
    if (argc < 2) return 41;
    pid_t child = fork();
    if (child == 0) {
        execv(argv[1], argv + 1);
        perror(argv[1]);
        _exit(127);
    }
    int status = 0;
    waitpid(child, &status, 0);
    printf("%s: its child ended %d\n", u.sysname, WIFEXITED(status) ? WEXITSTATUS(status) : -1);
    return 0;
}
