// /dev/gex-services, spoken by hand: the host-services channel and its
// first service, notifications, as a Mac program's framework speaks them
// (pyrus's libpyrusservices and UserNotifications).
//
// A connection asks for a service the host does not have and is told
// version 0, then the end of the stream. Another asks for notifications and
// says hello with an icon; asks for authorization; posts a notification
// with two actions (one taking text) and an image; posts it again under the
// same identifier (it replaces the first); posts a second; sets the badge;
// then prints every frame the host sends (each JSON object as it came)
// until it has been told of the user's answer to both notifications (the
// test's shell answers them), withdraws what is left, and closes. Events
// are read on a thread of their own while the main thread writes, as a
// framework reads them.
//
// usage: services           the channel, with a shell that answers
//        services absent    no node: the open fails
//        services refused   only the service nobody provides
//        services echo      a service `echo` of the page's own: negotiated,
//                           a request with data answered, an event
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static const char *node = "/dev/gex-services";

static int write_all(int fd, const void *p, size_t n) {
    const char *c = p;
    while (n) {
        ssize_t w = write(fd, c, n);
        if (w < 0 && errno == EINTR) continue;
        if (w <= 0) return -1;
        c += w;
        n -= (size_t)w;
    }
    return 0;
}

static int read_exactly(int fd, void *p, size_t n) {
    char *c = p;
    while (n) {
        ssize_t r = read(fd, c, n);
        if (r < 0 && errno == EINTR) continue;
        if (r <= 0) return -1;
        c += r;
        n -= (size_t)r;
    }
    return 0;
}

static void le32(unsigned char *p, uint32_t v) {
    p[0] = v & 0xff;
    p[1] = (v >> 8) & 0xff;
    p[2] = (v >> 16) & 0xff;
    p[3] = v >> 24;
}

static pthread_mutex_t out_lock = PTHREAD_MUTEX_INITIALIZER;

static int send_frame(int fd, const char *json, const void *data, size_t dlen) {
    unsigned char h[8];
    le32(h, (uint32_t)strlen(json));
    le32(h + 4, (uint32_t)dlen);
    pthread_mutex_lock(&out_lock);
    int r = write_all(fd, h, 8) || write_all(fd, json, strlen(json)) || (dlen && write_all(fd, data, dlen));
    pthread_mutex_unlock(&out_lock);
    return r ? -1 : 0;
}

/* One frame: its JSON (NUL-terminated, malloc'd), or NULL at the end. */
static char *recv_frame(int fd) {
    unsigned char h[8];
    if (read_exactly(fd, h, 8)) return NULL;
    uint32_t jl = h[0] | h[1] << 8 | h[2] << 16 | (uint32_t)h[3] << 24;
    uint32_t dl = h[4] | h[5] << 8 | h[6] << 16 | (uint32_t)h[7] << 24;
    char *j = malloc(jl + 1);
    if (read_exactly(fd, j, jl)) return NULL;
    j[jl] = 0;
    char skip[256];
    while (dl) {
        uint32_t n = dl < sizeof skip ? dl : sizeof skip;
        if (read_exactly(fd, skip, n)) return NULL;
        dl -= n;
    }
    return j;
}

/* A 1x1 PNG. */
static const unsigned char png[] = {
    0x89, 'P', 'N', 'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, 'I', 'H', 'D', 'R', 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4,
    0x89, 0, 0, 0, 0x0d, 'I', 'D', 'A', 'T', 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0, 5, 0, 1, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0, 0,
    0, 0, 'I', 'E', 'N', 'D', 0xae, 0x42, 0x60, 0x82,
};

static int answered;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cond = PTHREAD_COND_INITIALIZER;

static void *events(void *arg) {
    int fd = *(int *)arg;
    char *j;
    while ((j = recv_frame(fd))) {
        printf("event %s\n", j);
        fflush(stdout);
        if (strstr(j, "\"op\":\"response\"") || strstr(j, "\"op\":\"cleared\"")) {
            pthread_mutex_lock(&lock);
            answered++;
            pthread_cond_signal(&cond);
            pthread_mutex_unlock(&lock);
        }
        free(j);
    }
    printf("events: the end of the stream\n");
    fflush(stdout);
    return NULL;
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "";
    if (strcmp(mode, "absent") == 0) {
        int fd = open(node, O_RDWR);
        printf("open without the node: %s\n", fd < 0 && errno == ENOENT ? "ENOENT" : "unexpected");
        return 0;
    }
    if (strcmp(mode, "echo") == 0) {
        int fd = open(node, O_RDWR);
        send_frame(fd, "{\"service\":\"echo\",\"version\":9}", NULL, 0);
        char *j = recv_frame(fd);
        printf("echo: %s\n", j ? j : "(nothing)");
        send_frame(fd, "{\"op\":\"ping\",\"seq\":4}", "abc", 3);
        for (int i = 0; i < 2; i++) {
            j = recv_frame(fd);
            printf("echo: %s\n", j ? j : "(nothing)");
        }
        close(fd);
        return 0;
    }
    int no = open(node, O_RDWR);
    if (no < 0) {
        printf("open: %s\n", strerror(errno));
        return 1;
    }
    send_frame(no, "{\"service\":\"no-such-service\",\"version\":3}", NULL, 0);
    char *j = recv_frame(no);
    printf("a service nobody provides: %s\n", j ? j : "(nothing)");
    j = recv_frame(no);
    printf("and then: %s\n", j ? j : "the end of the stream");
    close(no);
    if (strcmp(mode, "refused") == 0) return 0;

    int fd = open(node, O_RDWR);
    send_frame(fd, "{\"service\":\"notifications\",\"version\":7}", NULL, 0);
    j = recv_frame(fd);
    printf("notifications: %s\n", j ? j : "(nothing)");
    if (!j || !strstr(j, "\"version\":1")) return 1;
    char msg[2048];
    snprintf(msg, sizeof msg, "{\"op\":\"hello\",\"app\":{\"id\":\"org.example.services\",\"name\":\"Services\\u0007 Test\",\"icon\":{\"at\":0,\"size\":%zu,\"type\":\"image/png\"}}}", sizeof png);
    send_frame(fd, msg, png, sizeof png);
    j = recv_frame(fd);
    printf("hello: %s\n", j ? j : "(nothing)");
    send_frame(fd, "{\"op\":\"authorize\",\"seq\":1,\"options\":[\"alert\",\"sound\",\"badge\"],\"ask\":false}", NULL, 0);
    j = recv_frame(fd);
    printf("settings before asking: %s\n", j ? j : "(nothing)");
    send_frame(fd, "{\"op\":\"authorize\",\"seq\":2,\"options\":[\"alert\",\"sound\",\"badge\"],\"ask\":true}", NULL, 0);
    j = recv_frame(fd);
    printf("authorization: %s\n", j ? j : "(nothing)");

    pthread_t t;
    pthread_create(&t, NULL, events, &fd);
    const char *post =
        "{\"op\":\"post\",\"id\":\"one\",\"title\":\"Hello\",\"subtitle\":\"from a test\",\"body\":\"Line one\\nline two\",\"sound\":\"default\",\"badge\":2,"
        "\"thread\":\"t1\",\"category\":\"greeting\",\"banner\":true,\"list\":true,\"actions\":["
        "{\"id\":\"ok\",\"title\":\"OK\",\"foreground\":true},"
        "{\"id\":\"reply\",\"title\":\"Reply\",\"text\":{\"button\":\"Send\",\"placeholder\":\"Say something\"}}],"
        "\"image\":{\"at\":0,\"size\":%zu,\"type\":\"image/png\"}}";
    snprintf(msg, sizeof msg, post, sizeof png);
    send_frame(fd, msg, png, sizeof png);
    snprintf(msg, sizeof msg, post, sizeof png);
    send_frame(fd, msg, png, sizeof png);
    send_frame(fd, "{\"op\":\"post\",\"id\":\"two\",\"title\":\"Second\",\"body\":\"<b>not markup</b>\",\"sound\":null,\"list\":true,\"image\":{\"at\":0,\"size\":4,\"type\":\"image/png\"}}", "abcd", 4);
    send_frame(fd, "{\"op\":\"badge\",\"count\":5}", NULL, 0);
    pthread_mutex_lock(&lock);
    while (answered < 2) pthread_cond_wait(&cond, &lock);
    pthread_mutex_unlock(&lock);
    send_frame(fd, "{\"op\":\"withdraw\",\"all\":true}", NULL, 0);
    send_frame(fd, "{\"op\":\"badge\",\"count\":0}", NULL, 0);
    // The host takes what was sent before the end is seen: a frame it
    // cannot read ends the connection, which the event thread hears.
    unsigned char bad[8];
    le32(bad, 1u << 30);
    le32(bad + 4, 0);
    pthread_mutex_lock(&out_lock);
    write_all(fd, bad, 8);
    pthread_mutex_unlock(&out_lock);
    pthread_join(t, NULL);
    close(fd);
    printf("closed\n");
    return 0;
}
