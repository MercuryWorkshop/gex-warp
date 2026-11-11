// What a Mac program is told is its own path, each way it can ask: the
// `apple` vector's executable_path (dyld's _NSGetExecutablePath, and so a
// main bundle), proc_pidpath, and KERN_PROCARGS2's exec path. argv[0] is
// apart from them: a program exec'd by one path and named another is told
// both. `exepath exec PATH NAME` becomes PATH again, named NAME.
#include "t.h"

enum { SYS_proc_info = 336, PROC_INFO_CALL_PIDINFO = 2, PROC_PIDPATHINFO = 11 };

static char buf[65536];

static void report(const char *who, char **argv, char **apple) {
    t_printf("%s: argv[0] %s\n", who, argv[0]);
    const char *e = t_getenv(apple, "executable_path");
    t_printf("%s: executable_path %s\n", who, e ? e : "(none)");
    long pid = SYS(SYS_getpid);
    t_memset(buf, 0, sizeof buf);
    long n = SYS(SYS_proc_info, PROC_INFO_CALL_PIDINFO, pid, PROC_PIDPATHINFO, 0, buf, 4096);
    if (n < 0) t_printf("%s: proc_pidpath error %ld\n", who, -n);
    else t_printf("%s: proc_pidpath %s\n", who, buf);
    int mib[3] = {1, 49, (int)pid}; // CTL_KERN, KERN_PROCARGS2
    size_t len = sizeof buf;
    t_memset(buf, 0, sizeof buf);
    long r = SYS(SYS_sysctl, mib, 3, buf, &len, 0, 0);
    if (r < 0) {
        t_printf("%s: procargs2 error %ld\n", who, -r);
        return;
    }
    int argc = *(int *)buf;
    const char *path = buf + sizeof(int);
    // The exec path, then NULs to the first argument.
    const char *p = path + t_strlen(path);
    while (p < buf + len && !*p) p++;
    t_printf("%s: procargs2 %s, argc %d, argv[0] %s\n", who, path, argc, p < buf + len ? p : "(none)");
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)envp;
    if (argc >= 4 && !t_strcmp(argv[1], "exec")) {
        report("first", argv, apple);
        char *args[] = {argv[3], "again", 0};
        char *env[] = {0};
        long r = SYS(SYS_execve, argv[2], args, env);
        t_printf("FAIL execve %s: %ld\n", argv[2], r);
        return 1;
    }
    report(argc >= 2 && !t_strcmp(argv[1], "again") ? "second" : "first", argv, apple);
    return 0;
}
