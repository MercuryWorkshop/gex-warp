// Code made at one address by processes a fork parted: each runs its own.
// A loader puts a library where the address space has room, the same place
// in a parent and its child, so the two can have different code at one
// address; a JIT the same. Every order of that: the child first, the parent
// first, two children at once taking turns, a child forked after its
// sibling made its code, a process that unmaps a function and maps another
// in its place while its relative still calls the old one, and processes of
// more than one thread on either side.
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#define ROOM 0x4000

static unsigned char *room;

// A function that returns `n`, written to `room` (mapped there afresh) and
// made executable: the machine's move of a constant to the result register,
// and return. Built both with and without the flush of the instruction
// cache (NO_FLUSH): a page made executable afresh needs none from the
// program on Linux, and an AArch64 `ic ivau` must not be what tells two
// processes' code apart.
static int make(int n) {
    unsigned char *at = mmap(room, ROOM, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
    if (at != room) return -1;
#if defined(__aarch64__)
    unsigned code[2] = {0x52800000u | (unsigned)n << 5, 0xd65f03c0u};
#else
    unsigned char code[6] = {0xb8, (unsigned char)n, 0, 0, 0, 0xc3};
#endif
    memcpy(at, code, sizeof code);
    if (mprotect(at, ROOM, PROT_READ | PROT_EXEC)) return -2;
#ifndef NO_FLUSH
    __builtin___clear_cache((char *)at, (char *)at + sizeof code);
#endif
    return 0;
}

static int call(void) {
    return ((int (*)(void))room)();
}

static int code_at(int n) {
    int r = make(n);
    return r ? r : call();
}

// A new place there is room at, found and let go before anything forks.
static void new_room(void) {
    room = mmap(0, ROOM, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    munmap(room, ROOM);
}

static void say(int fd) {
    char c = 0;
    if (write(fd, &c, 1) != 1) _exit(90);
}

static void hear(int fd) {
    char c;
    if (read(fd, &c, 1) != 1) _exit(91);
}

static void reap(int n) {
    for (int i = 0; i < n; i++) {
        int st;
        if (wait(&st) < 0 || !WIFEXITED(st) || WEXITSTATUS(st)) printf("a child failed: %x\n", st);
    }
}

// Two channels, one each way.
struct link {
    int to[2], from[2];
};

static void open_link(struct link *l) {
    if (pipe(l->to) || pipe(l->from)) _exit(92);
}

static void *thread_waits(void *arg) {
    hear(*(int *)arg);
    printf("its thread, after: %d\n", call());
    return 0;
}

static int child_thread_n;

static void *thread_makes(void *arg) {
    (void)arg;
    printf("the child's thread's code: %d\n", code_at(child_thread_n));
    return 0;
}

int main(void) {
    setvbuf(stdout, 0, _IONBF, 0);

    // The child first, then its parent.
    new_room();
    if (fork() == 0) {
        printf("child first: %d\n", code_at(11));
        _exit(0);
    }
    reap(1);
    printf("then its parent: %d\n", code_at(22));

    // The parent first, then a child forked before it.
    new_room();
    struct link l;
    open_link(&l);
    if (fork() == 0) {
        hear(l.to[0]);
        printf("then a child forked before: %d\n", code_at(44));
        say(l.from[1]);
        _exit(0);
    }
    printf("parent first: %d\n", code_at(33));
    say(l.to[1]);
    hear(l.from[0]);
    printf("the parent again: %d\n", call());
    reap(1);

    // Two children at once, taking turns.
    new_room();
    struct link a, b;
    open_link(&a);
    open_link(&b);
    if (fork() == 0) {
        hear(a.to[0]);
        printf("one child: %d\n", code_at(55));
        say(a.from[1]);
        hear(a.to[0]);
        printf("one child again: %d\n", call());
        say(a.from[1]);
        _exit(0);
    }
    if (fork() == 0) {
        hear(b.to[0]);
        printf("the other: %d\n", code_at(66));
        say(b.from[1]);
        hear(b.to[0]);
        printf("the other again: %d\n", call());
        say(b.from[1]);
        _exit(0);
    }
    for (int i = 0; i < 2; i++) {
        say(a.to[1]);
        hear(a.from[0]);
        say(b.to[1]);
        hear(b.from[0]);
    }
    reap(2);

    // A child forked after its sibling made code there, while that sibling
    // still runs.
    new_room();
    open_link(&a);
    if (fork() == 0) {
        printf("an older child: %d\n", code_at(77));
        say(a.from[1]);
        hear(a.to[0]);
        printf("the older child again: %d\n", call());
        _exit(0);
    }
    hear(a.from[0]);
    if (fork() == 0) {
        printf("a younger child: %d\n", code_at(88));
        _exit(0);
    }
    reap(1);
    say(a.to[1]);
    reap(1);

    // A function unmapped and another mapped in its place, while the
    // process's relative still calls the first: the parent changes it, then
    // the child does.
    new_room();
    open_link(&l);
    printf("before the fork: %d\n", code_at(99));
    if (fork() == 0) {
        printf("the child calls it: %d\n", call());
        say(l.from[1]);
        hear(l.to[0]);
        printf("the child's is the old one still: %d\n", call());
        munmap(room, ROOM);
        printf("the child's new one: %d\n", code_at(111));
        say(l.from[1]);
        _exit(0);
    }
    hear(l.from[0]);
    munmap(room, ROOM);
    printf("the parent's new one: %d\n", code_at(100));
    say(l.to[1]);
    hear(l.from[0]);
    printf("the parent's is its own still: %d\n", call());
    reap(1);

    // A parent with a second thread running forks, and the child and the
    // parent each make code there.
    new_room();
    open_link(&l);
    pthread_t th;
    pthread_create(&th, 0, thread_waits, &l.to[0]);
    if (fork() == 0) {
        printf("the child of two threads: %d\n", code_at(122));
        _exit(0);
    }
    reap(1);
    printf("the parent of two threads: %d\n", code_at(133));
    say(l.to[1]);
    pthread_join(th, 0);

    // A child that starts a thread, which makes code there; then its
    // parent.
    new_room();
    open_link(&l);
    if (fork() == 0) {
        child_thread_n = 144;
        pthread_create(&th, 0, thread_makes, 0);
        pthread_join(th, 0);
        say(l.from[1]);
        hear(l.to[0]);
        printf("the child's, after: %d\n", call());
        _exit(0);
    }
    hear(l.from[0]);
    printf("its parent's: %d\n", code_at(155));
    say(l.to[1]);
    reap(1);
    return 0;
}
