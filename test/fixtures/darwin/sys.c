// What a process asks about itself and its machine: sysctl by number and
// by name, csops, proc_info, statfs and pathconf; then waitid, sigwait and
// the last step of abort. Only what every Mac answers alike is printed.
#include "t.h"

enum {
    SYS_csops = 169, SYS_waitid = 173, SYS_pathconf = 191, SYS_fpathconf = 192, SYS_gettid = 286,
    SYS___sigwait = 330, SYS___pthread_canceled = 333, SYS_proc_info = 336, SYS_statfs64 = 345,
    SYS_fstatfs64 = 346, SYS_getfsstat64 = 347, SYS_abort_with_payload = 521,
};
enum { CTL_KERN = 1, CTL_HW = 6 };
enum { KERN_OSTYPE = 1, KERN_OSRELEASE = 2, KERN_ARGMAX = 8, KERN_PROC = 14, KERN_PROCARGS2 = 49, KERN_PROC_PID = 1 };
enum { HW_MACHINE = 1, HW_NCPU = 3, HW_BYTEORDER = 4, HW_PAGESIZE = 7, HW_MEMSIZE = 24 };

struct t_statfs {
    u32 f_bsize;
    i32 f_iosize;
    u64 f_blocks, f_bfree, f_bavail, f_files, f_ffree;
    i32 f_fsid[2];
    u32 f_owner, f_type, f_flags, f_fssubtype;
    char f_fstypename[16], f_mntonname[1024], f_mntfromname[1024];
    u32 f_flags_ext, f_reserved[7];
};

static long sysctl(int *mib, u32 n, void *old, size_t *oldlen, void *new, size_t newlen) {
    return SYS(SYS_sysctl, mib, n, old, oldlen, new, newlen);
}

static long byname(const char *name, void *old, size_t *oldlen) {
    return SYS(SYS_sysctlbyname, name, t_strlen(name), old, oldlen, 0, 0);
}

static long fork_(void) {
    long child_flag = -1;
    long pid = t_syscall2(&child_flag, SYS_fork, 0, 0, 0);
    if (pid < 0) return pid;
    return child_flag ? 0 : pid;
}

static void by_number(void) {
    char buf[64];
    size_t len = sizeof buf;
    int mib[12] = {CTL_KERN, KERN_OSTYPE};
    t_memset(buf, 'x', sizeof buf);
    t_show("sysctl kern.ostype", sysctl(mib, 2, buf, &len, 0, 0));
    t_printf("  \"%s\", %zu bytes\n", buf, len);
    len = 0;
    t_show("its length alone", sysctl(mib, 2, 0, &len, 0, 0));
    t_printf("  %zu bytes\n", len);
    len = 3;
    t_memset(buf, 'x', sizeof buf);
    t_show("into three bytes", sysctl(mib, 2, buf, &len, 0, 0));
    t_printf("  \"%c%c%c%c\", length %zu\n", buf[0], buf[1], buf[2], buf[3], len);
    t_show("set it", sysctl(mib, 2, 0, 0, "Linux", 6));

    mib[0] = CTL_HW, mib[1] = HW_PAGESIZE;
    int i = -1;
    len = sizeof i;
    t_show("sysctl hw.pagesize as an int", sysctl(mib, 2, &i, &len, 0, 0));
    t_printf("  %d, %zu bytes\n", i, len);
    i64 q = -1;
    len = sizeof q;
    t_show("as a long long", sysctl(mib, 2, &q, &len, 0, 0));
    t_printf("  %lld, %zu bytes\n", q, len);
    mib[1] = HW_BYTEORDER, len = sizeof i;
    t_show("sysctl hw.byteorder", sysctl(mib, 2, &i, &len, 0, 0));
    t_printf("  %d\n", i);
    mib[1] = HW_NCPU, len = sizeof i, i = 0;
    t_show("sysctl hw.ncpu", sysctl(mib, 2, &i, &len, 0, 0));
    t_check("a processor at least", i >= 1 && len == 4);
    mib[1] = HW_MEMSIZE, len = sizeof q, q = 0;
    t_show("sysctl hw.memsize", sysctl(mib, 2, &q, &len, 0, 0));
    t_check("some memory", q >= (1ll << 30) && len == 8);
    mib[1] = HW_MACHINE, len = sizeof buf;
    t_show("sysctl hw.machine", sysctl(mib, 2, buf, &len, 0, 0));
    t_printf("  \"%s\"\n", buf);
    mib[0] = CTL_KERN, mib[1] = KERN_OSRELEASE, len = sizeof buf;
    t_show("sysctl kern.osrelease", sysctl(mib, 2, buf, &len, 0, 0));
    t_check("a version with a dot in it", len > 3 && len == t_strlen(buf) + 1 && buf[2] == '.');
    mib[1] = KERN_ARGMAX, len = sizeof i;
    t_show("sysctl kern.argmax", sysctl(mib, 2, &i, &len, 0, 0));
    t_printf("  %d\n", i);

    mib[0] = CTL_HW, mib[1] = 9999, len = sizeof i;
    t_show("a number nothing has", sysctl(mib, 2, &i, &len, 0, 0));
    mib[1] = HW_NCPU, mib[2] = 5;
    t_show("a name under a value", sysctl(mib, 3, &i, &len, 0, 0));
    t_show("one number", sysctl(mib, 1, &i, &len, 0, 0));
    t_show("thirteen", sysctl(mib, 13, &i, &len, 0, 0));

    // sysctl.name2oid.
    int oid[12], ask[2] = {0, 3};
    len = sizeof oid;
    t_show("name2oid kern.ostype", sysctl(ask, 2, oid, &len, "kern.ostype", 11));
    t_printf("  %zu bytes: %d.%d\n", len, oid[0], oid[1]);
    len = sizeof oid;
    // The name is not the old number: it has one of its own, for a long
    // long where HW_PAGESIZE is an int.
    t_show("name2oid hw.pagesize", sysctl(ask, 2, oid, &len, "hw.pagesize", 11));
    t_printf("  %zu bytes: %d.%s\n", len, oid[0], oid[1] == HW_PAGESIZE ? "HW_PAGESIZE" : "another");
    q = -1, len = sizeof q;
    t_show("and by that number, a long long", sysctl(oid, 2, &q, &len, 0, 0));
    t_printf("  %lld, %zu bytes\n", q, len);
    len = sizeof oid;
    t_show("name2oid of no name", sysctl(ask, 2, oid, &len, "no.such.name", 12));
    // A name with no number of its own gets one that names it.
    len = sizeof oid;
    t_show("name2oid hw.cputype", sysctl(ask, 2, oid, &len, "hw.cputype", 10));
    size_t n = len / 4;
    len = sizeof i, i = 0;
    t_show("and by that number", sysctl(oid, (u32)n, &i, &len, 0, 0));
    t_printf("  %x\n", i);
}

static void by_name(void) {
    char buf[64];
    size_t len = sizeof buf;
    t_show("sysctlbyname kern.ostype", byname("kern.ostype", buf, &len));
    t_printf("  \"%s\", %zu bytes\n", buf, len);
    int i = -1;
    len = sizeof i;
    t_show("hw.pagesize", byname("hw.pagesize", &i, &len));
    t_printf("  %d\n", i);
    len = sizeof i;
    t_show("hw.cputype", byname("hw.cputype", &i, &len));
    t_printf("  %x\n", i);
    len = sizeof i;
    t_show("hw.cpu64bit_capable", byname("hw.cpu64bit_capable", &i, &len));
    t_printf("  %d\n", i);
    len = sizeof i;
    t_show("hw.optional.floatingpoint", byname("hw.optional.floatingpoint", &i, &len));
    t_printf("  %d\n", i);
    len = sizeof i, i = -1;
    t_show("hw.optional.arm64", byname("hw.optional.arm64", &i, &len));
    t_printf("  %d\n", i);
    len = sizeof i, i = -1;
    t_show("sysctl.proc_translated", byname("sysctl.proc_translated", &i, &len));
    t_printf("  %d\n", i);
    len = sizeof i, i = 0;
    t_show("hw.logicalcpu", byname("hw.logicalcpu", &i, &len));
    t_check("a processor at least", i >= 1);
    i64 q = 0;
    len = sizeof q;
    t_show("hw.tbfrequency", byname("hw.tbfrequency", &q, &len));
    t_check("a frequency", q > 0 && len == 8);
    len = sizeof buf;
    t_show("kern.osversion", byname("kern.osversion", buf, &len));
    t_check("a build", len >= 5 && len == t_strlen(buf) + 1);
    len = sizeof buf;
    t_show("kern.osproductversion", byname("kern.osproductversion", buf, &len));
    t_check("a release", len >= 4 && len == t_strlen(buf) + 1);
    len = sizeof buf;
    t_show("no.such.name", byname("no.such.name", buf, &len));
    len = sizeof buf;
    t_show("hw.optional, which is no value", byname("hw.optional", buf, &len));
    struct t_timeval boot = {0, 0}, now;
    len = sizeof boot;
    t_show("kern.boottime", byname("kern.boottime", &boot, &len));
    SYS(SYS_gettimeofday, &now, 0, 0);
    t_check("in the past", len == 16 && boot.tv_sec > 0 && boot.tv_sec <= now.tv_sec);
}

static void process(int argc, char **argv) {
    long me = SYS(SYS_getpid);
    // struct kinfo_proc: p_flag at 32, p_pid at 40; e_ppid at 296 + 264.
    static u8 kp[648 * 2];
    size_t len = sizeof kp;
    int mib[4] = {CTL_KERN, KERN_PROC, KERN_PROC_PID, (int)me};
    t_show("sysctl kern.proc.pid", sysctl(mib, 4, kp, &len, 0, 0));
    t_printf("  %zu bytes\n", len);
    t_check("it is this process", *(i32 *)(kp + 40) == me);
    t_check("of its parent", *(i32 *)(kp + 296 + 264) == SYS(SYS_getppid));
    t_check("and 64-bit", *(u32 *)(kp + 32) & 4);
    t_check("with its user", *(u32 *)(kp + 296 + 120 + 4) == (u32)SYS(SYS_geteuid));
    len = 0;
    t_show("its size alone", sysctl(mib, 4, 0, &len, 0, 0));
    t_check("a kinfo_proc or more", len >= 648);

    static char args[16384];
    len = sizeof args;
    mib[1] = KERN_PROCARGS2, mib[2] = (int)me;
    t_show("sysctl kern.procargs2", sysctl(mib, 3, args, &len, 0, 0));
    t_check("the argument count", *(int *)args == argc);
    // The executable's path, NULs to a word, then the arguments.
    char *p = args + 4;
    size_t plen = t_strlen(p), alen = t_strlen(argv[0]);
    t_check("the path ends as argv[0] does", plen >= alen || alen > 0);
    p += plen;
    while (p < args + len && !*p) p++;
    t_check("then argv[0]", !t_strcmp(p, argv[0]));
    p += t_strlen(p) + 1;
    if (argc > 1) t_printf("  then \"%s\"\n", p);

    u32 flags = 0;
    t_show("csops status", SYS(SYS_csops, me, 0, &flags, sizeof flags));
    t_check("signed and valid", flags & 1);
    t_show("csops of nobody", SYS(SYS_csops, 0x3fffffff, 0, &flags, sizeof flags));
    t_show("csops of an operation there is not", SYS(SYS_csops, me, 999, &flags, sizeof flags));

    static char path[4096];
    t_show("proc_info path", SYS(SYS_proc_info, 2, me, 11, 0, path, sizeof path));
    plen = t_strlen(path);
    t_check("an absolute one", path[0] == '/' && plen > 1);
    static u8 bi[136];
    t_show("proc_info bsdinfo", SYS(SYS_proc_info, 2, me, 3, 0, bi, sizeof bi));
    t_check("pid, parent, user", *(u32 *)(bi + 12) == me && *(u32 *)(bi + 16) == SYS(SYS_getppid) && *(u32 *)(bi + 20) == (u32)SYS(SYS_geteuid));
    static u8 si[64];
    t_show("proc_info short bsdinfo", SYS(SYS_proc_info, 2, me, 13, 0, si, sizeof si));
    t_check("pid, parent, group", *(u32 *)si == me && *(u32 *)(si + 4) == SYS(SYS_getppid) && *(u32 *)(si + 8) == (u32)SYS(SYS_getpgrp));
    t_show("bsdinfo into too little", SYS(SYS_proc_info, 2, me, 3, 0, bi, 100));
    t_show("proc_info of nobody", SYS(SYS_proc_info, 2, 0x3fffffff, 3, 0, bi, sizeof bi));
    t_show("proc_info of a flavour there is not", SYS(SYS_proc_info, 2, me, 9999, 0, bi, sizeof bi));
    t_show("gettid", SYS(SYS_gettid, &flags, &flags));
    t_show("__pthread_canceled(0)", SYS(SYS___pthread_canceled, 0));
}

static void filesystem(void) {
    static struct t_statfs fs;
    t_show("statfs64 /", SYS(SYS_statfs64, "/", &fs));
    t_printf("  %s on %s\n", fs.f_fstypename, fs.f_mntonname);
    t_check("blocks of some size", fs.f_bsize >= 512 && fs.f_blocks > 0 && fs.f_bfree <= fs.f_blocks);
    t_check("a device", fs.f_mntfromname[0] == '/');
    t_show("statfs64 of nothing", SYS(SYS_statfs64, "/no/such/place", &fs));
    long fd = SYS(SYS_open, "/", O_RDONLY);
    t_memset(&fs, 0, sizeof fs);
    t_show("fstatfs64", SYS(SYS_fstatfs64, fd, &fs));
    t_printf("  %s\n", fs.f_fstypename);
    t_show("fstatfs64 of no descriptor", SYS(SYS_fstatfs64, 99, &fs));
    t_check("getfsstat64 counts one or more", SYS(SYS_getfsstat64, 0, 0, 2) >= 1);
    t_memset(&fs, 0, sizeof fs);
    t_check("and fills one in", SYS(SYS_getfsstat64, &fs, sizeof fs, 2) == 1 && fs.f_mntonname[0] == '/' && fs.f_bsize >= 512);
    t_show("pathconf name max", SYS(SYS_pathconf, "/", 4));
    t_show("pathconf path max", SYS(SYS_pathconf, "/", 5));
    t_show("pathconf pipe buf", SYS(SYS_pathconf, "/", 6));
    t_show("fpathconf name max", SYS(SYS_fpathconf, fd, 4));
    t_show("fpathconf of nothing known", SYS(SYS_fpathconf, fd, 999));
    t_show("fpathconf of no descriptor", SYS(SYS_fpathconf, 99, 4));
    SYS(SYS_close, fd);
}

// Extended attributes, with Darwin's extra arguments (a position, and
// options), and renameatx_np's two flags.
static void attributes(void) {
    enum { SYS_getxattr = 234, SYS_setxattr = 236, SYS_removexattr = 238, SYS_listxattr = 240, SYS_renameatx_np = 488 };
    enum { XATTR_CREATE = 2, XATTR_REPLACE = 4, RENAME_EXCL = 4 };
    long fd = SYS(SYS_open, "x", O_CREAT | O_WRONLY, 0644);
    SYS(SYS_write, fd, "ex", 2);
    SYS(SYS_close, fd);
    char buf[32];
    t_show("setxattr", SYS(SYS_setxattr, "x", "user.t", "value", 5, 0, 0));
    t_memset(buf, 0, sizeof buf);
    t_show("getxattr", SYS(SYS_getxattr, "x", "user.t", buf, sizeof buf, 0, 0));
    t_printf("  \"%s\"\n", buf);
    t_show("its size alone", SYS(SYS_getxattr, "x", "user.t", 0, 0, 0, 0));
    t_show("getxattr of none", SYS(SYS_getxattr, "x", "user.none", buf, sizeof buf, 0, 0));
    t_show("setxattr, to create what is there", SYS(SYS_setxattr, "x", "user.t", "v", 1, 0, XATTR_CREATE));
    t_show("setxattr, to replace what is not", SYS(SYS_setxattr, "x", "user.none", "v", 1, 0, XATTR_REPLACE));
    t_memset(buf, 0, sizeof buf);
    t_show("listxattr", SYS(SYS_listxattr, "x", buf, sizeof buf, 0));
    t_printf("  \"%s\"\n", buf);
    t_show("removexattr", SYS(SYS_removexattr, "x", "user.t", 0));
    t_show("and again", SYS(SYS_removexattr, "x", "user.t", 0));
    t_show("getxattr of no file", SYS(SYS_getxattr, "nothing", "user.t", buf, sizeof buf, 0, 0));

    fd = SYS(SYS_open, "y", O_CREAT | O_WRONLY, 0644);
    SYS(SYS_write, fd, "why", 3);
    SYS(SYS_close, fd);
    t_show("renameatx_np onto a file, exclusively", SYS(SYS_renameatx_np, AT_FDCWD, "x", AT_FDCWD, "y", RENAME_EXCL));
    t_show("and onto none", SYS(SYS_renameatx_np, AT_FDCWD, "x", AT_FDCWD, "z", RENAME_EXCL));
    struct t_stat st;
    t_show("the old name", SYS(SYS_stat64, "x", &st));
    SYS(SYS_stat64, "z", &st);
    t_printf("  the new one is %lld bytes\n", st.st_size);
    // (RENAME_SWAP is the volume's to offer: APFS does, gex's do not.)
    SYS(SYS_unlink, "z"), SYS(SYS_unlink, "y");
}

static void children(void) {
    static struct t_siginfo info;
    static u64 guard[4];
    long pid = fork_();
    if (pid == 0) t_exit(7);
    // P_PID, WEXITED.
    t_memset(&info, 0x55, sizeof info);
    guard[0] = 0x1122334455667788ull;
    t_show("waitid a child that exits", SYS(SYS_waitid, 1, pid, &info, 4));
    t_printf("  signal %d, code %d, status %d, %s\n", info.si_signo, info.si_code, info.si_status, info.si_pid == pid ? "its pid" : "another's");
    t_check("nothing past it was touched", guard[0] == 0x1122334455667788ull);
    pid = fork_();
    if (pid == 0) {
        long self = SYS(SYS_getpid);
        SYS(SYS_kill, self, SIGTERM);
        t_exit(1);
    }
    t_show("waitid one that is killed", SYS(SYS_waitid, 1, pid, &info, 4));
    t_printf("  signal %d, code %d, status %d\n", info.si_signo, info.si_code, info.si_status);
    t_show("waitid with nobody left", SYS(SYS_waitid, 0, 0, &info, 4));
    // WNOHANG | WEXITED on one still running.
    long wr = -1;
    long rd = t_syscall2(&wr, SYS_pipe, 0, 0, 0);
    pid = fork_();
    if (pid == 0) {
        char c;
        SYS(SYS_read, rd, &c, 1);
        t_exit(0);
    }
    t_memset(&info, 0x55, sizeof info);
    t_show("waitid without waiting", SYS(SYS_waitid, 1, pid, &info, 5));
    t_printf("  signal %d, pid %d\n", info.si_signo, info.si_pid);
    SYS(SYS_write, wr, "x", 1);
    int status = 0;
    SYS(SYS_wait4, pid, &status, 0, 0);

    pid = fork_();
    if (pid == 0) {
        SYS(SYS_abort_with_payload, 0, 0, 0, 0, "gone", 0);
        t_exit(1);
    }
    status = 0;
    SYS(SYS_wait4, pid, &status, 0, 0);
    t_printf("abort_with_payload: status %x\n", status & 0x7f);

    u32 set = T_SIGBIT(SIGUSR1) | T_SIGBIT(SIGUSR2), old = 0;
    SYS(SYS_sigprocmask, SIG_BLOCK, &set, &old);
    long self = SYS(SYS_getpid);
    SYS(SYS_kill, self, SIGUSR2);
    int sig = 0;
    t_show("__sigwait", SYS(SYS___sigwait, &set, &sig));
    t_printf("  signal %d\n", sig);
    SYS(SYS_sigprocmask, SIG_SETMASK, &old, 0);
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)envp, (void)apple;
    by_number();
    by_name();
    process(argc, argv);
    filesystem();
    attributes();
    children();
    return 0;
}
