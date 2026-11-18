// ulock: the kernel's half of os_unfair_lock and of libdispatch's waits, as
// libplatform and libdispatch ask it (ulock_wait, ulock_wake).
//
// - a wait on a word that no longer holds the value returns at once, and a
//   wake nobody waits for says so (ENOENT);
// - an unfair lock's word names its owner (the thread's port name; the low
//   bit set while nobody waits): a wait for a lock whose owner has ended
//   (EOWNERDEAD, at once: XNU looks the owner up), one whose word names no
//   thread at all, and one whose owner is the caller (EDEADLK);
// - two threads taking and giving back one unfair lock by turns, as
//   os_unfair_lock does (the owner's name in the word, the waiter clears
//   the low bit, the unlocker wakes when it is clear), many times: every
//   wait ends with the lock given back, none times out (a lost wake).
//
// Each wait that could hang has a timeout, so that a kernel that sleeps
// where the Mac answers says ETIMEDOUT instead of hanging.
#include "t.h"

enum { ENOENT = 2, EDEADLK = 11, EINTR = 4, ETIMEDOUT = 60, EOWNERDEAD = 105 };

static long ulock_wait(u32 op, u32 *word, u32 value, u32 timeout_us) {
    return SYS(SYS_ulock_wait, op, word, value, timeout_us);
}

static long ulock_wake(u32 op, u32 *word) {
    return SYS(SYS_ulock_wake, op, word, 0);
}

// What a call answered: 0, or its error by name (or number).
static void say(const char *what, long r) {
    const char *name = 0;
    switch (r) {
    case 0: name = "0"; break;
    case -ENOENT: name = "ENOENT"; break;
    case -EDEADLK: name = "EDEADLK"; break;
    case -ETIMEDOUT: name = "ETIMEDOUT"; break;
    case -EOWNERDEAD: name = "EOWNERDEAD"; break;
    case -EINTR: name = "EINTR"; break;
    }
    if (name) t_printf("%s: %s\n", what, name);
    else t_printf("%s: %ld\n", what, r);
}

// The lock, as os_unfair_lock keeps it: 0 free, else the owner's thread
// port name, its low bit cleared once somebody waits.
static u32 lock_word;
static u32 lock_waits_timed_out;

static void lock(u32 me) {
    for (;;) {
        u32 cur = 0;
        if (__atomic_compare_exchange_n(&lock_word, &cur, me, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) return;
        // (Say that somebody waits: the low bit cleared.)
        if (cur & 1) {
            u32 want = cur & ~1u;
            if (!__atomic_compare_exchange_n(&lock_word, &cur, want, 0, __ATOMIC_RELAXED, __ATOMIC_RELAXED)) continue;
            cur = want;
        }
        // (Taken with the low bit already clear: more may wait.)
        long r = ulock_wait(UL_UNFAIR_LOCK, &lock_word, cur, 2000000);
        if (r == -ETIMEDOUT) __atomic_fetch_add(&lock_waits_timed_out, 1, __ATOMIC_RELAXED);
        cur = 0;
        if (__atomic_compare_exchange_n(&lock_word, &cur, me & ~1u, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) return;
    }
}

static void unlock(void) {
    u32 was = __atomic_exchange_n(&lock_word, 0, __ATOMIC_RELEASE);
    if (!(was & 1)) ulock_wake(UL_UNFAIR_LOCK, &lock_word);
}

enum { TURNS = 20000 };
static u32 counted;

static void *turns(void *arg) {
    (void)arg;
    u32 me = (u32)TRAP(TRAP_thread_self) | 1;
    for (int i = 0; i < TURNS; i++) {
        lock(me);
        u32 c = counted;
        // (Long enough held for the other to wait now and then.)
        for (volatile int k = 0; k < 20; k++) {}
        counted = c + 1;
        unlock();
    }
    return 0;
}

static u32 ended_owner;
static void *own_and_end(void *arg) {
    (void)arg;
    ended_owner = (u32)TRAP(TRAP_thread_self);
    return 0;
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    // A word that no longer holds the value, and a wake for nobody.
    static u32 w;
    __atomic_store_n(&w, 5, __ATOMIC_RELAXED);
    say("wake, a word never waited on", ulock_wake(UL_COMPARE_AND_WAIT, &w));
    say("wait, the word changed", ulock_wait(UL_COMPARE_AND_WAIT, &w, 4, 1000000));
    say("wait, the word the same, 50 ms", ulock_wait(UL_COMPARE_AND_WAIT, &w, 5, 50000));
    say("wake, nobody waits", ulock_wake(UL_COMPARE_AND_WAIT, &w));
    say("wake all, nobody waits", ulock_wake(UL_COMPARE_AND_WAIT | ULF_WAKE_ALL, &w));
    // (A wait that timed out leaves nobody waiting: a wake after it wakes
    // nobody, and a waiter after that is the one woken.)
    say("wake again, nobody waits", ulock_wake(UL_COMPARE_AND_WAIT, &w));

    // An unfair lock whose owner has ended: the thread took its own
    // port's name (as os_unfair_lock's owner) and ended.
    int h = t_thread_start(own_and_end, 0);
    if (h < 0) return 1;
    t_thread_join(h);
    // (It is ending: until its port says so.)
    long r = 0;
    for (int i = 0; i < 100; i++) {
        static u32 dead;
        dead = ended_owner & ~1u;
        r = ulock_wait(UL_UNFAIR_LOCK, &dead, dead, 300000);
        if (r == -EOWNERDEAD) break;
        SYS(SYS_ulock_wait, UL_COMPARE_AND_WAIT, &w, 5, 10000);
    }
    say("unfair lock, its owner ended", r);
    // One whose word names no thread (a name nothing has).
    static u32 nobody = 0x7ff0002;
    say("unfair lock, its owner no thread", ulock_wait(UL_UNFAIR_LOCK, &nobody, nobody, 300000));
    // One the caller holds itself.
    static u32 mine;
    mine = (u32)TRAP(TRAP_thread_self) & ~1u;
    say("unfair lock, held by the caller", ulock_wait(UL_UNFAIR_LOCK, &mine, mine, 300000));

    // Two threads by turns.
    int a = t_thread_start(turns, 0), b = t_thread_start(turns, 0);
    if (a < 0 || b < 0) return 1;
    t_thread_join(a);
    t_thread_join(b);
    t_printf("by turns: %u of %u, %s\n", counted, 2 * TURNS, lock_waits_timed_out ? "some waits timed out" : "no wait timed out");
    return 0;
}
