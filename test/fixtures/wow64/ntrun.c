// `gexc run ntrun.exe`: says its pointer size, for the test of PE detection.
#include <windows.h>
#include <stdio.h>

int main(int argc, char **argv) {
    printf("ntrun ok %d-bit\n", (int)(8 * sizeof(void *)));
    fflush(stdout);
    return 0;
}
