// The calls from before 64-bit inodes, which a program built for an old
// system makes: stat, fstat, lstat and fstatat with the first `struct
// stat`, getdirentries with the first `struct dirent`, statfs, fstatfs and
// getfsstat with the first `struct statfs`. The kernel still answers them.
#include "t.h"

enum {
    SYS_getfsstat = 18, SYS_statfs = 157, SYS_fstatfs = 158, SYS_stat = 188, SYS_fstat = 189, SYS_lstat = 190,
    SYS_getdirentries = 196, SYS_fstatat = 469,
};

// `struct stat` as it was: a 32-bit inode number, and no birth time.
struct t_ostat {
    i32 st_dev;
    u32 st_ino;
    u16 st_mode, st_nlink;
    u32 st_uid, st_gid;
    i32 st_rdev;
    struct t_timespec st_atim, st_mtim, st_ctim;
    i64 st_size, st_blocks;
    i32 st_blksize;
    u32 st_flags, st_gen;
    i32 st_lspare;
    i64 st_qspare[2];
};
// `struct statfs` as it was: short names, and counts that are longs.
struct t_ostatfs {
    short f_otype, f_oflags;
    long f_bsize, f_iosize, f_blocks, f_bfree, f_bavail, f_files, f_ffree;
    i32 f_fsid[2];
    u32 f_owner;
    short f_reserved1, f_type;
    long f_flags, f_reserved2[2];
    char f_fstypename[15], f_mntonname[90], f_mntfromname[90], f_reserved3;
    long f_reserved4[4];
};
// `struct dirent` as it was: a 32-bit inode number and a one-byte length.
struct t_odirent {
    u32 d_ino;
    u16 d_reclen;
    u8 d_type, d_namlen;
    char d_name[256];
};

#define GUARD 0x1122334455667788ull

static void stats(void) {
    long fd = SYS(SYS_open, "o", O_CREAT | O_RDWR, 0640);
    SYS(SYS_write, fd, "hello", 5);
    SYS(SYS_symlink, "o", "l");
    static struct {
        struct t_ostat st;
        u64 guard;
    } b;
    static struct t_stat now;
    t_printf("the structure is %zu bytes\n", sizeof b.st);
    t_memset(&b.st, 0x55, sizeof b.st);
    b.guard = GUARD;
    t_show("stat", SYS(SYS_stat, "o", &b.st));
    SYS(SYS_stat64, "o", &now);
    t_printf("  mode %o, %d link, %lld bytes\n", b.st.st_mode, b.st.st_nlink, b.st.st_size);
    t_check("nothing past it was written", b.guard == GUARD);
    t_check("the inode number's low half", b.st.st_ino != 0 && b.st.st_ino == (u32)now.st_ino);
    t_check("the device, owner and group", b.st.st_dev == now.st_dev && b.st.st_uid == now.st_uid && b.st.st_gid == now.st_gid);
    t_check("the times", b.st.st_mtim.tv_sec == now.st_mtim.tv_sec && b.st.st_mtim.tv_nsec == now.st_mtim.tv_nsec &&
                             b.st.st_ctim.tv_sec == now.st_ctim.tv_sec && b.st.st_atim.tv_sec == now.st_atim.tv_sec);
    t_check("the blocks", b.st.st_blocks == now.st_blocks && b.st.st_blksize == now.st_blksize);
    t_memset(&b.st, 0x55, sizeof b.st);
    t_show("fstat", SYS(SYS_fstat, fd, &b.st));
    t_printf("  mode %o, %lld bytes\n", b.st.st_mode, b.st.st_size);
    t_show("lstat a link", SYS(SYS_lstat, "l", &b.st));
    t_printf("  mode %o\n", b.st.st_mode & 0170000);
    t_show("stat through it", SYS(SYS_stat, "l", &b.st));
    t_printf("  mode %o\n", b.st.st_mode & 0170000);
    t_show("fstatat", SYS(SYS_fstatat, AT_FDCWD, "o", &b.st, 0));
    t_printf("  mode %o, %lld bytes\n", b.st.st_mode, b.st.st_size);
    t_show("fstatat a link, not following", SYS(SYS_fstatat, AT_FDCWD, "l", &b.st, AT_SYMLINK_NOFOLLOW));
    t_printf("  mode %o\n", b.st.st_mode & 0170000);
    t_check("nothing past it was written", b.guard == GUARD);
    t_show("stat of nothing", SYS(SYS_stat, "nothing", &b.st));
    t_show("fstat of no descriptor", SYS(SYS_fstat, 99, &b.st));
    SYS(SYS_close, fd);
    SYS(SYS_unlink, "o"), SYS(SYS_unlink, "l");
}

static void touch(const char *path) {
    long fd = SYS(SYS_open, path, O_CREAT | O_WRONLY, 0644);
    SYS(SYS_close, fd);
}

static void directory(void) {
    SYS(SYS_mkdir, "d", 0755);
    touch("d/a");
    touch("d/bb");
    SYS(SYS_mkdir, "d/sub", 0755);
    long fd = SYS(SYS_open, "d", O_RDONLY);
    static char buf[2048];
    long base = -1;
    long n = SYS(SYS_getdirentries, fd, buf, sizeof buf, &base);
    t_check("getdirentries", n > 0);
    t_printf("  it began at %ld\n", base);
    // The entries, in whatever order: each is checked, and named in ours.
    static const char *want[] = {".", "..", "a", "bb", "sub"};
    int seen[5] = {0, 0, 0, 0, 0}, others = 0, ok = 1;
    for (long at = 0; at < n;) {
        struct t_odirent *d = (struct t_odirent *)(buf + at);
        if (d->d_reclen == 0 || d->d_reclen % 4 || at + d->d_reclen > n) {
            ok = 0;
            break;
        }
        if (d->d_namlen != t_strlen(d->d_name) || d->d_reclen < 8 + d->d_namlen + 1) ok = 0;
        int found = 0;
        for (int i = 0; i < 5; i++) {
            if (!t_strcmp(d->d_name, want[i])) {
                seen[i] = d->d_type, found = 1;
                if (d->d_ino == 0) ok = 0;
            }
        }
        // (A removed entry is a record with no inode number.)
        if (!found && d->d_ino != 0) others++;
        at += d->d_reclen;
    }
    t_check("records that add up", ok);
    for (int i = 0; i < 5; i++) t_printf("  %s: type %d\n", want[i], seen[i]);
    t_printf("  and %d others\n", others);
    t_show("and at its end", SYS(SYS_getdirentries, fd, buf, sizeof buf, &base));
    t_show("into too little", ({ SYS(SYS_lseek, fd, 0, 0); SYS(SYS_getdirentries, fd, buf, 8, &base); }));
    t_show("of a file", ({ long f = SYS(SYS_open, "d/a", O_RDONLY); SYS(SYS_getdirentries, f, buf, sizeof buf, &base); }));
    SYS(SYS_close, fd);
    SYS(SYS_unlink, "d/a"), SYS(SYS_unlink, "d/bb"), SYS(SYS_rmdir, "d/sub"), SYS(SYS_rmdir, "d");
}

static void filesystems(void) {
    static struct {
        struct t_ostatfs fs;
        u64 guard;
    } b;
    t_printf("the structure is %zu bytes\n", sizeof b.fs);
    t_memset(&b.fs, 0x55, sizeof b.fs);
    b.guard = GUARD;
    t_show("statfs /", SYS(SYS_statfs, "/", &b.fs));
    t_printf("  %s on %s\n", b.fs.f_fstypename, b.fs.f_mntonname);
    t_check("blocks of some size", b.fs.f_bsize >= 512 && b.fs.f_blocks > 0 && b.fs.f_bfree <= b.fs.f_blocks);
    t_check("a device", b.fs.f_mntfromname[0] == '/');
    t_check("nothing past it was written", b.guard == GUARD);
    t_show("statfs of nothing", SYS(SYS_statfs, "/no/such/place", &b.fs));
    long fd = SYS(SYS_open, "/", O_RDONLY);
    t_memset(&b.fs, 0, sizeof b.fs);
    t_show("fstatfs", SYS(SYS_fstatfs, fd, &b.fs));
    t_printf("  %s\n", b.fs.f_fstypename);
    t_show("fstatfs of no descriptor", SYS(SYS_fstatfs, 99, &b.fs));
    t_check("getfsstat counts one or more", SYS(SYS_getfsstat, 0, 0, 2) >= 1);
    t_memset(&b.fs, 0, sizeof b.fs);
    t_check("and fills one in", SYS(SYS_getfsstat, &b.fs, sizeof b.fs, 2) == 1 && b.fs.f_mntonname[0] == '/' && b.fs.f_bsize >= 512);
    t_check("nothing past it was written", b.guard == GUARD);
    SYS(SYS_close, fd);
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    stats();
    directory();
    filesystems();
    return 0;
}
