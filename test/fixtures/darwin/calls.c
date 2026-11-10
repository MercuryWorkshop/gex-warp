// What Apple's command-line tools ask that a first look at the kernel
// missed: what kind of device a descriptor is, a descriptor opened by its
// number, a file locked as it is opened, who may give a file away, a
// link's own mode, the priority of a process there is none of, and what a
// child that was waited for used.
#include "t.h"

enum { SYS_chown = 16, SYS_fchown = 123, SYS_setpriority = 96, SYS_getpriority = 100, SYS_fchmodat = 467, SYS_fchownat = 468 };
enum { O_SHLOCK = 0x10, O_EXLOCK = 0x20 };
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
