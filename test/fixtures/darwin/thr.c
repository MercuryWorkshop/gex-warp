// Threads: made and ended as libpthread asks the kernel, waiting on words
// of memory with ulock_wait and ulock_wake, and a signal for one of them.
#include "t.h"

static u32 lock, counter;
static u64 ids[8];
static volatile u32 gate, started, woken;
static volatile u64 handled_by;

static void *count(void *arg) {
    long n = (long)arg;
    ids[n] = (u64)SYS(SYS_thread_selfid);
    for (int i = 0; i < 10000; i++) {
        t_lock(&lock);
        counter++;
        t_unlock(&lock);
    }
    return 0;
}

static void *sleeper(void *arg) {
    (void)arg;
    __atomic_add_fetch(&started, 1, __ATOMIC_RELEASE);
    // Until the gate opens: a wake with the word unchanged is waited again.
    while (!__atomic_load_n(&gate, __ATOMIC_ACQUIRE)) SYS(SYS_ulock_wait, UL_COMPARE_AND_WAIT, &gate, 0, 0);
    __atomic_add_fetch(&woken, 1, __ATOMIC_RELEASE);
    return 0;
}

static void on_usr1(int sig) {
    (void)sig;
    handled_by = (u64)SYS(SYS_thread_selfid);
}

static volatile u64 target_id;
static volatile u32 target_go;
static void *target(void *arg) {
    (void)arg;
    target_id = (u64)SYS(SYS_thread_selfid);
    while (!__atomic_load_n(&target_go, __ATOMIC_ACQUIRE)) SYS(SYS_ulock_wait, UL_COMPARE_AND_WAIT, &target_go, 0, 100000);
    return 0;
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    u64 me = (u64)SYS(SYS_thread_selfid);
    int h[4];
    for (long i = 0; i < 4; i++) h[i] = t_thread_start(count, (void *)i);
    t_check("four threads started", h[0] >= 0 && h[1] >= 0 && h[2] >= 0 && h[3] >= 0);
    for (int i = 0; i < 4; i++) t_thread_join(h[i]);
    t_printf("counted %u under the lock\n", counter);
    int distinct = 1;
    for (int i = 0; i < 4; i++) {
        distinct &= ids[i] != 0 && ids[i] != me;
        for (int j = 0; j < i; j++) distinct &= ids[i] != ids[j];
    }
    t_check("each has an id of its own", distinct);

    // ulock: a word that has already changed, a timeout, nobody to wake.
    u32 word = 5;
    t_show("ulock_wait on a changed word", SYS(SYS_ulock_wait, UL_COMPARE_AND_WAIT, &word, 4, 0));
    t_show("ulock_wait for 20 ms", SYS(SYS_ulock_wait, UL_COMPARE_AND_WAIT, &word, 5, 20000));
    t_show("the same with ULF_NO_ERRNO", SYS(SYS_ulock_wait, UL_COMPARE_AND_WAIT | ULF_NO_ERRNO, &word, 5, 20000));
    // (a word nothing has waited on: gex knows nobody is there only then)
    u32 quiet = 0;
    t_show("ulock_wake with nobody waiting", SYS(SYS_ulock_wake, UL_COMPARE_AND_WAIT, &quiet, 0));
    t_show("the same with ULF_NO_ERRNO", SYS(SYS_ulock_wake, UL_COMPARE_AND_WAIT | ULF_NO_ERRNO, &quiet, 0));
    t_show("ulock_wait of no kind", SYS(SYS_ulock_wait, 99, &word, 5, 1000));

    // Three sleepers on one word, woken together.
    int s[3];
    for (int i = 0; i < 3; i++) s[i] = t_thread_start(sleeper, 0);
    while (__atomic_load_n(&started, __ATOMIC_ACQUIRE) != 3) TRAP(TRAP_swtch_pri, 0);
    __atomic_store_n(&gate, 1, __ATOMIC_RELEASE);
    SYS(SYS_ulock_wake, UL_COMPARE_AND_WAIT | ULF_WAKE_ALL, &gate, 0);
    for (int i = 0; i < 3; i++) t_thread_join(s[i]);
    t_printf("woken %u\n", woken);

    // A signal for one thread is handled by that thread.
    t_sigaction(SIGUSR1, on_usr1, 0, 0, 0);
    int th = t_thread_start(target, 0);
    while (!target_id) TRAP(TRAP_swtch_pri, 0);
    t_show("__pthread_kill", SYS(SYS___pthread_kill, t_thread_port(th), SIGUSR1));
    while (!handled_by) TRAP(TRAP_swtch_pri, 0);
    t_check("handled by the thread it was for", handled_by == target_id);
    __atomic_store_n(&target_go, 1, __ATOMIC_RELEASE);
    SYS(SYS_ulock_wake, UL_COMPARE_AND_WAIT, &target_go, 0);
    t_thread_join(th);
    t_show("__pthread_kill of no thread", SYS(SYS___pthread_kill, 0x12345, SIGUSR1));
    return 0;
}
