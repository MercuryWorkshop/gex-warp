// psynch: the kernel's half of libpthread's mutexes, condition variables
// and read-write locks, asked as libpthread asks it. Each is sequence
// words kept by the program (L, how many have asked; U, how many were let
// go; S, how many the kernel answered); the kernel is called when somebody
// must wait or be woken, and what it answers are the bits those words take.
#include "t.h"
// (A waiter is written as its call; the rest of it starts as nothing.)
#pragma clang diagnostic ignored "-Wmissing-field-initializers"

enum {
    SYS_psynch_rw_longrdlock = 297, SYS_psynch_rw_yieldwrlock = 298, SYS_psynch_rw_downgrade = 299, SYS_psynch_rw_upgrade = 300,
    SYS_psynch_mutexwait = 301, SYS_psynch_mutexdrop = 302, SYS_psynch_cvbroad = 303, SYS_psynch_cvsignal = 304,
    SYS_psynch_cvwait = 305, SYS_psynch_rw_rdlock = 306, SYS_psynch_rw_wrlock = 307, SYS_psynch_rw_unlock = 308,
    SYS_psynch_rw_unlock2 = 309, SYS_psynch_cvclrprepost = 312,
};
// A mutex's options as libpthread passes them: its own process's, with
// the first-fit policy (the default) or the fair one; and that it is a
// mutex, for the call that takes back a prepost.
enum { PRIVATE = 0x20, FIRSTFIT = 0x80 | PRIVATE, FAIR = 0x40 | PRIVATE, IS_MUTEX = 0x2000 };
#define SEQ(n) ((u64)(n) << 8)
#define PAIR(high, low) ((u64)(high) << 32 | (u32)(low))

static void nap(unsigned ms) {
    static u32 never;
    SYS(SYS_ulock_wait, UL_COMPARE_AND_WAIT, &never, 0, ms * 1000);
}

// A thread that makes one waiting call and keeps its answer.
struct waiter {
    long n, a[8];
    volatile long answer;
    int handle;
};
static void *wait_once(void *arg) {
    struct waiter *w = arg;
    w->answer = t_syscall(w->n, w->a[0], w->a[1], w->a[2], w->a[3], w->a[4], w->a[5], w->a[6], w->a[7]);
    return 0;
}
static void start(struct waiter *w) {
    w->answer = 0x7777;
    w->handle = t_thread_start(wait_once, w);
    // Long enough for it to be on its queue.
    nap(60);
}
static void show(const char *what, struct waiter *w) {
    t_thread_join(w->handle);
    if (w->answer < 0)
        t_printf("%s = %ld\n", what, w->answer);
    else
        t_printf("%s = 0x%lx\n", what, w->answer);
}
static void hex(const char *what, long r) {
    if (r < 0)
        t_printf("%s = %ld\n", what, r);
    else
        t_printf("%s = 0x%lx\n", what, r);
}

static u64 m1[8], m2[8], m3[8], m4[8];

static void mutexes(void) {
    t_printf("a first-fit mutex\n");
    // Unlocked with a waiter its unlocker knew of, who is not here yet:
    // the next to come has it at once.
    hex("drop, nobody waiting", SYS(SYS_psynch_mutexdrop, m1, SEQ(2), SEQ(1), 0, FIRSTFIT));
    hex("the next to wait", SYS(SYS_psynch_mutexwait, m1, SEQ(3), SEQ(1), 0, FIRSTFIT));
    // Two drops are two.
    hex("drop", SYS(SYS_psynch_mutexdrop, m1, SEQ(4), SEQ(2), 0, FIRSTFIT));
    hex("drop", SYS(SYS_psynch_mutexdrop, m1, SEQ(5), SEQ(3), 0, FIRSTFIT));
    hex("wait", SYS(SYS_psynch_mutexwait, m1, SEQ(6), SEQ(3), 0, FIRSTFIT));
    hex("wait", SYS(SYS_psynch_mutexwait, m1, SEQ(7), SEQ(3), 0, FIRSTFIT));
    // A waiter, then the drop that wakes it.
    static struct waiter a = {SYS_psynch_mutexwait, {(long)m1, SEQ(8), SEQ(4), 0, FIRSTFIT}}, b = {SYS_psynch_mutexwait, {(long)m1, SEQ(9), SEQ(4), 0, FIRSTFIT}};
    start(&a);
    start(&b);
    hex("drop, two waiting", SYS(SYS_psynch_mutexdrop, m1, SEQ(9), SEQ(5), 0, FIRSTFIT));
    show("the first to have waited", &a);
    hex("drop, one waiting", SYS(SYS_psynch_mutexdrop, m1, SEQ(9), SEQ(6), 0, FIRSTFIT));
    show("the second", &b);
    // A drop that is taken back before anybody comes for it.
    hex("drop", SYS(SYS_psynch_mutexdrop, m2, SEQ(2), SEQ(1), 0, FIRSTFIT));
    hex("take it back", SYS(SYS_psynch_cvclrprepost, m2, SEQ(2), SEQ(1), 0, 0, SEQ(2), FIRSTFIT | IS_MUTEX));
    static struct waiter c = {SYS_psynch_mutexwait, {(long)m2, SEQ(3), SEQ(1), 0, FIRSTFIT}};
    start(&c);
    t_check("and the next to come waits", c.answer == 0x7777);
    hex("until a drop", SYS(SYS_psynch_mutexdrop, m2, SEQ(3), SEQ(2), 0, FIRSTFIT));
    show("which it has", &c);

    t_printf("a fair mutex\n");
    // The drop is for the next sequence, and no other.
    hex("drop, nobody waiting", SYS(SYS_psynch_mutexdrop, m3, SEQ(3), SEQ(1), 0, FAIR));
    hex("the one it was for", SYS(SYS_psynch_mutexwait, m3, SEQ(2), SEQ(1), 0, FAIR));
    static struct waiter d = {SYS_psynch_mutexwait, {(long)m3, SEQ(4), SEQ(2), 0, FAIR}}, e = {SYS_psynch_mutexwait, {(long)m3, SEQ(3), SEQ(2), 0, FAIR}};
    start(&d);
    start(&e);
    hex("drop, two waiting", SYS(SYS_psynch_mutexdrop, m3, SEQ(4), SEQ(2), 0, FAIR));
    show("the lower, which came second", &e);
    hex("drop", SYS(SYS_psynch_mutexdrop, m3, SEQ(4), SEQ(3), 0, FAIR));
    show("the higher", &d);
}

static u64 c1[8], c2[8], c3[8], c4[8], c5[8];

static void conditions(void) {
    t_printf("a condition\n");
    // (cv, S and L, U, mutex, its U and L, flags, seconds, nanoseconds)
    hex("wait, S past L", SYS(SYS_psynch_cvwait, c1, PAIR(SEQ(2), SEQ(1)), 0, 0, 0, PRIVATE, 0, 0));
    // Nobody signals: its time runs out, and it is told the kernel has
    // nothing more of the condition.
    u64 t0 = t_nanos();
    t_show("wait 30 ms, alone", SYS(SYS_psynch_cvwait, c1, PAIR(0, SEQ(1)), 0, 0, 0, PRIVATE, 0, 30000000));
    t_check("it waited", t_nanos() - t0 >= 30000000ull);
    // A signal before its waiter: kept, and taken.
    hex("signal, the waiter not here yet", SYS(SYS_psynch_cvsignal, c2, PAIR(0, SEQ(1)), 0, 0, 0, 0, 0, PRIVATE));
    hex("the waiter", SYS(SYS_psynch_cvwait, c2, PAIR(0, SEQ(1)), 0, 0, 0, PRIVATE, 0, 0));
    // Two wait; a signal for one, a broadcast for the rest.
    static struct waiter a = {SYS_psynch_cvwait, {(long)c3, PAIR(0, SEQ(1)), 0, 0, 0, PRIVATE, 0, 0}}, b = {SYS_psynch_cvwait, {(long)c3, PAIR(0, SEQ(2)), 0, 0, 0, PRIVATE, 0, 0}},
                         c = {SYS_psynch_cvwait, {(long)c3, PAIR(0, SEQ(3)), 0, 0, 0, PRIVATE, 5, 0}};
    start(&a);
    start(&b);
    start(&c);
    hex("signal, three waiting", SYS(SYS_psynch_cvsignal, c3, PAIR(0, SEQ(3)), 0, 0, 0, 0, 0, PRIVATE));
    show("the first", &a);
    // (cv, S and L, U and how many, flags, mutex, its words, thread)
    hex("broadcast", SYS(SYS_psynch_cvbroad, c3, PAIR(SEQ(1), SEQ(3)), PAIR(SEQ(1), SEQ(2)), PRIVATE, 0, 0, 0));
    show("the second", &b);
    show("the third", &c);
    // Two wait; the second gives up, then the first is signalled.
    static struct waiter d = {SYS_psynch_cvwait, {(long)c4, PAIR(0, SEQ(1)), 0, 0, 0, PRIVATE, 5, 0}};
    start(&d);
    t_show("a second waits 30 ms", SYS(SYS_psynch_cvwait, c4, PAIR(0, SEQ(2)), 0, 0, 0, PRIVATE, 0, 30000000));
    hex("signal", SYS(SYS_psynch_cvsignal, c4, PAIR(SEQ(1), SEQ(2)), 0, 0, 0, 0, 0, PRIVATE));
    show("the first", &d);
    // A signal for one thread by its port.
    static struct waiter e = {SYS_psynch_cvwait, {(long)c5, PAIR(0, SEQ(1)), 0, 0, 0, PRIVATE, 5, 0}}, f = {SYS_psynch_cvwait, {(long)c5, PAIR(0, SEQ(2)), 0, 0, 0, PRIVATE, 5, 0}};
    start(&e);
    start(&f);
    hex("signal the second by name", SYS(SYS_psynch_cvsignal, c5, PAIR(0, SEQ(2)), 0, t_thread_port(f.handle), 0, 0, 0, PRIVATE));
    show("it", &f);
    hex("signal a thread there is none of", SYS(SYS_psynch_cvsignal, c5, PAIR(SEQ(1), SEQ(2)), 0, 0x12345703, 0, 0, 0, PRIVATE));
    hex("signal", SYS(SYS_psynch_cvsignal, c5, PAIR(SEQ(1), SEQ(2)), 0, 0, 0, 0, 0, PRIVATE));
    show("the first", &e);
    hex("signal past L", SYS(SYS_psynch_cvsignal, c5, PAIR(0, SEQ(1)), SEQ(5), 0, 0, 0, 0, PRIVATE));
    // The wait drops its mutex as it begins: what the next to want the
    // mutex finds.
    t_show("wait 30 ms, holding a mutex", SYS(SYS_psynch_cvwait, c1, PAIR(0, SEQ(1)), 0, m4, PAIR(SEQ(1), SEQ(2)), FIRSTFIT, 0, 30000000));
    hex("the mutex's next", SYS(SYS_psynch_mutexwait, m4, SEQ(3), SEQ(1), 0, FIRSTFIT));
    // An address is one kind of thing while anything of it is outstanding.
    hex("drop a mutex", SYS(SYS_psynch_mutexdrop, m4, SEQ(4), SEQ(2), 0, FIRSTFIT));
    t_show("signal it as a condition", SYS(SYS_psynch_cvsignal, m4, PAIR(0, SEQ(1)), 0, 0, 0, 0, 0, PRIVATE));
    hex("and wait for it as the mutex it is", SYS(SYS_psynch_mutexwait, m4, SEQ(5), SEQ(2), 0, FIRSTFIT));
}

static u64 r1[8], r2[8], r3[8];

static void rwlocks(void) {
    t_printf("a read-write lock\n");
    // (rwlock, L, U, S, flags.) An unlock for one waiter who is not here
    // yet; the writer it was for arrives, and has the lock.
    hex("unlock, the waiter not here yet", SYS(SYS_psynch_rw_unlock, r1, SEQ(2) | 5, SEQ(1), SEQ(1), PRIVATE));
    hex("the writer", SYS(SYS_psynch_rw_wrlock, r1, SEQ(2) | 5, SEQ(1), SEQ(1), PRIVATE));
    // A writer waits for the readers' unlock.
    static struct waiter w = {SYS_psynch_rw_wrlock, {(long)r2, SEQ(3) | 5, 0, 0, PRIVATE}};
    start(&w);
    hex("unlock, a writer waiting", SYS(SYS_psynch_rw_unlock, r2, SEQ(3) | 5, SEQ(2), SEQ(2), PRIVATE));
    show("the writer", &w);
    // Two readers wait behind a writer; its unlock lets both in, and a
    // third reader, arriving as they go in, goes with them.
    static struct waiter a = {SYS_psynch_rw_rdlock, {(long)r3, SEQ(2) | 3, 0, 0, PRIVATE}}, b = {SYS_psynch_rw_rdlock, {(long)r3, SEQ(3) | 3, 0, 0, PRIVATE}};
    start(&a);
    start(&b);
    hex("unlock, two readers waiting", SYS(SYS_psynch_rw_unlock, r3, SEQ(3) | 3, SEQ(1), SEQ(1), PRIVATE));
    show("the first reader", &a);
    show("the second", &b);
    hex("a reader arriving after", SYS(SYS_psynch_rw_rdlock, r3, SEQ(4) | 1, SEQ(1), SEQ(3), PRIVATE));
    // An unlock below the last is spurious.
    hex("an unlock from before", SYS(SYS_psynch_rw_unlock, r3, SEQ(3), 0, SEQ(1), PRIVATE));
    t_show("the long read lock", SYS(SYS_psynch_rw_longrdlock, r1, SEQ(2), SEQ(1), SEQ(1), PRIVATE));
    t_show("the yielding write lock", SYS(SYS_psynch_rw_yieldwrlock, r1, SEQ(2), SEQ(1), SEQ(1), PRIVATE));
    t_show("upgrade", SYS(SYS_psynch_rw_upgrade, r1, SEQ(2), SEQ(1), SEQ(1), PRIVATE));
    t_show("downgrade", SYS(SYS_psynch_rw_downgrade, r1, SEQ(2), SEQ(1), SEQ(1), PRIVATE));
    t_show("the other unlock", SYS(SYS_psynch_rw_unlock2, r1, SEQ(2), SEQ(1), SEQ(1), PRIVATE));
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    mutexes();
    conditions();
    rwlocks();
    return 0;
}
