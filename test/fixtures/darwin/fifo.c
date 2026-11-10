// FIFOs: a file that names a pipe, made by mkfifo and by mknod, which one
// process writes and another reads, whichever of them opens it first.
#include "t.h"

enum { SYS_mknod = 14, SYS_mkfifo = 132 };
enum { S_IFIFO = 0010000, S_IFMT = 0170000 };

static void nap(long us) {
    struct t_timeval tv = {0, us};
    SYS(SYS_select, 0, 0, 0, 0, &tv);
}

// A child writes the FIFO in two parts and closes it; this process reads
// it to its end. One of them opens it a while before the other.
static void turn(const char *what, int reader_first) {
    long second = 0;
    long pid = t_syscall2(&second, SYS_fork, 0, 0, 0);
    if (second) {
        if (reader_first) nap(150000);
        long w = SYS(SYS_open, "ff", O_WRONLY, 0);
        SYS(SYS_write, w, "hello ", 6);
        nap(50000);
        SYS(SYS_write, w, "fifo", 4);
        SYS(SYS_close, w);
        t_exit(0);
    }
    if (!reader_first) nap(150000);
    long r = SYS(SYS_open, "ff", O_RDONLY, 0);
    char buf[64];
    long n, total = 0;
    while ((n = SYS(SYS_read, r, buf + total, sizeof buf - 1 - total)) > 0) total += n;
    buf[total] = 0;
    t_printf("%s: read \"%s\", and then %ld\n", what, buf, n);
    SYS(SYS_close, r);
    int status = -1;
    SYS(SYS_wait4, pid, &status, 0, 0);
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    struct t_stat st;
    SYS(SYS_umask, 022);
    t_show("make a FIFO", SYS(SYS_mkfifo, "ff", 0666));
    SYS(SYS_stat64, "ff", &st);
    t_printf("  it is one: %s, of mode %o and %ld bytes\n", (st.st_mode & S_IFMT) == S_IFIFO ? "yes" : "no", st.st_mode & 07777, (long)st.st_size);
    t_show("make it again", SYS(SYS_mkfifo, "ff", 0666));
    turn("the reader first", 1);
    turn("the writer first", 0);
    turn("and again, a pipe of its own each time", 1);
    t_show("open it to write with nobody reading, not to wait", SYS(SYS_open, "ff", O_WRONLY | O_NONBLOCK, 0));
    long r = SYS(SYS_open, "ff", O_RDONLY | O_NONBLOCK, 0);
    t_check("open it to read with nobody writing, not to wait", r >= 0);
    char b[4];
    t_show("  and read", SYS(SYS_read, r, b, 4));
    long w = SYS(SYS_open, "ff", O_WRONLY | O_NONBLOCK, 0);
    t_check("  to write, now that there is a reader", w >= 0);
    t_show("  written", SYS(SYS_write, w, "ab", 2));
    t_show("  read", SYS(SYS_read, r, b, 4));
    SYS(SYS_fstat64, r, &st);
    t_printf("  its descriptor is a FIFO's: %s\n", (st.st_mode & S_IFMT) == S_IFIFO ? "yes" : "no");
    SYS(SYS_close, w);
    t_show("  read once the writer has gone", SYS(SYS_read, r, b, 4));
    SYS(SYS_close, r);
    // To read and write both waits for nobody, and the pipe does not end
    // between one writer and the next.
    long rw = SYS(SYS_open, "ff", O_RDWR, 0);
    t_check("open it to read and write", rw >= 0);
    t_show("  written", SYS(SYS_write, rw, "abc", 3));
    t_show("  read", SYS(SYS_read, rw, b, 4));
    w = SYS(SYS_open, "ff", O_WRONLY | O_NONBLOCK, 0);
    t_check("  another writer, not to wait", w >= 0);
    SYS(SYS_write, w, "z", 1);
    SYS(SYS_close, w);
    t_show("  read what it wrote", SYS(SYS_read, rw, b, 4));
    SYS(SYS_fcntl, rw, 4, O_RDWR | O_NONBLOCK);
    t_show("  and after it has gone", SYS(SYS_read, rw, b, 4));
    t_printf("  open for %ld\n", SYS(SYS_fcntl, rw, 3, 0) & 3);
    SYS(SYS_close, rw);
    t_show("rename it", SYS(SYS_rename, "ff", "gg"));
    t_show("unlink it", SYS(SYS_unlink, "gg"));
    t_show("open it once it is gone", SYS(SYS_open, "ff", O_RDONLY | O_NONBLOCK, 0));
    // And as a node of that kind.
    t_show("make one by mknod", SYS(SYS_mknod, "ff", S_IFIFO | 0600, 0));
    SYS(SYS_stat64, "ff", &st);
    t_printf("  it is one: %s, of mode %o\n", (st.st_mode & S_IFMT) == S_IFIFO ? "yes" : "no", st.st_mode & 07777);
    t_show("a device by mknod", SYS(SYS_mknod, "dev", 0020000 | 0600, 0x0302));
    SYS(SYS_unlink, "ff");
    return 0;
}
