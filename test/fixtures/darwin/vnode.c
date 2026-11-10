// EVFILT_VNODE: a queue told of what happens to a file or a directory it
// has open -- written to, made longer or shorter, its mode changed, given
// a name and losing one, renamed, removed -- and of a directory's entries
// coming and going; each time, or until the note is cleared.
#include "t.h"

enum { SYS_kqueue = 362, SYS_kevent = 363 };
enum { VNODE = -4 };
enum { ADD = 1, DELETE = 2, ONESHOT = 0x10, CLEAR = 0x20 };
enum { N_DELETE = 1, N_WRITE = 2, N_EXTEND = 4, N_ATTRIB = 8, N_LINK = 0x10, N_RENAME = 0x20 };
#define ALL (N_DELETE | N_WRITE | N_EXTEND | N_ATTRIB | N_LINK | N_RENAME | 0x40 | 0x100)

struct kevent {
    u64 ident;
    short filter;
    u16 flags;
    u32 fflags;
    i64 data;
    u64 udata;
};

static long kq;

// What is ready now, a line for each of the notes named: in whatever order
// the kernel has them, which is the order things happened to them in.
static void say(const char *what, const char *const *names) {
    static const struct t_timespec none = {0, 0};
    struct kevent got[8];
    long n = SYS(SYS_kevent, kq, 0, 0, got, 8, &none);
    t_printf("%s: %ld\n", what, n);
    for (int k = 0; names[k]; k++)
        for (long i = 0; i < n && i < 8; i++)
            if (!t_strcmp((const char *)got[i].udata, names[k]))
                t_printf("  %s: filter %d flags 0x%x fflags 0x%x data %ld\n", names[k], got[i].filter, got[i].flags, got[i].fflags, (long)got[i].data);
}
static void watch(long fd, const char *name, int flags, u32 fflags) {
    static const struct t_timespec none = {0, 0};
    struct kevent e = {(u64)fd, VNODE, (u16)flags, fflags, 0, (u64)name};
    t_printf("watch %s", name);
    t_show("", SYS(SYS_kevent, kq, &e, 1, 0, 0, &none));
}

static long shared;

static void *writer(void *arg) {
    struct t_timeval tv = {0, 40000};
    SYS(SYS_select, 0, 0, 0, 0, &tv);
    SYS(SYS_write, shared, "late", 4);
    return arg;
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    static const char *const both[] = {"file", "dir", 0};
    kq = SYS(SYS_kqueue);
    SYS(SYS_mkdir, "d", 0755);
    long f = SYS(SYS_open, "d/f", O_RDWR | O_CREAT, 0644), d = SYS(SYS_open, "d", O_RDONLY);
    watch(f, "file", ADD | CLEAR, ALL);
    watch(d, "dir", ADD | CLEAR, ALL);
    say("nothing yet", both);
    SYS(SYS_write, f, "hello", 5);
    say("written to", both);
    SYS(SYS_pwrite, f, "HE", 2, 0);
    say("written over", both);
    SYS(SYS_write, f, "x", 1);
    SYS(SYS_write, f, "y", 1);
    say("written to twice", both);
    SYS(SYS_ftruncate, f, 2);
    say("made shorter", both);
    SYS(SYS_fchmod, f, 0600);
    say("its mode changed", both);
    SYS(SYS_link, "d/f", "d/g");
    say("given another name", both);
    SYS(SYS_unlink, "d/g");
    say("the other name removed", both);
    SYS(SYS_rename, "d/f", "d/h");
    say("renamed", both);
    long o = SYS(SYS_open, "d/new", O_WRONLY | O_CREAT, 0644);
    say("a file made beside it", both);
    SYS(SYS_write, o, "z", 1);
    say("and written to", both);
    SYS(SYS_close, o);
    SYS(SYS_mkdir, "d/sub", 0755);
    say("a directory made beside it", both);
    SYS(SYS_rmdir, "d/sub");
    say("and removed", both);
    char b[4];
    SYS(SYS_read, f, b, 4);
    say("read", both);
    SYS(SYS_unlink, "d/new");
    say("the other file removed", both);
    SYS(SYS_unlink, "d/h");
    say("removed", both);
    SYS(SYS_close, f);
    SYS(SYS_rename, "d", "e");
    say("the directory renamed", both);
    SYS(SYS_rmdir, "e");
    say("and removed", both);
    SYS(SYS_close, d);
    say("both closed", both);

    // A note that is not cleared says what has happened until it is
    // removed, and of what it was asked to: which a second adding changes,
    // with what it hands back, and nothing else.
    static const char *const plain[] = {"plain", "again", "once", "nothing", 0};
    long g = SYS(SYS_open, "g", O_RDWR | O_CREAT, 0644);
    watch(g, "plain", ADD, N_WRITE);
    SYS(SYS_write, g, "a", 1);
    say("written to", plain);
    say("and still", plain);
    SYS(SYS_fchmod, g, 0600);
    say("its mode changed, which was not asked", plain);
    watch(g, "again", ADD, N_ATTRIB);
    say("asked that instead", plain);
    SYS(SYS_fchmod, g, 0644);
    say("its mode changed", plain);
    watch(g, "once", ADD | ONESHOT, N_WRITE);
    SYS(SYS_unlink, "g");
    say("removed, which was not asked", plain);
    watch(g, "nothing", ADD, 0);
    SYS(SYS_write, g, "a", 1);
    say("written to, with nothing asked", plain);
    watch(g, "nothing", DELETE, 0);
    say("the note removed", plain);
    watch(g, "nothing", DELETE, 0);

    // What is no file, or is not open.
    long wr = -1;
    long rd = t_syscall2(&wr, SYS_pipe, 0, 0, 0);
    watch(rd, "a pipe", ADD, N_WRITE);
    watch(99, "what is not open", ADD, N_WRITE);
    watch(kq, "the queue itself", ADD, N_WRITE);

    // And waited for: what another thread does, a while from now.
    static const char *const waited[] = {"waited", 0};
    shared = SYS(SYS_open, "w", O_RDWR | O_CREAT, 0644);
    watch(shared, "waited", ADD | CLEAR, N_WRITE | N_EXTEND | N_DELETE);
    int th = t_thread_start(writer, 0);
    struct kevent got;
    u64 t0 = t_nanos();
    struct t_timespec long_ = {20, 0};
    long n = SYS(SYS_kevent, kq, 0, 0, &got, 1, &long_);
    t_printf("waited for: %ld, %s fflags 0x%x\n", n, n == 1 ? (const char *)got.udata : "", n == 1 ? got.fflags : 0);
    t_check("for a while", t_nanos() - t0 >= 30000000ull && t_nanos() - t0 < 10000000000ull);
    t_thread_join(th);
    SYS(SYS_unlink, "w");
    say("and removed", waited);
    return 0;
}
