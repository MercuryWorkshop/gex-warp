// A private mapping of a file nothing writes (argv[1]): gex maps the whole
// host pages of one on a read-only mount from the file itself,
// copy-on-write, as XNU maps a library's segments. Whatever backs it, the
// mapping is the file's bytes, a store into it is the mapping's alone (not
// the file's, not another mapping's of the same file, not a forked child's
// or its parent's), and memory mapped over it is zero.
#include "t.h"

#define HP 16384L
#define SIZE (6 * HP + 100)

static long mmap_(void *addr, long len, int prot, int flags, int fd, long off) {
    return SYS(SYS_mmap, addr, len, prot, flags, fd, off);
}

static u8 buf[SIZE];

int main(int argc, char **argv, char **envp, char **apple) {
    (void)envp, (void)apple;
    if (argc < 2) t_exit(2);
    long fd = SYS(SYS_open, argv[1], O_RDONLY);
    t_check("open", fd >= 0);
    t_check("pread it whole", SYS(SYS_pread, fd, buf, SIZE, 0) == SIZE);

    // Room on a host page, and the file mapped there from its start.
    long room = mmap_(0, 10 * HP, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
    t_check("room", room > 0);
    u8 *at = (u8 *)((room + HP - 1) & ~(HP - 1));
    long m = mmap_(at, SIZE, PROT_READ, MAP_PRIVATE | MAP_FIXED, (int)fd, 0);
    t_check("mapped where asked", m == (long)at);
    t_check("its bytes", !t_memcmp(at, buf, SIZE));
    t_check("zero past the file to the page's end", at[SIZE] == 0 && at[SIZE + 3000] == 0);
    // Another mapping of the same file, from its second host page.
    u8 *two = (u8 *)mmap_(0, 2 * HP, PROT_READ, MAP_PRIVATE, (int)fd, HP);
    t_check("a second mapping", (long)two > 0 && !t_memcmp(two, buf + HP, 2 * HP));

    // Stores, made writable: the mapping's own.
    t_show("mprotect to write", SYS(SYS_mprotect, at, 6 * HP, PROT_READ | PROT_WRITE));
    at[5] = 'a', at[HP + 7] = 'b', at[3 * HP - 1] = 'c';
    t_check("the mapping has them", at[5] == 'a' && at[HP + 7] == 'b' && at[3 * HP - 1] == 'c');
    t_check("the rest is the file's", at[6] == buf[6] && at[2 * HP] == buf[2 * HP]);
    u8 c = 0;
    SYS(SYS_pread, fd, &c, 1, HP + 7);
    t_check("not the file's", c == buf[HP + 7]);
    t_check("not the other mapping's", two[7] == buf[HP + 7] && two[2 * HP - 1] == buf[3 * HP - 1]);

    // A child has the parent's bytes, and its stores are its own.
    long child = 0;
    long pid = t_syscall2(&child, SYS_fork, 0, 0, 0);
    if (child) {
        int ok = at[HP + 7] == 'b' && at[2 * HP] == buf[2 * HP];
        at[HP + 8] = 'z', at[4 * HP] = 'y';
        t_exit(ok && at[HP + 8] == 'z' ? 0 : 1);
    }
    int status = -1;
    SYS(SYS_wait4, pid, &status, 0, 0);
    t_check("the child saw the parent's stores", status == 0);
    t_check("and its own stayed its own", at[HP + 8] == buf[HP + 8] && at[4 * HP] == buf[4 * HP]);

    // Memory mapped over it is zero.
    long z = mmap_(at + HP, HP, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0);
    t_check("anonymous memory over it", z == (long)(at + HP));
    int zero = 1;
    for (long i = 0; i < HP; i++) zero &= at[HP + i] == 0;
    t_check("is zero", zero && at[2 * HP] == buf[2 * HP]);
    t_show("munmap", SYS(SYS_munmap, at, 6 * HP));
    t_show("close", SYS(SYS_close, fd));
    return 0;
}
