/* Calls into libcrashinfo.dylib with a pointer nothing maps:
   crashinfo.test.js. "handle": a SIGSEGV handler ends the run (nothing is
   to be said); "abort": abort() instead of the bad pointer. */
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
int crash_outer(volatile int *p);

static void on_segv(int sig) {
  (void)sig;
  write(1, "handled\n", 8);
  _exit(0);
}

int main(int argc, char **argv) {
  puts("before");
  fflush(stdout);
  if (argc > 1 && !strcmp(argv[1], "handle")) signal(SIGSEGV, on_segv);
  if (argc > 1 && !strcmp(argv[1], "abort")) abort();
  /* (Not a tail call: main keeps its frame, which the backtrace shows.) */
  int r = crash_outer(argc > 5 ? 0 : (volatile int *)0x7f0000001000ULL);
  puts("after");
  return r;
}
