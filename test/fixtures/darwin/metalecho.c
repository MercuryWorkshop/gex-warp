// /dev/gex-metal as a channel, with a host program that answers what it is
// sent (cat): each open is a stream to a child of its own; a read waits
// while there is nothing and another thread can write meanwhile; a
// non-blocking descriptor is told EAGAIN; poll says when there is something;
// a megabyte goes through and comes back whole.
//
// usage: metalecho          the device is there
//        metalecho absent   the device is not there
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static const char *node = "/dev/gex-metal";

static int read_exactly(int fd, char *p, size_t n) {
    while (n) {
        ssize_t r = read(fd, p, n);
        if (r < 0 && errno == EINTR) continue;
        if (r <= 0) return -1;
        p += r;
        n -= (size_t)r;
    }
    return 0;
}

static int write_all(int fd, const char *p, size_t n) {
    while (n) {
        ssize_t w = write(fd, p, n);
        if (w < 0 && errno == EINTR) continue;
        if (w <= 0) return -1;
        p += w;
        n -= (size_t)w;
    }
    return 0;
}

struct waiter {
    int fd;
    char got[8];
    int result;
};

static void *wait_for_four(void *arg) {
    struct waiter *w = arg;
    w->result = read_exactly(w->fd, w->got, 4);
    return NULL;
}

enum { BIG = 1 << 20 };

struct drain {
    int fd;
    char *into;
    int result;
};

static void *drain_big(void *arg) {
    struct drain *d = arg;
    d->result = read_exactly(d->fd, d->into, BIG);
    return NULL;
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "absent") == 0) {
        int fd = open(node, O_RDWR);
        printf("open without a host: %s\n", fd < 0 && errno == ENOENT ? "ENOENT" : "unexpected");
        return 0;
    }
    struct stat st;
    printf("stat: %s\n", stat(node, &st) == 0 && S_ISCHR(st.st_mode) ? "a character device" : "unexpected");
    int fd = open(node, O_RDWR);
    if (fd < 0) {
        printf("open: %s\n", strerror(errno));
        return 1;
    }
    // What is written comes back.
    char buf[16] = {0};
    printf("write: %zd\n", write(fd, "ping", 4));
    printf("read: %s\n", read_exactly(fd, buf, 4) == 0 ? buf : "failed");

    // A read that waits, while this thread writes what it is waiting for.
    struct waiter w = {fd, {0}, -1};
    pthread_t t;
    pthread_create(&t, NULL, wait_for_four, &w);
    usleep(200000);
    printf("write while another thread reads: %zd\n", write(fd, "pong", 4));
    pthread_join(t, NULL);
    printf("the waiting read: %s\n", w.result == 0 ? w.got : "failed");

    // poll: nothing, then something.
    struct pollfd p = {fd, POLLIN, 0};
    printf("poll with nothing to read: %d\n", poll(&p, 1, 50));
    write(fd, "x", 1);
    p.revents = 0;
    int ready = poll(&p, 1, 5000);
    printf("poll after a write: %d%s\n", ready, p.revents & POLLIN ? " POLLIN" : "");
    read_exactly(fd, buf, 1);

    // Non-blocking: EAGAIN rather than a wait.
    fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK);
    ssize_t r = read(fd, buf, 4);
    printf("non-blocking read with nothing: %s\n", r < 0 && errno == EAGAIN ? "EAGAIN" : "unexpected");
    fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) & ~O_NONBLOCK);

    // A megabyte each way at once: neither side may wait for the other.
    char *out = malloc(BIG), *in = malloc(BIG);
    for (int i = 0; i < BIG; i++) out[i] = (char)(i * 31 + (i >> 9));
    struct drain d = {fd, in, -1};
    pthread_create(&t, NULL, drain_big, &d);
    int wrote = write_all(fd, out, BIG);
    pthread_join(t, NULL);
    printf("a megabyte: written %s, read back %s\n", wrote == 0 ? "whole" : "short", d.result == 0 && memcmp(in, out, BIG) == 0 ? "identical" : "different");

    // A second open is its own connection.
    int other = open(node, O_RDWR);
    write(other, "two!", 4);
    write(fd, "one!", 4);
    char a[8] = {0}, b[8] = {0};
    read_exactly(fd, a, 4);
    read_exactly(other, b, 4);
    printf("two opens: %s %s\n", a, b);
    close(other);
    close(fd);
    printf("closed\n");
    return 0;
}
