// Measures /dev/gex-services with the synthetic service `bench` (natively
// hosts/native service_bench.rs, GEX_SERVICES_BENCH=1; on the page
// test/darwin/services-bench's provider), as a framework would use it:
//
//   servicebench bulk SIZE COUNT RATE   COUNT frames of SIZE bytes from the
//                                       host, RATE a second (0: as fast as
//                                       they come), on /dev/gex-services-bulk;
//                                       each frame's bytes checked
//   servicebench small COUNT INTERVAL   COUNT round trips of 64-byte frames,
//                                       one each INTERVAL ms; then COUNT
//                                       64-byte events from the host,
//                                       INTERVAL ms apart
//
// It prints one line of numbers for each (a test reads them), and fails
// when a frame is wrong or missing.
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static double now_ms(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1e3 + t.tv_nsec / 1e6;
}

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

static int read_all(int fd, void *p, size_t n) {
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

static int send_json(int fd, const char *json) {
    uint32_t h[2] = {(uint32_t)strlen(json), 0};
    return write_all(fd, h, 8) || write_all(fd, json, strlen(json)) ? -1 : 0;
}

// The next frame into `buf` (of `cap` bytes): its JSON NUL-terminated at
// `buf`, its data after; the data's length in `*dlen`. -1 at the end.
static int recv_frame(int fd, char *buf, size_t cap, size_t *jlen, size_t *dlen) {
    uint32_t h[2];
    if (read_all(fd, h, 8) || (size_t)h[0] + h[1] + 1 > cap) return -1;
    if (read_all(fd, buf, h[0])) return -1;
    buf[h[0]] = 0;
    if (h[1] && read_all(fd, buf + h[0] + 1, h[1])) return -1;
    *jlen = h[0];
    *dlen = h[1];
    return 0;
}

static int connect_to(const char *node) {
    int fd = open(node, O_RDWR);
    if (fd < 0) {
        printf("open %s: %s\n", node, strerror(errno));
        exit(1);
    }
    char buf[256];
    size_t jl, dl;
    if (send_json(fd, "{\"service\":\"bench\",\"version\":1}") || recv_frame(fd, buf, sizeof buf, &jl, &dl) || !strstr(buf, "\"version\":1")) {
        printf("no bench service on %s\n", node);
        exit(1);
    }
    return fd;
}

static int cmp(const void *a, const void *b) {
    double x = *(const double *)a, y = *(const double *)b;
    return x < y ? -1 : x > y;
}

int main(int argc, char **argv) {
    if (argc >= 5 && strcmp(argv[1], "bulk") == 0) {
        size_t size = strtoul(argv[2], NULL, 10);
        int count = atoi(argv[3]);
        double rate = atof(argv[4]);
        int fd = connect_to("/dev/gex-services-bulk");
        size_t cap = size + 4096;
        char *buf = malloc(cap);
        char ask[160];
        snprintf(ask, sizeof ask, "{\"op\":\"pull\",\"seq\":1,\"size\":%zu,\"count\":%d,\"rate\":%g}", size, count, rate);
        double t0 = now_ms(), first = 0, late = 0;
        send_json(fd, ask);
        int got = 0, bad = 0;
        for (;;) {
            size_t jl, dl;
            if (recv_frame(fd, buf, cap, &jl, &dl)) {
                printf("bulk: the stream ended after %d frames\n", got);
                return 1;
            }
            if (strstr(buf, "\"pulled\"")) break;
            double t = now_ms();
            if (!got) first = t;
            // When it was due (paced from the first), and how late it came.
            if (rate > 0 && got) {
                double l = t - (first + got * 1000.0 / rate);
                if (l > late) late = l;
            }
            const unsigned char *d = (const unsigned char *)buf + jl + 1;
            if (dl != size || (size && (d[0] != (unsigned char)got || d[size - 1] != (unsigned char)(got + size - 1)))) bad++;
            got++;
        }
        double t1 = now_ms();
        double span = (t1 - first) / 1000.0;
        printf("bulk: %d frames of %zu bytes, %d wrong; first after %.1f ms; %.1f frames/s, %.0f MB/s; latest %.1f ms behind its time\n", got, size, bad,
               first - t0, got > 1 ? (got - 1) / span : 0.0, got > 1 ? (double)size * (got - 1) / span / 1e6 : 0.0, late);
        return got == count && !bad ? 0 : 1;
    }
    if (argc >= 4 && strcmp(argv[1], "small") == 0) {
        int count = atoi(argv[2]);
        double interval = atof(argv[3]);
        int fd = connect_to("/dev/gex-services");
        char buf[512];
        double *rtt = calloc((size_t)count, sizeof *rtt);
        double t0 = now_ms();
        for (int i = 0; i < count; i++) {
            double due = t0 + i * interval;
            while (now_ms() < due) {
                double wait = due - now_ms();
                if (wait > 0.2) usleep((useconds_t)((wait - 0.1) * 1000));
            }
            // 64 bytes in all: 8 of header, 56 of JSON.
            char msg[64];
            int n = snprintf(msg, sizeof msg, "{\"op\":\"ping\",\"seq\":%d,\"t\":%d,\"p\":\"", i + 2, i);
            while (n < 54) msg[n++] = 'x';
            msg[n++] = '"';
            msg[n++] = '}';
            msg[n] = 0;
            double s = now_ms();
            size_t jl, dl;
            if (send_json(fd, msg) || recv_frame(fd, buf, sizeof buf, &jl, &dl) || !strstr(buf, "pong")) {
                printf("small: no answer to ping %d\n", i);
                return 1;
            }
            rtt[i] = now_ms() - s;
        }
        double span = now_ms() - t0;
        qsort(rtt, (size_t)count, sizeof *rtt, cmp);
        printf("small: %d round trips of 64-byte frames in %.0f ms (%.0f a second); round trip median %.3f ms, p99 %.3f ms, max %.3f ms\n", count, span,
               count * 1000.0 / span, rtt[count / 2], rtt[count * 99 / 100], rtt[count - 1]);
        // And from the host: events INTERVAL ms apart, their gaps here.
        char ask[128];
        snprintf(ask, sizeof ask, "{\"op\":\"ticks\",\"seq\":1,\"count\":%d,\"interval\":%g}", count, interval);
        send_json(fd, ask);
        double *at = calloc((size_t)count, sizeof *at);
        int got = 0;
        for (;;) {
            size_t jl, dl;
            if (recv_frame(fd, buf, sizeof buf, &jl, &dl)) return 1;
            if (strstr(buf, "ticked")) break;
            if (got < count) at[got++] = now_ms();
        }
        // How late each came, against the schedule its first set.
        double *late = calloc((size_t)count, sizeof *late);
        for (int i = 0; i < got; i++) late[i] = at[i] - (at[0] + i * interval);
        double worst = late[0];
        for (int i = 0; i < got; i++)
            if (late[i] > worst) worst = late[i];
        qsort(late, (size_t)got, sizeof *late, cmp);
        printf("ticks: %d of %d events %.1f ms apart; behind their time median %.3f ms, p99 %.3f ms, max %.3f ms\n", got, count, interval, late[got / 2],
               late[got * 99 / 100], worst);
        return got == count ? 0 : 1;
    }
    fprintf(stderr, "usage: servicebench bulk SIZE COUNT RATE | small COUNT INTERVAL\n");
    return 2;
}
