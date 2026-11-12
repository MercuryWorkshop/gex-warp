// One thread waits in epoll_wait on a set that has only an eventfd it never
// writes; another, once the waiter is parked, adds a pipe's read end to the
// set and writes to the pipe. The waiter must wake for the pipe, as on Linux
// (async-io's reactor waits so while the threads it serves add their
// descriptors). Prints "woken by the pipe" or, after its own 5 s, "timed out".
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <time.h>
#include <unistd.h>

static int ep, fds[2];

static void *adder(void *arg) {
    (void)arg;
    struct timespec t = { 0, 300 * 1000 * 1000 };
    nanosleep(&t, 0);
    struct epoll_event e = { .events = EPOLLIN, .data.u64 = 42 };
    if (epoll_ctl(ep, EPOLL_CTL_ADD, fds[0], &e) != 0) perror("epoll_ctl");
    nanosleep(&t, 0);
    if (write(fds[1], "x", 1) != 1) perror("write");
    return 0;
}

int main(void) {
    ep = epoll_create1(0);
    int never = eventfd(0, 0);
    struct epoll_event e = { .events = EPOLLIN, .data.u64 = 7 };
    epoll_ctl(ep, EPOLL_CTL_ADD, never, &e);
    if (pipe(fds) != 0) return 1;
    pthread_t t;
    pthread_create(&t, 0, adder, 0);
    struct epoll_event out[4];
    int n = epoll_wait(ep, out, 4, 5000);
    if (n == 1 && out[0].data.u64 == 42) printf("woken by the pipe\n");
    else if (n == 0) printf("timed out\n");
    else printf("woken: %d events, first %llu\n", n, n > 0 ? (unsigned long long)out[0].data.u64 : 0ULL);
    pthread_join(t, 0);
    return 0;
}
