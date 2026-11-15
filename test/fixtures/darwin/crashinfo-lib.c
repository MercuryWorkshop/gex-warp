/* A dylib whose function dereferences a bad pointer, two calls deep, for
   crashinfo.test.js. noinline and a frame pointer, so the backtrace has
   frames to walk. crash_trap ends the process with an instruction: one its
   processor leaves undefined (ud2, udf), or the compiler's trap (ud2 on
   x86-64, brk on arm64). */
__attribute__((noinline, visibility("default"))) int crash_inner(volatile int *p) {
  return *p;
}

__attribute__((noinline, visibility("default"))) int crash_outer(volatile int *p) {
  return crash_inner(p) + 1;
}

__attribute__((noinline, visibility("default"))) int crash_trap(int undefined) {
  if (undefined) {
#if defined(__x86_64__)
    __asm__ volatile("ud2");
#else
    __asm__ volatile(".long 0x00000000");
#endif
    return 1;
  }
  __builtin_trap();
}
