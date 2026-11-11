// /prog of a root: the sum its root's /lib/libv.so makes, and then, with
// `wait`, waiting for a byte on its input before it ends (so it is still
// up while the other root's runs).
#include <stdio.h>
#include <string.h>
#include <unistd.h>

long v_sum(long n);

int main(int argc, char **argv) {
    printf("sum %ld\n", v_sum(200000));
    fflush(stdout);
    if (argc > 1 && !strcmp(argv[1], "wait")) {
        char c;
        (void)!read(0, &c, 1);
    }
    return 0;
}
