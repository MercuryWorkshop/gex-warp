// What Apple's command-line tools ask that a first look at the kernel
// missed: what kind of device a descriptor is, a descriptor opened by its
// number, a file locked as it is opened, who may give a file away, a
// link's own mode, the priority of a process there is none of, and what a
// child that was waited for used.
#include "t.h"

enum { SYS_chown = 16, SYS_fchown = 123, SYS_setpriority = 96, SYS_getpriority = 100, SYS_fchmodat = 467, SYS_fchownat = 468 };
enum { O_SHLOCK = 0x10, O_EXLOCK = 0x20 };
enum { F_GETLK = 7, F_SETLK = 8, F_RDLCK = 1, F_UNLCK = 2, F_WRLCK = 3 };
// _IOR('f', 122, int), _IOR('t', 19, struct termios), _IOR('f', 127, int)
#define FIODTYPE 0x4004667aul
#define TIOCGETA 0x40487413ul
#define FIONREAD 0x4004667ful

static void kind(const char *what, long fd) {
    int type = -1, there = -1;
    char termios[72];
    long r = SYS(SYS_ioctl, fd, FIODTYPE, &type);
    t_printf("%s: its kind %ld", what, r);
    if (r == 0) t_printf(" (%d)", type);
    t_printf(", a terminal's settings %ld", SYS(SYS_ioctl, fd, TIOCGETA, termios));
    r = SYS(SYS_ioctl, fd, FIONREAD, &there);
    t_printf(", how much to read %ld", r);
    if (r == 0) t_printf(" (%d)", there);
    t_printf("\n");
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    long wr = -1;
    long rd = t_syscall2(&wr, SYS_pipe, 0, 0, 0);
    long f = SYS(SYS_open, "f", O_RDWR | O_CREAT, 0644);
    SYS(SYS_write, f, "abc", 3);
    SYS(SYS_lseek, f, 1, 0);
    kind("a pipe", rd);
    kind("the null device", SYS(SYS_open, "/dev/null", O_RDWR));
    kind("the zero device", SYS(SYS_open, "/dev/zero", O_RDONLY));
    kind("a file", f);
    kind("no descriptor", 99);

    // A descriptor by its number: the same open file again, if it was
    // opened for what is asked.
    char b[8];
    SYS(SYS_write, wr, "xyz", 3);
    SYS(SYS_dup2, rd, 0);
    long d = SYS(SYS_open, "/dev/stdin", O_RDONLY);
    t_check("the first descriptor by its name, a pipe", d >= 0);
    t_show("  read through it", SYS(SYS_read, d, b, 2));
    t_show("  and what is left in the pipe", SYS(SYS_read, rd, b, 8));
    SYS(SYS_lseek, f, 0, 0);
    char name[16] = "/dev/fd/";
    name[8] = (char)('0' + f % 10), name[9] = 0;
    if (f >= 10) name[8] = (char)('0' + f / 10), name[9] = (char)('0' + f % 10), name[10] = 0;
    d = SYS(SYS_open, name, O_RDONLY);
    t_check("a file's descriptor by its number", d >= 0);
    t_show("  read through it", SYS(SYS_read, d, b, 2));
    t_show("  and then of the file", SYS(SYS_read, f, b, 8));
    t_check("opened to write, of one open to read and write", SYS(SYS_open, name, O_WRONLY) >= 0);
    long ro = SYS(SYS_open, "f", O_RDONLY);
    name[8] = (char)('0' + ro % 10), name[9] = 0;
    if (ro >= 10) name[8] = (char)('0' + ro / 10), name[9] = (char)('0' + ro % 10), name[10] = 0;
    t_show("opened to write, of one open to read", SYS(SYS_open, name, O_WRONLY));
    t_show("a number that is no descriptor's", SYS(SYS_open, "/dev/fd/99", O_RDONLY));

    // And asked about by its number: what is open there, not a file of
    // `/dev`'s; the first three have names too, which are links.
    struct t_stat ds;
    t_show("the first descriptor asked about by its name", SYS(SYS_stat64, "/dev/stdin", &ds));
    t_printf("  a pipe: %o\n", ds.st_mode & 0170000);
    t_show("the name itself", SYS(SYS_lstat64, "/dev/stdin", &ds));
    t_printf("  a link: %o\n", ds.st_mode & 0170000);
    name[8] = (char)('0' + f % 10), name[9] = 0;
    if (f >= 10) name[8] = (char)('0' + f / 10), name[9] = (char)('0' + f % 10), name[10] = 0;
    t_show("a file's descriptor asked about by its number", SYS(SYS_stat64, name, &ds));
    t_printf("  a file: %o, of %ld bytes\n", ds.st_mode & 0170000, (long)ds.st_size);
    t_show("and with no link followed", SYS(SYS_lstat64, name, &ds));
    t_printf("  a file: %o, of %ld bytes\n", ds.st_mode & 0170000, (long)ds.st_size);
    t_show("a number that is no descriptor's", SYS(SYS_stat64, "/dev/fd/99", &ds));
    t_show("whether it is there", SYS(SYS_access, name, 0));
    t_show("whether one that is not is", SYS(SYS_access, "/dev/fd/99", 0));

    // A file locked as it is opened.
    long e = SYS(SYS_open, "lk", O_CREAT | O_EXLOCK | O_NONBLOCK | O_RDWR, 0644);
    t_check("a new file, locked as it is made", e >= 0);
    t_show("locked again while that is held", SYS(SYS_open, "lk", O_EXLOCK | O_NONBLOCK | O_RDWR));
    t_show("and to share", SYS(SYS_open, "lk", O_SHLOCK | O_NONBLOCK | O_RDONLY));
    SYS(SYS_close, e);
    long s1 = SYS(SYS_open, "lk", O_SHLOCK | O_NONBLOCK | O_RDONLY);
    t_check("shared once it is let go", s1 >= 0);
    t_check("and shared again", SYS(SYS_open, "lk", O_SHLOCK | O_NONBLOCK | O_RDONLY) >= 0);
    t_show("but not had alone", SYS(SYS_open, "lk", O_EXLOCK | O_NONBLOCK | O_RDONLY));

    // Record locks: a part of a file one process has locked is in
    // another's way, and the other is told whose it is.
    struct {
        i64 start, len;
        int pid;
        short type, whence;
    } fl = {128, 1, 0, F_WRLCK, 0};
    long rec = SYS(SYS_open, "rec", O_CREAT | O_RDWR, 0644);
    long self = SYS(SYS_getpid);
    t_show("lock a byte, to write", SYS(SYS_fcntl, rec, F_SETLK, &fl));
    fl.type = F_UNLCK;
    t_show("ask what is in the way of no lock", SYS(SYS_fcntl, rec, F_GETLK, &fl));
    t_printf("  type %d\n", fl.type);
    fl.type = F_WRLCK;
    t_show("and of one's own", SYS(SYS_fcntl, rec, F_GETLK, &fl));
    t_printf("  type %d\n", fl.type);
    long in_child = 0;
    long locker = t_syscall2(&in_child, SYS_fork, 0, 0, 0);
    if (in_child) {
        long mine2 = SYS(SYS_open, "rec", O_RDWR);
        fl.start = 100, fl.len = 100, fl.type = F_RDLCK, fl.pid = 0;
        t_show("another process asks", SYS(SYS_fcntl, mine2, F_GETLK, &fl));
        t_printf("  type %d from %ld for %ld, whence %d\n", fl.type, (long)fl.start, (long)fl.len, fl.whence);
        t_check("  its holder is named", fl.pid == self);
        fl.start = 128, fl.len = 1, fl.type = F_RDLCK;
        t_show("and locks that byte", SYS(SYS_fcntl, mine2, F_SETLK, &fl));
        fl.start = 0, fl.len = 100, fl.type = F_WRLCK;
        t_show("and bytes beside it", SYS(SYS_fcntl, mine2, F_SETLK, &fl));
        fl.type = 9;
        t_show("and with no such kind of lock", SYS(SYS_fcntl, mine2, F_SETLK, &fl));
        t_exit(0);
    }
    int lock_status = -1;
    SYS(SYS_wait4, locker, &lock_status, 0, 0);
    fl.start = 128, fl.len = 1, fl.type = F_UNLCK;
    t_show("unlock it", SYS(SYS_fcntl, rec, F_SETLK, &fl));

    // Who may give a file away.
    long me = SYS(SYS_getuid), mine = SYS(SYS_getgid);
    t_show("give a file to another", SYS(SYS_chown, "f", 12345, -1));
    t_show("to oneself", SYS(SYS_chown, "f", me, -1));
    t_show("to a group that is not one's own", SYS(SYS_chown, "f", -1, 54321));
    t_show("to one's own group", SYS(SYS_chown, "f", -1, mine));
    t_show("by its descriptor, to another", SYS(SYS_fchown, f, 12345, -1));
    t_show("by a directory's, to another", SYS(SYS_fchownat, AT_FDCWD, "f", 12345, -1, 0));
    t_show("nothing of it changed", SYS(SYS_chown, "f", -1, -1));

    // A link's own mode, which is not its file's.
    SYS(SYS_symlink, "f", "l");
    struct t_stat st;
    SYS(SYS_lstat64, "l", &st);
    t_printf("a link's mode: %o\n", st.st_mode & 07777);
    t_show("change the link's own", SYS(SYS_fchmodat, AT_FDCWD, "l", 0600, AT_SYMLINK_NOFOLLOW));
    SYS(SYS_stat64, "f", &st);
    t_printf("  its file's stays %o\n", st.st_mode & 07777);
    // (What a mode is changed to is `files`' to say: the page's files keep
    // none.)
    t_show("change what the link names", SYS(SYS_fchmodat, AT_FDCWD, "l", 0644, 0));

    t_show("the priority of a process there is none of", SYS(SYS_setpriority, 0, 99999, 5));
    t_show("of this one", SYS(SYS_setpriority, 0, 0, 5));

    // What a child used, said in full or not at all: never what the
    // memory held.
    long second = 0;
    long pid = t_syscall2(&second, SYS_fork, 0, 0, 0);
    if (second) t_exit(0);
    struct {
        long utime[2], stime[2], rest[14];
    } ru;
    t_memset(&ru, 0x55, sizeof ru);
    int status = -1;
    t_check("wait for a child", SYS(SYS_wait4, pid, &status, 0, &ru) == pid);
    int sane = ru.utime[0] >= 0 && ru.utime[0] < 100 && (int)ru.utime[1] >= 0 && (int)ru.utime[1] < 1000000 && ru.stime[0] >= 0 && ru.stime[0] < 100 && (int)ru.stime[1] >= 0 && (int)ru.stime[1] < 1000000;
    for (int i = 0; i < 14; i++) sane = sane && ru.rest[i] >= 0 && ru.rest[i] < (1l << 40);
    t_check("what it used is said, and is not what the memory held", sane);
    return 0;
}
