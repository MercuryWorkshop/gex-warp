// exepath.c's Linux half: /proc/self/exe and AT_EXECFN, the program's own
// path, with argv[0] apart from it.
#include <stdio.h>
#include <unistd.h>
#include <sys/auxv.h>
int main(int c, char **v) { char b[512]; ssize_t n = readlink("/proc/self/exe", b, sizeof b - 1); b[n > 0 ? n : 0] = 0;
  printf("argv[0] %s\n/proc/self/exe %s\nAT_EXECFN %s\n", v[0], b, (char *)getauxval(AT_EXECFN)); return 0; }
