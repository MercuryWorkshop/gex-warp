// Drag and drop between two windows, from programs that have no Wayland
// library (a Mac's has none): the protocol by hand over the compositor's
// socket, as window.c does. One program, three ways:
//
//   dnd source TITLE TEXT [WIDTH HEIGHT]   a window that starts a drag of
//       TEXT (text/plain;charset=utf-8, copy or move) at a button press,
//       with an icon (24x24, its hotspot its centre), and says what its
//       source is told: target, action, send (and writes TEXT), dropped,
//       finished, cancelled. Ends at finished or cancelled.
//   dnd target TITLE [WIDTH HEIGHT]        a window that takes a drag of
//       text: says its enter (with the types offered), its first motion and
//       its leave, accepts text/plain;charset=utf-8 with copy, and at the
//       drop reads the text and says it, then finishes. Ends after a drop.
//   dnd pair TEXT                          both: a source titled "dnd
//       source" and, in a child, a target titled "dnd target".
//
// The socket is $XDG_RUNTIME_DIR/$WAYLAND_DISPLAY (wayland-0 if that is not
// set). A line on its output for each thing, "dnd: ROLE: ...". Builds for a
// Mac (shared memory by name) and for Linux (the same, from /dev/shm).
#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/uio.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

static const char *role = "dnd";
static int sock;
static uint8_t in[65536];
static size_t have, used;
static int fds[16];
static unsigned nfds;

static void say(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    printf("dnd: %s: ", role);
    vprintf(fmt, ap);
    printf("\n");
    va_end(ap);
}

static void fail(const char *what) {
    printf("dnd: %s: %s: %s\n", role, what, strerror(errno));
    exit(1);
}

// One request, with a descriptor to go with it (or -1).
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

static unsigned put_string(uint32_t *to, const char *s) {
    unsigned len = (unsigned)strlen(s) + 1, words = (len + 3) / 4;
    to[0] = len;
    memset(to + 1, 0, 4 * words);
    memcpy(to + 1, s, len);
    return 1 + words;
}

// What arrived, read with the descriptors that came with it.
static int fill(int ms) {
    struct pollfd p = {sock, POLLIN, 0};
    int r = poll(&p, 1, ms);
    if (r == 0) return 0;
    if (r < 0) return errno == EINTR ? 0 : -1;
    memmove(in, in + used, have - used);
    have -= used, used = 0;
    struct iovec iov = {in + have, sizeof in - have};
    union {
        char bytes[CMSG_SPACE(8 * sizeof(int))];
        struct cmsghdr align;
    } control;
    struct msghdr msg = {0};
    msg.msg_iov = &iov, msg.msg_iovlen = 1;
    msg.msg_control = control.bytes, msg.msg_controllen = sizeof control.bytes;
    ssize_t n = recvmsg(sock, &msg, 0);
    if (n <= 0) return -1;
    have += (size_t)n;
    for (struct cmsghdr *c = CMSG_FIRSTHDR(&msg); c; c = CMSG_NXTHDR(&msg, c)) {
        if (c->cmsg_level != SOL_SOCKET || c->cmsg_type != SCM_RIGHTS) continue;
        unsigned k = (unsigned)((c->cmsg_len - CMSG_LEN(0)) / sizeof(int));
        for (unsigned i = 0; i < k && nfds < 16; i++) memcpy(&fds[nfds++], CMSG_DATA(c) + i * sizeof(int), sizeof(int));
    }
    return 1;
}

static int take_fd(void) {
    if (!nfds) return -1;
    int fd = fds[0];
    memmove(fds, fds + 1, --nfds * sizeof(int));
    return fd;
}

enum { DISPLAY = 1, REGISTRY, SYNC, COMPOSITOR, SHM, WM, SEAT, DDM, SURFACE, XDG_SURFACE, TOPLEVEL, POOL, BUFFER, POINTER, KEYBOARD, DEVICE, SOURCE, ICON, ICON_BUFFER, FIRST_FREE };

// The next event, waited for up to `ms`: 1, 0 when the time is up, -1 when
// the compositor has gone. The keyboard's keymap (whose descriptor is not
// one of the drag's) is let go here, wherever it comes.
static int event(uint32_t *object, uint32_t *opcode, const uint32_t **args, int ms) {
    static uint32_t body[16384];
    for (;;) {
        if (have - used >= 8) {
            uint32_t head[2];
            memcpy(head, in + used, 8);
            size_t size = head[1] >> 16;
            if (size < 8 || size > sizeof in) return -1;
            if (have - used >= size) {
                memcpy(body, in + used + 8, size - 8);
                *object = head[0], *opcode = head[1] & 0xffff, *args = body;
                used += size;
                if (*object == KEYBOARD && *opcode == 0) {
                    int k = take_fd();
                    if (k >= 0) close(k);
                    continue;
                }
                return 1;
            }
        }
        int r = fill(ms);
        if (r <= 0) return r;
    }
}


// A shared memory object for pixels, by a name let go at once.
static int shm_file(size_t size) {
    char name[40];
    snprintf(name, sizeof name, "/gex-dnd-%d-%s", (int)getpid(), role);
    int fd = shm_open(name, O_RDWR | O_CREAT | O_EXCL, 0600);
    if (fd < 0) fail("shm_open");
    shm_unlink(name);
    if (ftruncate(fd, (off_t)size)) fail("ftruncate");
    return fd;
}

static int run(int source, const char *title, const char *text, int width, int height) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    const char *dir = getenv("XDG_RUNTIME_DIR"), *name = getenv("WAYLAND_DISPLAY");
    struct sockaddr_un at = {0};
    at.sun_family = AF_UNIX;
    snprintf(at.sun_path, sizeof at.sun_path, "%s/%s", dir ? dir : "/tmp", name ? name : "wayland-0");
    sock = socket(AF_UNIX, SOCK_STREAM, 0);
    if (sock < 0 || connect(sock, (struct sockaddr *)&at, sizeof at)) fail(at.sun_path);

    uint32_t a[64];
    a[0] = REGISTRY;
    request(DISPLAY, 1, a, 1, -1);
    a[0] = SYNC;
    request(DISPLAY, 0, a, 1, -1);
    uint32_t names[5] = {0}, versions[5] = {0};
    const char *want[5] = {"wl_compositor", "wl_shm", "xdg_wm_base", "wl_seat", "wl_data_device_manager"};
    uint32_t object, opcode;
    const uint32_t *args;
    for (;;) {
        if (event(&object, &opcode, &args, 20000) <= 0) fail("no answer from the compositor");
        if (object == SYNC) break;
        if (object != REGISTRY || opcode != 0) continue;
        for (int i = 0; i < 5; i++)
            if (!strcmp((const char *)(args + 2), want[i])) names[i] = args[0], versions[i] = args[2 + (args[1] + 3) / 4];
    }
    for (int i = 0; i < 5; i++)
        if (!names[i]) {
            say("the compositor has no %s", want[i]);
            return 1;
        }
    const uint32_t ids[5] = {COMPOSITOR, SHM, WM, SEAT, DDM}, most[5] = {4, 1, 2, 5, 3};
    for (int i = 0; i < 5; i++) {
        unsigned n = 0;
        a[n++] = names[i];
        n += put_string(a + n, want[i]);
        a[n++] = versions[i] < most[i] ? versions[i] : most[i];
        a[n++] = ids[i];
        request(REGISTRY, 0, a, n, -1);
    }
    a[0] = POINTER;
    request(SEAT, 0, a, 1, -1);
    a[0] = KEYBOARD;
    request(SEAT, 1, a, 1, -1);
    a[0] = DEVICE, a[1] = SEAT;
    request(DDM, 1, a, 2, -1);

    a[0] = SURFACE;
    request(COMPOSITOR, 0, a, 1, -1);
    a[0] = XDG_SURFACE, a[1] = SURFACE;
    request(WM, 2, a, 2, -1);
    a[0] = TOPLEVEL;
    request(XDG_SURFACE, 1, a, 1, -1);
    request(TOPLEVEL, 2, a, put_string(a, title), -1);
    request(SURFACE, 6, a, 0, -1);
    for (;;) {
        if (event(&object, &opcode, &args, 20000) <= 0) fail("no configure from the compositor");
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

    // The window's pixels, and after them the icon's (24x24), in one pool.
    const int icon = 24;
    size_t win_size = (size_t)width * 4 * (size_t)height, size = win_size + icon * icon * 4;
    int fd = shm_file(size);
    uint32_t *pixels = mmap(0, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (pixels == MAP_FAILED) fail("mmap");
    for (int i = 0; i < width * height; i++) pixels[i] = source ? 0xff2060c0 : 0xff20a040;
    for (int i = 0; i < icon * icon; i++) pixels[width * height + i] = 0xffe0c020;
    a[0] = POOL, a[1] = (uint32_t)size;
    request(SHM, 0, a, 2, fd);
    a[0] = BUFFER, a[1] = 0, a[2] = (uint32_t)width, a[3] = (uint32_t)height, a[4] = (uint32_t)width * 4, a[5] = 1;
    request(POOL, 0, a, 6, -1);
    a[0] = BUFFER, a[1] = 0, a[2] = 0;
    request(SURFACE, 1, a, 3, -1);
    a[0] = 0, a[1] = 0, a[2] = (uint32_t)width, a[3] = (uint32_t)height;
    request(SURFACE, 2, a, 4, -1);
    request(SURFACE, 6, a, 0, -1);
    if (source) {
        // The icon, drawn before the drag that shows it.
        a[0] = ICON;
        request(COMPOSITOR, 0, a, 1, -1);
        a[0] = ICON_BUFFER, a[1] = (uint32_t)win_size, a[2] = icon, a[3] = icon, a[4] = icon * 4, a[5] = 1;
        request(POOL, 0, a, 6, -1);
        a[0] = ICON_BUFFER, a[1] = 0, a[2] = 0;
        request(ICON, 1, a, 3, -1);
        a[0] = 0, a[1] = 0, a[2] = icon, a[3] = icon;
        request(ICON, 2, a, 4, -1);
        request(ICON, 6, a, 0, -1);
    }
    say("shown, %d by %d", width, height);

    // The target's offers, as they come: their id, types, and whether they
    // have text.
    uint32_t offer = 0, offer_text = 0, enter_serial = 0;
    char types[512] = "";
    int moved = 0, dragging = 0;
    for (;;) {
        int r = event(&object, &opcode, &args, 60000);
        if (r < 0) {
            say("the compositor has gone");
            return 1;
        }
        if (r == 0) {
            say("nothing happened for a minute");
            return 1;
        }
        if (object == WM && opcode == 0) {
            a[0] = args[0];
            request(WM, 3, a, 1, -1);
        }
        if (object == XDG_SURFACE && opcode == 0) {
            a[0] = args[0];
            request(XDG_SURFACE, 4, a, 1, -1);
        }
        if (object == TOPLEVEL && opcode == 1) return 0;
        if (object == POINTER && opcode == 3 && args[3] && source && !dragging) {
            // A press: the drag, as its answer (button(serial, time, which, state)).
            dragging = 1;
            uint32_t serial = args[0];
            a[0] = SOURCE;
            request(DDM, 0, a, 1, -1);
            request(SOURCE, 0, a, put_string(a, "text/plain;charset=utf-8"), -1);
            request(SOURCE, 0, a, put_string(a, "text/plain"), -1);
            a[0] = 1 | 2;
            request(SOURCE, 2, a, 1, -1);
            a[0] = SOURCE, a[1] = SURFACE, a[2] = ICON, a[3] = serial;
            request(DEVICE, 0, a, 4, -1);
            // The icon's hotspot its centre: moved up and left by half.
            a[0] = ICON_BUFFER, a[1] = (uint32_t)-(icon / 2), a[2] = (uint32_t)-(icon / 2);
            request(ICON, 1, a, 3, -1);
            request(ICON, 6, a, 0, -1);
            say("pressed: start_drag");
        } else if (object == POINTER && opcode == 3) {
            say("button %u %s", args[2], args[3] ? "down" : "up");
        }
        if (object == SOURCE) {
            if (opcode == 0) say("told target %s", args[0] ? (const char *)(args + 1) : "(none)");
            if (opcode == 1) {
                int out = take_fd();
                say("asked to send %s", (const char *)(args + 1));
                if (out >= 0) {
                    ssize_t w = write(out, text, strlen(text));
                    (void)w;
                    close(out);
                } else {
                    say("asked to send, with no descriptor");
                }
            }
            if (opcode == 2) {
                say("told cancelled");
                return 0;
            }
            if (opcode == 3) say("told dropped");
            if (opcode == 4) {
                say("told finished");
                return 0;
            }
            if (opcode == 5) say("told action %u", args[0]);
        }
        // data_offer(id) and the offer's own events: offer(type), source_actions, action.
        if (object == DEVICE && opcode == 0) offer = args[0], offer_text = 0, types[0] = 0;
        if (object == offer && opcode == 0) {
            const char *t = (const char *)(args + 1);
            if (!strcmp(t, "text/plain;charset=utf-8")) offer_text = 1;
            if (strlen(types) + strlen(t) + 2 < sizeof types) {
                if (types[0]) strcat(types, " ");
                strcat(types, t);
            }
        }
        // enter(serial, surface, x, y, offer)
        if (object == DEVICE && opcode == 1) {
            enter_serial = args[0];
            moved = 0;
            say("enter at %d,%d offering %s", (int32_t)args[2] >> 8, (int32_t)args[3] >> 8, types);
            // accept(serial, type or null), then set_actions(copy, copy).
            uint32_t to = args[4];
            unsigned n = 1;
            a[0] = enter_serial;
            if (offer_text) n += put_string(a + 1, "text/plain;charset=utf-8");
            else a[n++] = 0;
            request(to, 0, a, n, -1);
            a[0] = 1, a[1] = 1;
            request(to, 4, a, 2, -1);
        }
        if (object == DEVICE && opcode == 2) say("leave");
        if (object == DEVICE && opcode == 3 && !moved++) say("motion at %d,%d", (int32_t)args[1] >> 8, (int32_t)args[2] >> 8);
        if (object == DEVICE && opcode == 4) {
            // The drop: the text, read through a pipe whose end the source writes.
            int p[2];
            if (pipe(p)) fail("pipe");
            request(offer, 1, a, put_string(a, "text/plain;charset=utf-8"), p[1]);
            close(p[1]);
            char got[4096];
            size_t n = 0;
            for (;;) {
                // The compositor's events meanwhile (the source may be this
                // very client).
                struct pollfd q[2] = {{p[0], POLLIN, 0}, {sock, POLLIN, 0}};
                if (poll(q, 2, 20000) <= 0) break;
                if (q[1].revents && fill(0) < 0) break;
                if (!q[0].revents) {
                    // (a send for this client's own source: answered here)
                    while (have - used >= 8) {
                        if (event(&object, &opcode, &args, 0) <= 0) break;
                        if (object == SOURCE && opcode == 1) {
                            int out = take_fd();
                            if (out >= 0) {
                                ssize_t w = write(out, text, strlen(text));
                                (void)w;
                                close(out);
                            }
                        }
                    }
                    continue;
                }
                ssize_t k = read(p[0], got + n, sizeof got - 1 - n);
                if (k <= 0) break;
                n += (size_t)k;
            }
            got[n] = 0;
            close(p[0]);
            say("dropped: \"%s\"", got);
            request(offer, 3, a, 0, -1); // finish
            request(offer, 2, a, 0, -1); // destroy
            // A roundtrip, so the finish is read before this client goes.
            a[0] = FIRST_FREE + 100;
            request(DISPLAY, 0, a, 1, -1);
            for (int i = 0; i < 50; i++) {
                if (event(&object, &opcode, &args, 2000) <= 0 || object == FIRST_FREE + 100) break;
            }
            if (!source) return 0;
        }
    }
}

int main(int argc, char **argv) {
    if (argc > 1 && !strcmp(argv[1], "pair")) {
        const char *text = argc > 2 ? argv[2] : "hello";
        pid_t child = fork();
        if (child == 0) {
            role = "target";
            return run(0, "dnd target", text, 300, 200);
        }
        role = "source";
        int r = run(1, "dnd source", text, 300, 200);
        int status = 0;
        waitpid(child, &status, 0);
        printf("dnd: the target ended, status %d\n", WIFEXITED(status) ? WEXITSTATUS(status) : -1);
        return r;
    }
    if (argc > 3 && !strcmp(argv[1], "source")) {
        role = "source";
        return run(1, argv[2], argv[3], argc > 5 ? atoi(argv[4]) : 300, argc > 5 ? atoi(argv[5]) : 200);
    }
    if (argc > 2 && !strcmp(argv[1], "target")) {
        role = "target";
        return run(0, argv[2], "", argc > 4 ? atoi(argv[3]) : 300, argc > 4 ? atoi(argv[4]) : 200);
    }
    fprintf(stderr, "usage: dnd source TITLE TEXT [W H] | dnd target TITLE [W H] | dnd pair TEXT\n");
    return 2;
}
