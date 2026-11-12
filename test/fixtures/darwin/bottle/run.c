// A Linux program that becomes the program its arguments name: how the
// page's runtime under Node starts a Mac program of a root that is a
// directory of its tree (the first program there is loaded as it is; a
// Mac root is entered at an execve).
#include <stdio.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 2) return 2;
    execv(argv[1], argv + 1);
    perror(argv[1]);
    return 127;
}
