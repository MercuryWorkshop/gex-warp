// Which macOS the kernel says this is: its names for it (kern.osrelease,
// kern.osversion, kern.osproductversion and the rest; the processor the
// kernel was built for, in kern.version, is the machine's and not
// printed), and the files that say it, each read whole, with whether stat,
// fstat and lseek say its size. Then, given
// `exec PATH`, the same again as PATH (this program) with an environment
// of its own, which chooses no release: what a program is told is its
// own to work out, not its parent's.
#include "t.h"

static long byname(const char *name, void *old, size_t *oldlen) {
    return SYS(SYS_sysctlbyname, name, t_strlen(name), old, oldlen, 0, 0);
}

static void text(const char *who, const char *name) {
    char buf[512];
    size_t len = sizeof buf;
    long r = byname(name, buf, &len);
    if (r < 0) {
        t_printf("%s: %s: error %d\n", who, name, (int)-r);
        return;
    }
    // "RELEASE_ARM64_T6041": the processor is the machine's.
    for (size_t i = 0; buf[i]; i++)
        if (!t_strncmp(buf + i, "RELEASE_ARM64_", 14)) {
            buf[i + 14] = 0;
            break;
        }
    t_printf("%s: %s: %s\n", who, name, buf);
}

static void number(const char *who, const char *name) {
    int v = -1;
    size_t len = sizeof v;
    long r = byname(name, &v, &len);
    if (r < 0) t_printf("%s: %s: error %d\n", who, name, (int)-r);
    else t_printf("%s: %s: %d\n", who, name, v);
}

static void file(const char *who, const char *path) {
    struct t_stat st;
    long s = SYS(SYS_stat64, path, &st);
    long fd = SYS(SYS_open, path, O_RDONLY);
    if (fd < 0) {
        t_printf("%s: %s: error %d (stat %d)\n", who, path, (int)-fd, s < 0 ? (int)-s : 0);
        return;
    }
    static char buf[8192];
    long n = SYS(SYS_read, fd, buf, sizeof buf - 1);
    buf[n > 0 ? n : 0] = 0;
    struct t_stat fst;
    SYS(SYS_fstat64, fd, &fst);
    long end = SYS(SYS_lseek, fd, 0, 2);
    SYS(SYS_close, fd);
    t_printf("%s: %s: %d bytes; stat and fstat %s it, lseek to its end %s\n%s", who, path, (int)n,
             s == 0 && st.st_size == n && fst.st_size == n ? "size" : "missize", end == n ? "agrees" : "does not", buf);
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)envp, (void)apple;
    const char *who = argc > 3 && !t_strcmp(argv[1], "-") ? argv[2] : "first";
    const char *chosen = t_getenv(envp, "GEX_MACOS_VERSION");
    t_printf("%s: told %s\n", who, chosen ? chosen : "nothing");
    const char *names[] = {"kern.ostype", "kern.osrelease", "kern.osversion", "kern.osproductversion", "kern.osproductversioncompat", "kern.iossupportversion", "kern.version"};
    for (size_t i = 0; i < sizeof names / sizeof *names; i++) text(who, names[i]);
    number(who, "kern.osrevision");
    number(who, "sysctl.proc_translated");
    file(who, "/System/Library/CoreServices/SystemVersion.plist");
    file(who, "/System/Library/CoreServices/SystemVersionCompat.plist");
    file(who, "/System/Library/CoreServices/iOSSystemVersion.plist");
    if (argc > 2 && !t_strcmp(argv[1], "exec")) {
        char *args[] = {argv[2], "-", "second", "x", 0};
        char *env[] = {"T_VAR=yes", 0};
        long r = SYS(SYS_execve, argv[2], args, env);
        t_printf("execve: error %d\n", (int)-r);
        return 1;
    }
    return 0;
}
