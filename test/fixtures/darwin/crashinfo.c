/* Calls into libcrashinfo.dylib with a pointer nothing maps:
   crashinfo.test.js. "handle": a SIGSEGV handler ends the run (nothing is
   to be said); "abort": abort() instead of the bad pointer; "undefined"
   and "trap": an undefined instruction and the compiler's trap, in the
   dylib (and with a third word, handlers for SIGILL and SIGTRAP first). */
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
int crash_outer(volatile int *p);
int crash_trap(int undefined);

static void on_segv(int sig) {
  (void)sig;
  write(1, "handled\n", 8);
  _exit(0);
}

static void on_trap(int sig) {
  if (sig == SIGILL) write(1, "handled SIGILL\n", 15);
  else if (sig == SIGTRAP) write(1, "handled SIGTRAP\n", 16);
  _exit(0);
}

int main(int argc, char **argv) {
  puts("before");
  fflush(stdout);
  if (argc > 1 && !strcmp(argv[1], "handle")) signal(SIGSEGV, on_segv);
  if (argc > 1 && !strcmp(argv[1], "abort")) abort();
  if (argc > 1 && (!strcmp(argv[1], "undefined") || !strcmp(argv[1], "trap"))) {
    if (argc > 2) {
      signal(SIGILL, on_trap);
      signal(SIGTRAP, on_trap);
    }
    int r = crash_trap(argv[1][0] == 'u');
    puts("after");
    return r;
  }
  /* (Not a tail call: main keeps its frame, which the backtrace shows.) */
  int r = crash_outer(argc > 5 ? 0 : (volatile int *)0x7f0000001000ULL);
  puts("after");
  return r;
}
