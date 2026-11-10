// A window on a Wayland compositor from a program that has no Wayland
// library (a Mac's has none): the protocol by hand over the compositor's
// socket. It asks what the compositor has, makes a surface a top-level
// window with a title, shares a buffer of pixels with the compositor
// (POSIX shared memory, its descriptor passed over the socket), shows it,
// and answers the compositor until its input ends, saying what the pointer
// and the keyboard do in it.
//
//   window TITLE [WIDTH HEIGHT]
//
// The socket is $XDG_RUNTIME_DIR/$WAYLAND_DISPLAY (wayland-0 if that is not
// set). Says what it does on its output, a line each.
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

static int sock;
static uint8_t in[65536];
static size_t have;

static void fail(const char *what) {
    printf("window: %s: %s\n", what, strerror(errno));
    exit(1);
}

// One request: `words` 32-bit words after the header, and a descriptor to
// go with it (or -1).
static void request(uint32_t object, uint32_t opcode, const uint32_t *args, unsigned words, int fd) {
    uint32_t m[64];
    m[0] = object, m[1] = (uint32_t)((8 + 4 * words) << 16) | opcode;
    memcpy(m + 2, args, 4 * words);
    struct iovec iov = {m, 8 + 4 * words};
    struct msghdr msg = {0};
    msg.msg_iov = &iov, msg.msg_iovlen = 1;
    union {
        char bytes[CMSG_SPACE(sizeof(int))];
        struct cmsghdr align;
    } control;
    if (fd >= 0) {
        memset(&control, 0, sizeof control);
        msg.msg_control = control.bytes, msg.msg_controllen = sizeof control.bytes;
        struct cmsghdr *c = CMSG_FIRSTHDR(&msg);
        c->cmsg_level = SOL_SOCKET, c->cmsg_type = SCM_RIGHTS, c->cmsg_len = CMSG_LEN(sizeof(int));
        memcpy(CMSG_DATA(c), &fd, sizeof fd);
    }
    if (sendmsg(sock, &msg, 0) != (ssize_t)iov.iov_len) fail("sendmsg");
}

// A string as the protocol carries one, into `to`: how many words.
static unsigned put_string(uint32_t *to, const char *s) {
    unsigned len = (unsigned)strlen(s) + 1, words = (len + 3) / 4;
    to[0] = len;
    memset(to + 1, 0, 4 * words);
    memcpy(to + 1, s, len);
    return 1 + words;
}

// The next event, waited for up to `ms` (forever if negative): its object
// and opcode, its arguments at *args. 0 when the time is up, -1 when the
// compositor has gone.
static int event(uint32_t *object, uint32_t *opcode, const uint32_t **args, unsigned *words, int ms) {
    static size_t used;
    memmove(in, in + used, have - used);
    have -= used, used = 0;
    for (;;) {
        if (have >= 8) {
            uint32_t head[2];
            memcpy(head, in, 8);
            size_t size = head[1] >> 16;
            if (size < 8 || size > sizeof in) return -1;
            if (have >= size) {
                static uint32_t body[16384];
                memcpy(body, in + 8, size - 8);
                *object = head[0], *opcode = head[1] & 0xffff, *args = body, *words = (unsigned)(size - 8) / 4;
                used = size;
                return 1;
            }
        }
        struct pollfd p = {sock, POLLIN, 0};
        int r = poll(&p, 1, ms);
        if (r == 0) return 0;
        if (r < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        ssize_t n = read(sock, in + have, sizeof in - have);
        if (n <= 0) return -1;
        have += (size_t)n;
    }
}

enum { DISPLAY = 1, REGISTRY, SYNC, COMPOSITOR, SHM, WM, SURFACE, XDG_SURFACE, TOPLEVEL, POOL, BUFFER, SEAT, POINTER, KEYBOARD };

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    const char *title = argc > 1 ? argv[1] : "window";
    int width = argc > 3 ? atoi(argv[2]) : 320, height = argc > 3 ? atoi(argv[3]) : 200;
    const char *dir = getenv("XDG_RUNTIME_DIR"), *name = getenv("WAYLAND_DISPLAY");
    struct sockaddr_un at = {0};
    at.sun_family = AF_UNIX;
    snprintf(at.sun_path, sizeof at.sun_path, "%s/%s", dir ? dir : "/tmp", name ? name : "wayland-0");
    sock = socket(AF_UNIX, SOCK_STREAM, 0);
    if (sock < 0 || connect(sock, (struct sockaddr *)&at, sizeof at)) fail(at.sun_path);
    printf("window: connected\n");

    // What the compositor has: its registry's globals, up to the answer to
    // a sync asked after them.
    uint32_t a[40];
    a[0] = REGISTRY;
    request(DISPLAY, 1, a, 1, -1);
    a[0] = SYNC;
    request(DISPLAY, 0, a, 1, -1);
    uint32_t compositor = 0, shm = 0, wm = 0, seat = 0, compositor_version = 0, wm_version = 0, seat_version = 0;
    uint32_t object, opcode;
    const uint32_t *args;
    unsigned words;
    for (;;) {
        if (event(&object, &opcode, &args, &words, 20000) <= 0) fail("no answer from the compositor");
        if (object == SYNC) break;
        if (object == DISPLAY && opcode == 0) {
            printf("window: the compositor says: %s\n", (const char *)(args + 3));
            return 1;
        }
        if (object != REGISTRY || opcode != 0) continue;
        const char *interface = (const char *)(args + 2);
        uint32_t version = args[2 + (args[1] + 3) / 4];
        if (!strcmp(interface, "wl_compositor")) compositor = args[0], compositor_version = version;
        if (!strcmp(interface, "wl_shm")) shm = args[0];
        if (!strcmp(interface, "xdg_wm_base")) wm = args[0], wm_version = version;
        if (!strcmp(interface, "wl_seat")) seat = args[0], seat_version = version;
    }
    printf("window: a compositor %s, shared memory %s, windows %s\n", compositor ? "yes" : "no", shm ? "yes" : "no", wm ? "yes" : "no");
    if (!compositor || !shm || !wm) return 1;
    const struct {
        uint32_t *name, id;
        const char *interface;
        uint32_t *has, most;
    } binds[] = {{&compositor, COMPOSITOR, "wl_compositor", &compositor_version, 4}, {&shm, SHM, "wl_shm", 0, 1}, {&wm, WM, "xdg_wm_base", &wm_version, 2}, {&seat, SEAT, "wl_seat", &seat_version, 5}};
    for (unsigned i = 0; i < 4; i++) {
        if (!*binds[i].name) continue;
        unsigned n = 0;
        a[n++] = *binds[i].name;
        n += put_string(a + n, binds[i].interface);
        a[n++] = binds[i].has && *binds[i].has < binds[i].most ? *binds[i].has : binds[i].most;
        a[n++] = binds[i].id;
        request(REGISTRY, 0, a, n, -1);
    }

    // What there is to point and type with, if there is a seat.
    if (seat) {
        a[0] = POINTER;
        request(SEAT, 0, a, 1, -1);
        a[0] = KEYBOARD;
        request(SEAT, 1, a, 1, -1);
    }

    // A surface, and its part as a top-level window with a title.
    a[0] = SURFACE;
    request(COMPOSITOR, 0, a, 1, -1);
    a[0] = XDG_SURFACE, a[1] = SURFACE;
    request(WM, 2, a, 2, -1);
    a[0] = TOPLEVEL;
    request(XDG_SURFACE, 1, a, 1, -1);
    request(TOPLEVEL, 2, a, put_string(a, title), -1);
    request(SURFACE, 6, a, 0, -1);

    // The compositor says how it is to be (a size, or none of its own),
    // which is acknowledged before anything is shown.
    for (;;) {
        if (event(&object, &opcode, &args, &words, 20000) <= 0) fail("no configure from the compositor");
        if (object == WM && opcode == 0) {
            a[0] = args[0];
            request(WM, 3, a, 1, -1);
        }
        if (object == TOPLEVEL && opcode == 0 && args[0] && args[1]) width = (int)args[0], height = (int)args[1];
        if (object == XDG_SURFACE && opcode == 0) {
            a[0] = args[0];
            request(XDG_SURFACE, 4, a, 1, -1);
            break;
        }
    }
    printf("window: configured\n");

    // The pixels: memory shared by its name, which is then let go, and by
    // its descriptor, which the compositor is sent.
    int stride = width * 4;
    size_t size = (size_t)stride * (size_t)height;
    char shm_name[32];
    snprintf(shm_name, sizeof shm_name, "/gex-window-%d", (int)getpid());
    int fd = shm_open(shm_name, O_RDWR | O_CREAT | O_EXCL, 0600);
    if (fd < 0) fail("shm_open");
    shm_unlink(shm_name);
    if (ftruncate(fd, (off_t)size)) fail("ftruncate");
    uint32_t *pixels = mmap(0, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (pixels == MAP_FAILED) fail("mmap");
    // A border, and inside it bands of red, green and blue.
    for (int y = 0; y < height; y++)
        for (int x = 0; x < width; x++) {
            int edge = x < 8 || y < 8 || x >= width - 8 || y >= height - 8;
            uint32_t band = y * 3 / height == 0 ? 0xffe03020 : y * 3 / height == 1 ? 0xff20c040 : 0xff3050e0;
            pixels[y * width + x] = edge ? 0xffffffff : band;
        }
    a[0] = POOL, a[1] = (uint32_t)size;
    request(SHM, 0, a, 2, fd);
    a[0] = BUFFER, a[1] = 0, a[2] = (uint32_t)width, a[3] = (uint32_t)height, a[4] = (uint32_t)stride, a[5] = 1;
    request(POOL, 0, a, 6, -1);
    a[0] = BUFFER, a[1] = 0, a[2] = 0;
    request(SURFACE, 1, a, 3, -1);
    a[0] = 0, a[1] = 0, a[2] = (uint32_t)width, a[3] = (uint32_t)height;
    request(SURFACE, 2, a, 4, -1);
    request(SURFACE, 6, a, 0, -1);
    printf("window: shown, %d by %d\n", width, height);

    // Until its input ends: the compositor's pings answered, a new size
    // acknowledged, its word that the window is closed heeded; and what the
    // pointer and the keyboard do here said: where the pointer is when a
    // button goes down (in the window or not), and each key by its number.
    int px = -1, py = -1;
    for (;;) {
        struct pollfd p[2] = {{sock, POLLIN, 0}, {0, POLLIN, 0}};
        if (have < 8 && poll(p, 2, -1) < 0 && errno != EINTR) break;
        if (p[1].revents) {
            char c;
            if (read(0, &c, 1) <= 0) break;
        }
        if (have < 8 && !(p[0].revents)) continue;
        int r = event(&object, &opcode, &args, &words, 0);
        if (r < 0) {
            printf("window: the compositor has gone\n");
            return 1;
        }
        if (r == 0) continue;
        if (object == WM && opcode == 0) {
            a[0] = args[0];
            request(WM, 3, a, 1, -1);
        }
        if (object == XDG_SURFACE && opcode == 0) {
            a[0] = args[0];
            request(XDG_SURFACE, 4, a, 1, -1);
        }
        if (object == TOPLEVEL && opcode == 1) {
            printf("window: closed\n");
            return 0;
        }
        // The pointer comes in (serial, surface, x, y) and moves (time, x,
        // y), in 24.8 fixed point; a button is (serial, time, which, down).
        if (object == POINTER && opcode == 0) px = (int32_t)args[2] >> 8, py = (int32_t)args[3] >> 8;
        if (object == POINTER && opcode == 2) px = (int32_t)args[1] >> 8, py = (int32_t)args[2] >> 8;
        if (object == POINTER && opcode == 3)
            printf("window: button %u %s, the pointer %s the window\n", args[2], args[3] ? "down" : "up", px >= 0 && py >= 0 && px < width && py < height ? "in" : "not in");
        // A key is (serial, time, which, down).
        if (object == KEYBOARD && opcode == 3) printf("window: key %u %s\n", args[2], args[3] ? "down" : "up");
    }
    printf("window: done\n");
    return 0;
}
