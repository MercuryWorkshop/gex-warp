// Who a process is, and what that lets it do to its own files, for a user
// who is not root: the ids and the groups beside them (getgroups, as the
// kernel lists them: the effective group first), the refusals an
// unprivileged process is given (setgroups, setuid, setgid and their kin,
// setlogin), the ids it may set to its own, issetugid, the saved ids as
// proc_info tells them, and chown and chmod on a file of its own: to a group
// it is in, and not to another, with the set-user-ID bit gone from a file
// whose group is given. Run on this Mac and under gexc: the same lines.
#include "t.h"

enum {
    SYS_chown = 16, SYS_setuid = 23, SYS_setlogin = 50, SYS_getgroups = 79, SYS_setgroups = 80, SYS_setreuid = 126,
    SYS_setregid = 127, SYS_fchown = 123, SYS_setgid = 181, SYS_setegid = 182, SYS_seteuid = 183, SYS_proc_info = 336,
};
#define NO ((long)0xffffffffu)

static void ids(const char *when) {
    long pid = SYS(SYS_getpid);
    u8 info[136];
    t_memset(info, 0, sizeof info);
    long n = SYS(SYS_proc_info, 2, pid, 3, 0, info, sizeof info);
    u32 *w = (u32 *)info;
    // pbi_uid, pbi_gid, pbi_ruid, pbi_rgid, pbi_svuid, pbi_svgid.
    t_printf("%s: uid %d euid %d gid %d egid %d; proc_info %ld: uid %u gid %u ruid %u rgid %u svuid %u svgid %u; issetugid %ld\n", when,
             (int)SYS(SYS_getuid), (int)SYS(SYS_geteuid), (int)SYS(SYS_getgid), (int)SYS(SYS_getegid), n, w[5], w[6], w[7], w[8], w[9], w[10],
             SYS(SYS_issetugid));
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    long uid = SYS(SYS_getuid), gid = SYS(SYS_getgid), egid = SYS(SYS_getegid);
    t_check("not the superuser", uid != 0 && SYS(SYS_geteuid) == uid);
    ids("at the start");

    // The groups: how many, then each, the effective group first.
    long count = SYS(SYS_getgroups, 0, 0);
    t_show("getgroups(0)", count > 0 ? 1 : count);
    u32 list[32];
    long got = SYS(SYS_getgroups, 32, list);
    t_printf("getgroups(32) = %ld:", got);
    for (long i = 0; i < got; i++) t_printf(" %u", list[i]);
    t_printf("\n");
    t_check("the count is the list's length", got == count);
    t_check("the effective group is first", got > 0 && (long)list[0] == egid);
    if (count > 1) t_show("getgroups(1)", SYS(SYS_getgroups, 1, list));
    t_show("getgroups(-1)", SYS(SYS_getgroups, -1, list));

    // What an unprivileged process is refused.
    u32 some[2] = {(u32)egid, 80};
    t_show("setgroups(2)", SYS(SYS_setgroups, 2, some));
    t_show("setgroups(0)", SYS(SYS_setgroups, 0, 0));
    t_show("setgroups(17)", SYS(SYS_setgroups, 17, some));
    t_show("setuid(0)", SYS(SYS_setuid, 0));
    t_show("setuid(other)", SYS(SYS_setuid, uid + 1));
    t_show("seteuid(0)", SYS(SYS_seteuid, 0));
    t_show("setreuid(0, 0)", SYS(SYS_setreuid, 0, 0));
    t_show("setreuid(-1, 0)", SYS(SYS_setreuid, NO, 0));
    t_show("setgid(0)", SYS(SYS_setgid, 0));
    t_show("setegid(0)", SYS(SYS_setegid, 0));
    t_show("setregid(0, 0)", SYS(SYS_setregid, 0, 0));
    t_show("setegid(80)", SYS(SYS_setegid, 80));
    t_show("setlogin", SYS(SYS_setlogin, "someone"));
    ids("after the refusals");

    // What it may: its own ids again.
    t_show("setuid(uid)", SYS(SYS_setuid, uid));
    t_show("seteuid(uid)", SYS(SYS_seteuid, uid));
    t_show("setgid(gid)", SYS(SYS_setgid, gid));
    t_show("setegid(egid)", SYS(SYS_setegid, egid));
    t_show("setreuid(-1, uid)", SYS(SYS_setreuid, NO, uid));
    t_show("setregid(gid, -1)", SYS(SYS_setregid, gid, NO));
    ids("after setting its own");

    // A file of its own, and who it is given to.
    long fd = SYS(SYS_open, "owned", O_CREAT | O_RDWR | O_TRUNC, 0644);
    t_check("open", fd >= 0);
    struct t_stat st;
    t_show("fstat", SYS(SYS_fstat64, fd, &st));
    t_check("the file is the user's", st.st_uid == (u32)uid);
    t_printf("mode %o\n", st.st_mode & 07777);
    t_show("chown(0, -1)", SYS(SYS_chown, "owned", 0, NO));
    t_show("chown(uid + 1, -1)", SYS(SYS_chown, "owned", uid + 1, NO));
    t_show("chown(-1, 99999)", SYS(SYS_chown, "owned", NO, 99999));
    t_show("fchown(-1, 99999)", SYS(SYS_fchown, fd, NO, 99999));
    t_show("chown(uid, -1)", SYS(SYS_chown, "owned", uid, NO));
    t_show("chown(-1, egid)", SYS(SYS_chown, "owned", NO, egid));
    // A group the user is in beside its own: given, seen, and the
    // set-user-ID bit gone with the change.
    long other = -1;
    for (long i = 0; i < got; i++) {
        if ((long)list[i] != egid) {
            other = (long)list[i];
            break;
        }
    }
    if (other >= 0) {
        t_show("chmod 04755", SYS(SYS_chmod, "owned", 04755));
        t_show("fstat", SYS(SYS_stat64, "owned", &st));
        t_printf("mode %o\n", st.st_mode & 07777);
        t_show("chown(-1, a group beside its own)", SYS(SYS_chown, "owned", NO, other));
        SYS(SYS_stat64, "owned", &st);
        t_check("the group is given", (long)st.st_gid == other);
        t_printf("mode %o\n", st.st_mode & 07777);
        t_show("fchown(-1, egid)", SYS(SYS_fchown, fd, NO, egid));
        SYS(SYS_stat64, "owned", &st);
        t_check("and taken back", (long)st.st_gid == egid);
    }
    // Its own file's modes: the bits it sets, and what they let it do.
    t_show("chmod 06755", SYS(SYS_chmod, "owned", 06755));
    SYS(SYS_stat64, "owned", &st);
    t_printf("mode %o\n", st.st_mode & 07777);
    t_show("access(R_OK | W_OK)", SYS(SYS_access, "owned", 6));
    t_show("chmod 0", SYS(SYS_chmod, "owned", 0));
    t_show("access(F_OK)", SYS(SYS_access, "owned", 0));
    t_show("chmod 0644", SYS(SYS_chmod, "owned", 0644));
    // A name that is there is EEXIST whoever asks, before it is asked
    // whether the directory may be written; one made in a directory that may
    // not be, is not.
    t_show("mkdir(/usr)", SYS(SYS_mkdir, "/usr", 0755));
    t_show("mkdir(/usr/bin)", SYS(SYS_mkdir, "/usr/bin", 0755));
    t_show("mkdir(/usr/gex-not-made)", SYS(SYS_mkdir, "/usr/gex-not-made", 0755));
    t_show("symlink(/usr)", SYS(SYS_symlink, "owned", "/usr"));
    t_show("link(owned, /usr)", SYS(SYS_link, "owned", "/usr"));
    t_show("rmdir(/usr/gex-not-there)", SYS(SYS_rmdir, "/usr/gex-not-there"));
    t_show("unlink", SYS(SYS_unlink, "owned"));
    ids("at the end");
    return 0;
}
