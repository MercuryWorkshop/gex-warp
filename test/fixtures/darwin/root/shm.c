// POSIX shared memory by name, from a program of either system: says what
// the memory holds, writes its own system's name there, and runs the next
// program it is given (darwin/root: a Linux program and a Mac one share
// the memory one of them made).
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#ifdef __APPLE__
#define ME "Darwin"
#else
#define ME "Linux"
#endif

int main(int argc, char **argv) {
    static const char name[] = "/gex-test-root";
    int fd = shm_open(name, O_RDWR | O_CREAT, 0600);
    if (fd < 0) {
        perror(ME ": shm_open");
        return 1;
    }
    // (A Mac's is sized once: the second program's is refused, and is no
    // matter.)
    ftruncate(fd, 16384);
    char *p = mmap(0, 16384, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (p == MAP_FAILED) {
        perror(ME ": mmap");
        return 1;
    }
    printf("%s: the memory says \"%s\"\n", ME, p);
    fflush(stdout);
    strcpy(p, "written by " ME);
    munmap(p, 16384);
    if (argc > 1) execv(argv[1], argv + 1);
    shm_unlink(name);
    return 0;
}
