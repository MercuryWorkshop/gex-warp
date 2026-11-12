// Process-shared locks between processes, as a Mac has them: a
// PTHREAD_PROCESS_SHARED mutex, condition variable and read-write lock in
// MAP_SHARED memory used across a fork, and a named semaphore (sem_open)
// between a parent, its forked child and a process it spawns that opens
// the name itself. Each line says what happened; the same lines on a Mac
// and under gex.
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <semaphore.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;

struct shared {
    pthread_mutex_t m;
    pthread_cond_t c;
    pthread_rwlock_t rw;
    unsigned n;
    unsigned ready;
};

static int status_of(pid_t c) {
    int st;
    if (waitpid(c, &st, 0) != c) return -1;
    return WIFEXITED(st) ? WEXITSTATUS(st) : 128 + WTERMSIG(st);
}

static void mutex(struct shared *s) {
    pthread_mutexattr_t a;
    pthread_mutexattr_init(&a);
    pthread_mutexattr_setpshared(&a, PTHREAD_PROCESS_SHARED);
    pthread_mutex_init(&s->m, &a);
    s->n = 0;
    pid_t c = fork();
    if (c == 0) {
        alarm(30);
        for (int i = 0; i < 20000; i++) {
            pthread_mutex_lock(&s->m);
            unsigned v = s->n;
            if (i % 1000 == 0) usleep(100);
            s->n = v + 1;
            pthread_mutex_unlock(&s->m);
        }
        _exit(0);
    }
    for (int i = 0; i < 20000; i++) {
        pthread_mutex_lock(&s->m);
        unsigned v = s->n;
        if (i % 1000 == 0) usleep(100);
        s->n = v + 1;
        pthread_mutex_unlock(&s->m);
    }
    int st = status_of(c);
    printf("mutex: contended from two processes, 40000 wanted: %u (child %d)\n", s->n, st);
}

static void cond(struct shared *s) {
    pthread_condattr_t ca;
    pthread_condattr_init(&ca);
    pthread_condattr_setpshared(&ca, PTHREAD_PROCESS_SHARED);
    pthread_cond_init(&s->c, &ca);
    s->ready = 0;
    pid_t c = fork();
    if (c == 0) {
        alarm(30);
        pthread_mutex_lock(&s->m);
        while (!s->ready) pthread_cond_wait(&s->c, &s->m);
        pthread_mutex_unlock(&s->m);
        _exit(7);
    }
    usleep(200000);
    pthread_mutex_lock(&s->m);
    s->ready = 1;
    pthread_cond_broadcast(&s->c);
    pthread_mutex_unlock(&s->m);
    printf("condition: the child woken from another process's broadcast: %s\n", status_of(c) == 7 ? "yes" : "no");
}

static void rwlock(struct shared *s) {
    pthread_rwlockattr_t ra;
    pthread_rwlockattr_init(&ra);
    pthread_rwlockattr_setpshared(&ra, PTHREAD_PROCESS_SHARED);
    pthread_rwlock_init(&s->rw, &ra);
    pthread_rwlock_wrlock(&s->rw);
    int p[2];
    if (pipe(p)) return;
    pid_t c = fork();
    if (c == 0) {
        alarm(30);
        int busy = pthread_rwlock_tryrdlock(&s->rw);
        char b = busy == EBUSY ? 'b' : 'f';
        write(p[1], &b, 1);
        pthread_rwlock_rdlock(&s->rw);
        pthread_rwlock_unlock(&s->rw);
        _exit(busy == EBUSY ? 0 : 1);
    }
    char b = 0;
    read(p[0], &b, 1);
    usleep(100000);
    pthread_rwlock_unlock(&s->rw);
    printf("read-write lock: held for writing in one process, busy in the other: %s; read once let go: %s\n", b == 'b' ? "yes" : "no",
           status_of(c) == 0 ? "yes" : "no");
}

static void semaphore(const char *self) {
    char name[32];
    snprintf(name, sizeof name, "/gexsem%d", (int)getpid());
    sem_t *sem = sem_open(name, O_CREAT | O_EXCL, 0600, 0);
    printf("semaphore: sem_open %s\n", sem == SEM_FAILED ? strerror(errno) : "ok");
    if (sem == SEM_FAILED) return;
    sem_t *again = sem_open(name, O_CREAT | O_EXCL, 0600, 0);
    printf("semaphore: again with O_EXCL: %s\n", again == SEM_FAILED ? strerror(errno) : "opened");
    printf("semaphore: trywait on none: %s\n", sem_trywait(sem) == 0 ? "taken" : strerror(errno));
    pid_t c = fork();
    if (c == 0) {
        alarm(30);
        for (int i = 0; i < 1000; i++) sem_wait(sem);
        _exit(0);
    }
    for (int i = 0; i < 1000; i++) sem_post(sem);
    printf("semaphore: the forked child took all 1000 posts: %s\n", status_of(c) == 0 ? "yes" : "no");
    char *argv[] = {(char *)self, "post", name, 0};
    pid_t s;
    int r = posix_spawn(&s, self, 0, 0, argv, environ);
    if (r) {
        printf("semaphore: posix_spawn %s\n", strerror(r));
        return;
    }
    alarm(30);
    int w = sem_wait(sem);
    alarm(0);
    printf("semaphore: a post from a process that opened the name itself: %s (it exited %d)\n", w == 0 ? "taken" : strerror(errno), status_of(s));
    printf("semaphore: sem_close %d, sem_unlink %d, sem_unlink again: %s\n", sem_close(sem), sem_unlink(name), sem_unlink(name) == 0 ? "ok" : strerror(errno));
    printf("semaphore: sem_open of the unlinked name without O_CREAT: %s\n", sem_open(name, 0) == SEM_FAILED ? strerror(errno) : "opened");
}

int main(int argc, char **argv) {
    if (argc == 3 && strcmp(argv[1], "post") == 0) {
        sem_t *sem = sem_open(argv[2], 0);
        if (sem == SEM_FAILED) return 2;
        return sem_post(sem) == 0 ? 0 : 3;
    }
    setvbuf(stdout, 0, _IONBF, 0);
    struct shared *s = mmap(0, sizeof *s, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANON, -1, 0);
    mutex(s);
    cond(s);
    rwlock(s);
    semaphore(argv[0]);
    return 0;
}
