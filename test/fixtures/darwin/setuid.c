// A program with the set-user-ID or set-group-ID bit, run by an ordinary
// user (gexc only: the owner it is given -- root, and a group -- is what
// the test's xattr puts on a file of the host, test/darwin/setuid.test.js).
//
//   setuid exec PATH [ARGS...]
//                           execve PATH with ARGS, or "show"
//   setuid fork PATH        fork, and the child does
//   setuid spawn PATH       posix_spawn PATH with "show", and wait for it
//   setuid show LABEL       say who this is, what that lets it do, and where
//                           it stands on its own file
//   setuid drop LABEL SECRET PLAIN
//                           as `show`, then give up root as su does
//                           (setgid, setgroups, setuid to another user), say
//                           what that leaves, and exec PLAIN with "show"
#include "t.h"

enum {
    SYS_setuid = 23, SYS_getgroups = 79, SYS_setgroups = 80, SYS_setgid = 181, SYS_proc_info = 336,
    SYS_posix_spawn = 244,
};

static void ids(const char *label, const char *when) {
    long pid = SYS(SYS_getpid);
    u8 info[136];
    t_memset(info, 0, sizeof info);
    SYS(SYS_proc_info, 2, pid, 3, 0, info, sizeof info);
    u32 *w = (u32 *)info;
    t_printf("%s %s: ruid %d euid %d svuid %u rgid %d egid %d svgid %u issetugid %ld\n", label, when, (int)SYS(SYS_getuid),
             (int)SYS(SYS_geteuid), w[9], (int)SYS(SYS_getgid), (int)SYS(SYS_getegid), w[10], SYS(SYS_issetugid));
    u32 list[32];
    long n = SYS(SYS_getgroups, 32, list);
    t_printf("%s %s: getgroups = %ld:", label, when, n);
    for (long i = 0; i < n; i++) t_printf(" %u", list[i]);
    t_printf("\n");
}

static void who(const char *label, const char *path) {
    struct t_stat st;
    long r = SYS(SYS_stat64, path, &st);
    t_printf("%s: its file is owned by %u:%u, mode %o (%ld)\n", label, st.st_uid, st.st_gid, st.st_mode & 07777, r);
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)apple;
    if (argc < 3) return 2;
    const char *how = argv[1], *path = argv[2];
    if (!t_strcmp(how, "exec")) {
        char *args[16] = {(char *)path, (char *)"show", (char *)"exec", 0};
        if (argc > 3) {
            int n = 0;
            for (int i = 2; i < argc && n < 15; i++) args[n++] = argv[i];
            args[n] = 0;
        }
        t_show("execve", SYS(SYS_execve, path, args, envp));
        return 1;
    }
    if (!t_strcmp(how, "fork") || !t_strcmp(how, "spawn")) {
        char *args[] = {(char *)path, (char *)"show", (char *)how, 0};
        long pid;
        if (!t_strcmp(how, "spawn")) {
            int p = 0;
            long r = SYS(SYS_posix_spawn, &p, path, 0, args, envp);
            t_show("posix_spawn", r);
            pid = p;
        } else {
            long child_flag = -1;
            pid = t_syscall2(&child_flag, SYS_fork, 0, 0, 0);
            if (pid >= 0 && child_flag) {
                SYS(SYS_execve, path, args, envp);
                t_exit(127);
            }
        }
        int status = -1;
        long got = SYS(SYS_wait4, pid, &status, 0, 0);
        t_printf("wait4: %s, status %x\n", got == pid ? "its pid" : "something else", status);
        return 0;
    }
    if (!t_strcmp(how, "show") || !t_strcmp(how, "drop")) {
        const char *label = path;
        ids(label, "at the start");
        who(label, argv[0]);
        if (!t_strcmp(how, "show")) return 0;
        const char *secret = argv[3], *plain = argv[4];
        t_show("open the secret", SYS(SYS_open, secret, O_RDONLY));
        u32 groups[2] = {20, 80};
        t_show("setgroups(2)", SYS(SYS_setgroups, 2, groups));
        ids(label, "after setgroups");
        t_show("setgid(20)", SYS(SYS_setgid, 20));
        t_show("setgroups(1)", SYS(SYS_setgroups, 1, groups));
        t_show("setuid(502)", SYS(SYS_setuid, 502));
        ids(label, "after giving up root");
        t_show("setuid(0)", SYS(SYS_setuid, 0));
        t_show("setgid(0)", SYS(SYS_setgid, 0));
        t_show("setgroups(1)", SYS(SYS_setgroups, 1, groups));
        t_show("open the secret", SYS(SYS_open, secret, O_RDONLY));
        char *args[] = {(char *)plain, (char *)"show", (char *)"after the drop", 0};
        t_show("execve", SYS(SYS_execve, plain, args, envp));
        return 1;
    }
    return 2;
}
