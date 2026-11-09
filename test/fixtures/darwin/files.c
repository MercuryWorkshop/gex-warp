// Files and directories through the BSD calls, in the directory the test
// is run in: what each call answers, the error numbers Darwin gives, and
// `struct stat` and directory records in Darwin's layouts.
#include "t.h"

static void stat_line(const char *what, long r, struct t_stat *st) {
    if (r < 0) {
        t_printf("%s = %ld\n", what, r);
        return;
    }
    // Not the inode, device, times or block counts: those are the host
    // filesystem's own. Nor a symbolic link's permission bits, which mean
    // nothing, nor a pipe's (0660 and no links on XNU).
    unsigned type = st->st_mode & 0170000;
    if (type == 0120000 || type == 0010000) t_printf("%s: type %o size %ld\n", what, type, (long)st->st_size);
    else t_printf("%s: mode %o nlink %u size %ld\n", what, st->st_mode, st->st_nlink, (long)st->st_size);
}

// The names in a directory, sorted (a filesystem's order is its own).
static void list(const char *dir) {
    static char buf[4096];
    static char names[32][64];
    int n = 0;
    long fd = SYS(SYS_open, dir, O_RDONLY | O_DIRECTORY);
    if (fd < 0) {
        t_printf("open %s = %ld\n", dir, fd);
        return;
    }
    for (;;) {
        long base = -1;
        long got = SYS(SYS_getdirentries64, fd, buf, sizeof buf, &base);
        if (got <= 0) {
            if (got < 0) t_printf("getdirentries64 = %ld\n", got);
            break;
        }
        for (long at = 0; at < got;) {
            // d_ino u64, d_seekoff u64, d_reclen u16, d_namlen u16, d_type u8, d_name.
            u16 reclen = *(u16 *)(buf + at + 16);
            u16 namlen = *(u16 *)(buf + at + 18);
            u8 type = *(u8 *)(buf + at + 20);
            const char *name = buf + at + 21;
            if (reclen == 0) break;
            if (t_strlen(name) != namlen) t_printf("FAIL d_namlen of %s\n", name);
            if (n < 32 && namlen < 62) {
                names[n][0] = (char)('0' + type);
                t_memcpy(names[n] + 1, name, namlen + 1);
                n++;
            }
            at += reclen;
        }
    }
    SYS(SYS_close, fd);
    for (int i = 0; i < n; i++)
        for (int j = i + 1; j < n; j++)
            if (t_strcmp(names[i] + 1, names[j] + 1) > 0) {
                char tmp[64];
                t_memcpy(tmp, names[i], 64), t_memcpy(names[i], names[j], 64), t_memcpy(names[j], tmp, 64);
            }
    t_printf("%s:", dir);
    for (int i = 0; i < n; i++) t_printf(" %s(%c)", names[i] + 1, names[i][0]);
    t_printf("\n");
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    struct t_stat st;
    char buf[128];

    t_check("sizeof(struct stat) is 144", sizeof st == 144);
    long fd = SYS(SYS_open, "a.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    t_check("open O_CREAT", fd >= 3);
    t_show("write", SYS(SYS_write, fd, "hello, file\n", 12));
    t_show("lseek cur", SYS(SYS_lseek, fd, 0, 1));
    t_show("pwrite", SYS(SYS_pwrite, fd, "J", 1, 0));
    stat_line("fstat", SYS(SYS_fstat64, fd, &st), &st);
    t_show("read a write-only descriptor", SYS(SYS_read, fd, buf, 4));
    t_show("close", SYS(SYS_close, fd));
    t_show("close again", SYS(SYS_close, fd));

    t_show("open O_EXCL over it", SYS(SYS_open, "a.txt", O_WRONLY | O_CREAT | O_EXCL, 0644));
    t_show("open a missing file", SYS(SYS_open, "missing", O_RDONLY));
    t_show("open under a file", SYS(SYS_open, "a.txt/x", O_RDONLY));
    fd = SYS(SYS_open, "a.txt", O_RDONLY);
    long n = SYS(SYS_read, fd, buf, sizeof buf - 1);
    buf[n > 0 ? n : 0] = 0;
    t_printf("read %ld: %s", n, buf);
    t_show("read at the end", SYS(SYS_read, fd, buf, 16));
    t_show("pread", SYS(SYS_pread, fd, buf, 5, 7));
    buf[5] = 0;
    t_printf("pread: %s\n", buf);
    t_show("lseek end", SYS(SYS_lseek, fd, -6, 2));
    t_show("lseek before the start", SYS(SYS_lseek, fd, -100, 0));
    t_show("write a read-only descriptor", SYS(SYS_write, fd, "x", 1));
    SYS(SYS_close, fd);

    fd = SYS(SYS_open, "a.txt", O_WRONLY | O_APPEND);
    SYS(SYS_write, fd, "more\n", 5);
    // (XNU also reports a bit of its own for a descriptor written through.)
    t_show("F_GETFL", SYS(SYS_fcntl, fd, F_GETFL, 0) & 0xffff);
    t_show("F_GETFD", SYS(SYS_fcntl, fd, F_GETFD, 0));
    t_show("F_SETFD", SYS(SYS_fcntl, fd, F_SETFD, 1));
    t_show("F_GETFD", SYS(SYS_fcntl, fd, F_GETFD, 0));
    t_show("F_SETFL O_NONBLOCK", SYS(SYS_fcntl, fd, F_SETFL, O_NONBLOCK | O_APPEND));
    t_show("F_GETFL", SYS(SYS_fcntl, fd, F_GETFL, 0) & 0xffff);
    long dupfd = SYS(SYS_fcntl, fd, F_DUPFD, 20);
    t_show("F_DUPFD 20", dupfd);
    t_show("F_DUPFD_CLOEXEC 30", SYS(SYS_fcntl, fd, F_DUPFD_CLOEXEC, 30));
    t_show("its F_GETFD", SYS(SYS_fcntl, 30, F_GETFD, 0));
    t_show("dup2 to 40", SYS(SYS_dup2, fd, 40));
    t_show("dup2 of a bad descriptor", SYS(SYS_dup2, 99, 41));
    t_show("dup", SYS(SYS_dup, fd) > fd);
    t_show("ftruncate", SYS(SYS_ftruncate, fd, 5));
    SYS(SYS_close, fd);
    stat_line("stat", SYS(SYS_stat64, "a.txt", &st), &st);
    stat_line("stat missing", SYS(SYS_stat64, "missing", &st), &st);
    t_show("truncate", SYS(SYS_truncate, "a.txt", 100));
    stat_line("stat", SYS(SYS_stat64, "a.txt", &st), &st);
    t_show("chmod", SYS(SYS_chmod, "a.txt", 0600));
    stat_line("stat", SYS(SYS_stat64, "a.txt", &st), &st);
    t_show("access R", SYS(SYS_access, "a.txt", 4));
    t_show("access X", SYS(SYS_access, "a.txt", 1));
    t_show("access missing", SYS(SYS_access, "missing", 0));

    t_show("mkdir", SYS(SYS_mkdir, "d", 0755));
    t_show("mkdir again", SYS(SYS_mkdir, "d", 0755));
    stat_line("stat d", SYS(SYS_stat64, "d", &st), &st);
    t_show("open a directory to write", SYS(SYS_open, "d", O_WRONLY));
    t_show("rename", SYS(SYS_rename, "a.txt", "d/b.txt"));
    t_show("rename missing", SYS(SYS_rename, "a.txt", "d/c.txt"));
    t_show("link", SYS(SYS_link, "d/b.txt", "d/hard"));
    stat_line("stat d/hard", SYS(SYS_stat64, "d/hard", &st), &st);
    t_show("symlink", SYS(SYS_symlink, "b.txt", "d/soft"));
    t_show("symlink again", SYS(SYS_symlink, "b.txt", "d/soft"));
    n = SYS(SYS_readlink, "d/soft", buf, sizeof buf);
    buf[n > 0 ? n : 0] = 0;
    t_printf("readlink %ld: %s\n", n, buf);
    t_show("readlink of a file", SYS(SYS_readlink, "d/b.txt", buf, sizeof buf));
    stat_line("stat d/soft", SYS(SYS_stat64, "d/soft", &st), &st);
    stat_line("lstat d/soft", SYS(SYS_lstat64, "d/soft", &st), &st);
    stat_line("fstatat nofollow", SYS(SYS_fstatat64, AT_FDCWD, "d/soft", &st, AT_SYMLINK_NOFOLLOW), &st);
    t_show("symlink to nowhere", SYS(SYS_symlink, "nowhere", "d/dangling"));
    t_show("open it", SYS(SYS_open, "d/dangling", O_RDONLY));
    t_show("open O_NOFOLLOW a link", SYS(SYS_open, "d/soft", O_RDONLY | O_NOFOLLOW));
    t_show("mkdir d/sub", SYS(SYS_mkdir, "d/sub", 0700));
    list("d");
    list(".");

    long dfd = SYS(SYS_open, "d", O_RDONLY | O_DIRECTORY);
    fd = SYS(SYS_openat, dfd, "b.txt", O_RDONLY);
    t_check("openat", fd > dfd);
    t_show("F_GETPATH", SYS(SYS_fcntl, fd, F_GETPATH, buf));
    n = (long)t_strlen(buf);
    t_printf("F_GETPATH ends %s\n", n >= 8 ? buf + n - 8 : buf);
    SYS(SYS_close, fd);
    t_show("mkdirat", SYS(SYS_mkdirat, dfd, "sub2", 0755));
    t_show("renameat", SYS(SYS_renameat, dfd, "sub2", AT_FDCWD, "moved"));
    t_show("unlinkat a directory", SYS(SYS_unlinkat, AT_FDCWD, "moved", 0));
    t_show("unlinkat AT_REMOVEDIR", SYS(SYS_unlinkat, AT_FDCWD, "moved", AT_REMOVEDIR));
    t_show("readlinkat", SYS(SYS_readlinkat, dfd, "soft", buf, sizeof buf));
    t_show("faccessat", SYS(SYS_faccessat, dfd, "hard", 4, 0));
    t_show("openat a bad descriptor", SYS(SYS_openat, 99, "b.txt", O_RDONLY));
    t_show("fchdir", SYS(SYS_fchdir, dfd));
    t_show("open b.txt from there", SYS(SYS_open, "b.txt", O_RDONLY) > 0);
    t_show("chdir ..", SYS(SYS_chdir, ".."));
    SYS(SYS_close, dfd);

    t_show("rmdir a full directory", SYS(SYS_rmdir, "d"));
    t_show("rmdir a file", SYS(SYS_rmdir, "d/b.txt"));
    t_show("unlink a directory", SYS(SYS_unlink, "d/sub") < 0);
    t_show("unlink", SYS(SYS_unlink, "d/hard"));
    t_show("unlink", SYS(SYS_unlink, "d/soft"));
    t_show("unlink", SYS(SYS_unlink, "d/dangling"));
    t_show("unlink", SYS(SYS_unlink, "d/b.txt"));
    t_show("unlink again", SYS(SYS_unlink, "d/b.txt"));
    t_show("rmdir", SYS(SYS_rmdir, "d/sub"));
    t_show("rmdir", SYS(SYS_rmdir, "d"));
    t_show("rmdir missing", SYS(SYS_rmdir, "d"));
    char longname[300];
    t_memset(longname, 'x', sizeof longname - 1);
    longname[sizeof longname - 1] = 0;
    t_show("a name too long", SYS(SYS_mkdir, longname, 0755));

    long second = -1;
    long rd = t_syscall2(&second, SYS_pipe, 0, 0, 0);
    t_check("pipe gives two descriptors", rd >= 3 && second > rd);
    t_show("write the pipe", SYS(SYS_write, second, "through", 7));
    n = SYS(SYS_read, rd, buf, sizeof buf);
    buf[n > 0 ? n : 0] = 0;
    t_printf("read the pipe %ld: %s\n", n, buf);
    stat_line("fstat a pipe", SYS(SYS_fstat64, rd, &st), &st);
    t_show("lseek a pipe", SYS(SYS_lseek, rd, 0, 0));
    SYS(SYS_close, second);
    t_show("read a closed pipe", SYS(SYS_read, rd, buf, sizeof buf));
    t_show("umask", SYS(SYS_umask, 027));
    t_show("umask", SYS(SYS_umask, 022));
    t_show("getdtablesize is positive", SYS(SYS_getdtablesize) > 0);
    return 0;
}
