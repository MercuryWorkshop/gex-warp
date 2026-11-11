// /lib/libv.so of a root: the same code in each root's, but for the
// constant V (-DV=1 in one root, -DV=2 in the other), so the two are laid
// out alike and loaded at the same address.
#ifndef V
#define V 1
#endif

__attribute__((noinline)) long v_step(long acc, long i) {
    return acc + V + (i & 0);
}

long v_sum(long n) {
    long acc = 0;
    for (long i = 0; i < n; i++) acc = v_step(acc, i);
    return acc;
}
