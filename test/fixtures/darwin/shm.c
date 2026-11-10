// POSIX shared memory by its name, as one program shares a buffer with
// another: made, sized, mapped, found again by a child under its name, and
// unlinked; and what a descriptor of it will not do.
#include "t.h"

#if defined(__arm64__)
#define PAGE 16384L
#else
#define PAGE 4096L
#endif

enum { SYS_shm_open = 266, SYS_shm_unlink = 267 };

static void about(const char *what, long fd) {
    struct t_stat st;
    long r = SYS(SYS_fstat64, fd, &st);
    t_printf("%s: fstat %ld, kind %o, mode %o, of %ld pages and %ld bytes\n", what, r, st.st_mode & 0170000, st.st_mode & 07777, (long)(st.st_size / PAGE),
             (long)(st.st_size % PAGE));
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    static const char name[] = "/gex-test-shm";
    SYS(SYS_shm_unlink, name);
    t_show("unlink one there is none of", SYS(SYS_shm_unlink, name));
    t_show("open one there is none of", SYS(SYS_shm_open, name, O_RDWR, 0));
    long fd = SYS(SYS_shm_open, name, O_CREAT | O_EXCL | O_RDWR, 0600);
    t_check("make one", fd >= 0);
    t_show("its descriptor's flags", SYS(SYS_fcntl, fd, F_GETFD, 0));
    about("new", fd);
    t_show("make it again, which must be new", SYS(SYS_shm_open, name, O_CREAT | O_EXCL | O_RDWR, 0600));
    t_show("map it before it has a size", SYS(SYS_mmap, 0, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0));
    t_show("size it", SYS(SYS_ftruncate, fd, 2 * PAGE + 100));
    about("sized", fd);
    long map = SYS(SYS_mmap, 0, 3 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    t_check("map it", map > 0);
    char *p = (char *)map;
    t_check("zeroed", p[0] == 0 && p[2 * PAGE + 99] == 0);
    p[0] = 'P';

    // A child finds it by its name, and what each writes the other reads.
    // (Each on a page of its own: the kernel's shared mappings are copies
    // brought up to date page by page, and a page two processes have both
    // written since is one's or the other's.)
    long second = 0;
    long pid = t_syscall2(&second, SYS_fork, 0, 0, 0);
    if (second) {
        long f2 = SYS(SYS_shm_open, name, O_RDWR, 0);
        long m2 = SYS(SYS_mmap, 0, 3 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, f2, 0);
        if (f2 < 0 || m2 <= 0) t_exit(2);
        char *q = (char *)m2;
        if (q[0] != 'P') t_exit(3);
        q[2 * PAGE + 1] = 'C';
        p[PAGE + 1] = 'c';
        t_exit(0);
    }
    int status = -1;
    SYS(SYS_wait4, pid, &status, 0, 0);
    t_printf("the child ended %d\n", (status >> 8) & 0xff);
    t_check("what it wrote by its own mapping", p[2 * PAGE + 1] == 'C');
    t_check("and by the one it was born with", p[PAGE + 1] == 'c');

    // It is sized once. (Nor is it read, written or sought in by its
    // descriptor on a Mac, ENXIO and ESPIPE: here it is, as a file.)
    t_show("size it again", SYS(SYS_ftruncate, fd, 4 * PAGE));
    about("still", fd);
    long ro = SYS(SYS_shm_open, name, O_RDONLY, 0);
    t_check("open it to read", ro >= 0);
    t_show("map that to write", SYS(SYS_mmap, 0, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, ro, 0));
    t_show("map more of it than there is", SYS(SYS_mmap, 0, 4 * PAGE, PROT_READ, MAP_SHARED, ro, 0));
    t_show("map it from past its end", SYS(SYS_mmap, 0, PAGE, PROT_READ, MAP_SHARED, ro, 3 * PAGE));
    t_show("map it for this process alone", SYS(SYS_mmap, 0, PAGE, PROT_READ, MAP_PRIVATE, ro, 0));
    long rm = SYS(SYS_mmap, 0, PAGE, PROT_READ, MAP_SHARED, ro, 0);
    t_check("and to read", rm > 0 && ((char *)rm)[0] == 'P');

    t_show("unlink it", SYS(SYS_shm_unlink, name));
    t_show("open it once it is unlinked", SYS(SYS_shm_open, name, O_RDWR, 0));
    p[2] = 'x';
    t_check("its mappings stay", ((char *)rm)[2] == 'x');
    long again = SYS(SYS_shm_open, name, O_CREAT | O_RDWR, 0600);
    t_check("one of the name again", again >= 0);
    about("which is new", again);
    SYS(SYS_shm_unlink, name);
    t_show("a name too long", SYS(SYS_shm_open, "/gex-test-shm-with-a-name-longer-than-is-kept", O_CREAT | O_RDWR, 0600));
    t_show("no name", SYS(SYS_shm_open, "", O_CREAT | O_RDWR, 0600));
    return 0;
}
