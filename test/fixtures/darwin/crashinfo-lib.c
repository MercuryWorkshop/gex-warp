/* A dylib whose function dereferences a bad pointer, two calls deep, for
   crashinfo.test.js. noinline and a frame pointer, so the backtrace has
   frames to walk. */
__attribute__((noinline, visibility("default"))) int crash_inner(volatile int *p) {
  return *p;
}

__attribute__((noinline, visibility("default"))) int crash_outer(volatile int *p) {
  return crash_inner(p) + 1;
}
