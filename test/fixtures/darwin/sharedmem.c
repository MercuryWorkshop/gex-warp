// What a forked child shares with its parent: anonymous MAP_SHARED memory,
// POSIX shared memory (shm_open), a file mapped MAP_SHARED, a memfd (Linux),
// and private memory for contrast; seen after the other's exit, at a
// system call (a pipe), and with no call at all (a spin); then atomics, a
// process-shared mutex and an unnamed semaphore in shared memory.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <semaphore.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#ifdef __linux__
#include <sys/syscall.h>
#endif

#define LEN 65536

static void put(int fd) { char c = 'x'; if (write(fd, &c, 1) != 1) _exit(90); }
static void get(int fd) { char c; if (read(fd, &c, 1) != 1) _exit(91); }

// One region: the child sees the parent's store from before the fork, the
// parent sees the child's at a pipe while it runs, the child the parent's
// after, then a page further in, and the child's last store after its exit.
static void across(const char *what, volatile unsigned *p) {
    int a[2], b[2];
    if (pipe(a) || pipe(b)) { printf("%s: pipe %d\n", what, errno); return; }
    p[0] = 1, p[4096] = 0;
    pid_t c = fork();
    if (c == 0) {
        alarm(10);
        unsigned saw = p[0];
        p[0] = 2;
        put(a[1]);
        get(b[0]);
        unsigned saw2 = p[0], saw3 = p[4096];
        p[1] = 7;
        _exit(saw | saw2 << 4 | saw3 << 8);
    }
    get(a[0]);
    unsigned mid = p[0];
    p[0] = 3, p[4096] = 5;
    put(b[1]);
    int st;
    waitpid(c, &st, 0);
    int e = WIFEXITED(st) ? WEXITSTATUS(st) : -1;
    printf("%s: child saw %d at fork, then %d and %d; parent saw %u at the pipe, %u after the exit\n", what, e < 0 ? -1 : e & 15, e < 0 ? -1 : e >> 4 & 15,
           e < 0 ? -1 : e >> 8 & 15, mid, p[1]);
    close(a[0]), close(a[1]), close(b[0]), close(b[1]);
}

// With no system call between: the child spins until the parent's store.
static void spin(const char *what, volatile unsigned *p) {
    p[2] = 0, p[3] = 0;
    pid_t c = fork();
    if (c == 0) {
        alarm(5);
        p[3] = 1;
        while (p[2] == 0) {}
        _exit(0);
    }
    // (The child's start is waited for with a poll of the memory, and a
    // sleep between, so a store made at a call boundary is seen too.)
    for (int i = 0; i < 500 && p[3] == 0; i++) usleep(10000);
    unsigned started = p[3];
    p[2] = 1;
    int st;
    waitpid(c, &st, 0);
    printf("%s: the parent saw the child start %s; the child saw the parent's store with no call %s\n", what, started ? "yes" : "no",
           WIFEXITED(st) ? "yes" : WIFSIGNALED(st) && WTERMSIG(st) == SIGALRM ? "no (it spun 5 s)" : "?");
}

static void atomics(const char *what, unsigned *p) {
    p[8] = 0;
    pid_t c = fork();
    if (c == 0) {
        alarm(20);
        for (int i = 0; i < 100000; i++) __atomic_fetch_add(&p[8], 1, __ATOMIC_SEQ_CST);
        _exit(0);
    }
    for (int i = 0; i < 100000; i++) __atomic_fetch_add(&p[8], 1, __ATOMIC_SEQ_CST);
    waitpid(c, 0, 0);
    printf("%s: atomic adds of both, 200000 wanted: %u\n", what, p[8]);
}

struct locked {
    pthread_mutex_t m;
    unsigned n;
    sem_t s;
    unsigned posted;
};

static void mutex(void) {
    struct locked *l = mmap(0, sizeof *l, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANON, -1, 0);
    pthread_mutexattr_t a;
    pthread_mutexattr_init(&a);
    int r = pthread_mutexattr_setpshared(&a, PTHREAD_PROCESS_SHARED);
    printf("mutex: setpshared %d, init %d\n", r, pthread_mutex_init(&l->m, &a));
    pthread_mutex_lock(&l->m);
    int p[2];
    if (pipe(p)) return;
    pid_t c = fork();
    if (c == 0) {
        alarm(20);
        int busy = pthread_mutex_trylock(&l->m);
        put(p[1]);
        for (int i = 0; i < 20000; i++) {
            pthread_mutex_lock(&l->m);
            unsigned v = l->n;
            l->n = v + 1;
            pthread_mutex_unlock(&l->m);
        }
        _exit(busy == EBUSY ? 0 : 1);
    }
    get(p[0]);
    pthread_mutex_unlock(&l->m);
    for (int i = 0; i < 20000; i++) {
        pthread_mutex_lock(&l->m);
        unsigned v = l->n;
        l->n = v + 1;
        pthread_mutex_unlock(&l->m);
    }
    int st;
    waitpid(c, &st, 0);
    printf("mutex: the child found it held %s; counted under it, 40000 wanted: %u%s\n", WIFEXITED(st) && WEXITSTATUS(st) == 0 ? "yes" : "no", l->n,
           WIFSIGNALED(st) ? " (the child timed out)" : "");
    errno = 0;
    r = sem_init(&l->s, 1, 0);
    printf("semaphore: sem_init shared %d (errno %d)\n", r, r ? errno : 0);
    if (r) return;
    c = fork();
    if (c == 0) {
        alarm(20);
        for (int i = 0; i < 1000; i++) sem_wait(&l->s);
        _exit(0);
    }
    for (int i = 0; i < 1000; i++) sem_post(&l->s);
    waitpid(c, &st, 0);
    printf("semaphore: the child took all 1000 posts %s\n", WIFEXITED(st) ? "yes" : "no (it timed out)");
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    unsigned *anon = mmap(0, LEN, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANON, -1, 0);
    across("anonymous shared", anon);
    spin("anonymous shared", anon);
    atomics("anonymous shared", anon);

    unsigned *priv = mmap(0, LEN, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    across("anonymous private", priv);

    char name[64];
    snprintf(name, sizeof name, "/gexshm%d", (int)getpid());
    int fd = shm_open(name, O_CREAT | O_RDWR | O_EXCL, 0600);
    if (fd < 0 || ftruncate(fd, LEN)) printf("shm_open: %d\n", errno);
    else {
        unsigned *s = mmap(0, LEN, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        across("shm_open", s);
        spin("shm_open", s);
        atomics("shm_open", s);
        // Mapped again by name in a child, after the fork.
        s[5] = 0;
        pid_t c = fork();
        if (c == 0) {
            int f = shm_open(name, O_RDWR, 0);
            unsigned *t = mmap(0, LEN, PROT_READ | PROT_WRITE, MAP_SHARED, f, 0);
            t[5] = 42;
            _exit(0);
        }
        waitpid(c, 0, 0);
        printf("shm_open: a child's own mapping of the name, seen after its exit: %u\n", s[5]);
        shm_unlink(name);
        close(fd);
    }

    char path[] = "/tmp/gexshmfileXXXXXX";
    fd = mkstemp(path);
    if (fd < 0 || ftruncate(fd, LEN)) printf("file: %d\n", errno);
    else {
        unsigned *f = mmap(0, LEN, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        across("file", f);
        spin("file", f);
        atomics("file", f);
        unsigned v = 0;
        pread(fd, &v, 4, 4);
        printf("file: the child's store read from the file: %u\n", v);
        unlink(path);
        close(fd);
    }
#ifdef __linux__
    fd = syscall(SYS_memfd_create, "gex", 0);
    if (fd < 0 || ftruncate(fd, LEN)) printf("memfd: %d\n", errno);
    else {
        unsigned *m = mmap(0, LEN, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        across("memfd", m);
        spin("memfd", m);
        atomics("memfd", m);
    }
#endif
    mutex();
    return 0;
}
