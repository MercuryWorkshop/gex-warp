// Audit sessions, as root (gexc only: making one is the superuser's, and a
// Mac's are launchd's to make; test/darwin/audit.test.js).
//
//   audit            the whole of it, below
//   audit show WHO   say which session this process is in
#include "t.h"

enum {
    SYS_setuid = 23, SYS_auditon = 351, SYS_getauid = 353, SYS_setauid = 354,
    SYS_getaudit_addr = 357, SYS_setaudit_addr = 358, SYS_audit_session_self = 428, SYS_audit_session_join = 429,
    SYS_audit_session_port = 432,
    A_GETSINFO_ADDR = 32, A_GETSFLAGS = 39, A_SETSFLAGS = 40,
    AU_IPv4 = 4,
    // AU_SESSION_FLAG_*: the first session, the window server, a terminal,
    // the console, an authenticated user.
    IS_INITIAL = 0x1, HAS_GRAPHIC_ACCESS = 0x10, HAS_TTY = 0x20, HAS_CONSOLE_ACCESS = 0x2000, HAS_AUTHENTICATED = 0x4000,
};

// auditinfo_addr_t
struct info {
    u32 auid, mask[2], port, type, addr[4], asid;
    u64 flags;
};

static void show(const char *who) {
    struct info i;
    t_memset(&i, 0x55, sizeof i);
    long r = SYS(SYS_getaudit_addr, &i, sizeof i);
    t_printf("%s: getaudit_addr %ld: audit user %u session %u flags 0x%llx terminal type %u\n", who, r, i.auid, i.asid, i.flags, i.type);
}

static long session(u32 auid, u32 asid, u64 flags, struct info *out) {
    struct info i;
    t_memset(&i, 0, sizeof i);
    i.auid = auid, i.asid = asid, i.flags = flags, i.type = AU_IPv4;
    long r = SYS(SYS_setaudit_addr, &i, sizeof i);
    if (out) *out = i;
    return r;
}

static long mask(const char *name, u64 *old, u64 *new) {
    size_t len = sizeof *old;
    return SYS(SYS_sysctlbyname, name, t_strlen(name), old, old ? &len : 0, new, new ? sizeof *new : 0);
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)apple;
    if (argc > 2 && !t_strcmp(argv[1], "show")) {
        show(argv[2]);
        return 0;
    }
    // The first process is in the session the machine began in.
    show("at the start");
    t_check("the first session has a port", SYS(SYS_audit_session_self) > 0);
    u64 flags = 7;
    t_show("A_GETSFLAGS", SYS(SYS_auditon, A_GETSFLAGS, &flags, sizeof flags));
    t_printf("flags 0x%llx\n", flags);

    // Another, as launchd makes one: the kernel chooses the id. (No id,
    // 0, asks for a new one as AU_ASSIGN_ASID does.)
    struct info i;
    t_show("setaudit_addr(no id)", session(-1, 0, 0, &i));
    t_printf("assigned session %u\n", i.asid);
    t_show("setaudit_addr(AU_ASSIGN_ASID, initial)", session(-1, -1, IS_INITIAL, &i));
    t_printf("assigned session %u\n", i.asid);
    u32 system = i.asid;
    show("in the system's session");
    // (A terminal type that is none is refused.)
    t_memset(&i, 0, sizeof i);
    i.asid = -1;
    t_show("setaudit_addr with no terminal type", SYS(SYS_setaudit_addr, &i, sizeof i));

    // Which flags may change: none, until the superuser says.
    u64 m = 9;
    t_show("the superuser's set mask", mask("audit.session.superuser_set_sflags_mask", &m, 0));
    t_printf("mask 0x%llx\n", m);
    flags = IS_INITIAL | HAS_AUTHENTICATED;
    t_show("A_SETSFLAGS(authenticated) with no mask", SYS(SYS_auditon, A_SETSFLAGS, &flags, sizeof flags));
    m = HAS_AUTHENTICATED;
    t_show("set the superuser's set mask", mask("audit.session.superuser_set_sflags_mask", 0, &m));
    t_show("set the member's clear mask", mask("audit.session.member_clear_sflags_mask", 0, &m));

    // A login's: a new session, with its user and what it reaches.
    t_show("setaudit_addr(AU_ASSIGN_ASID, user 501, graphic and tty)", session(501, -1, HAS_GRAPHIC_ACCESS | HAS_TTY, &i));
    u32 login = i.asid;
    t_printf("assigned session %u\n", login);
    show("in the login's session");
    u32 au = 7;
    t_show("getauid", SYS(SYS_getauid, &au));
    t_printf("audit user %u\n", au);
    au = 502;
    t_show("setauid(502), once it is set", SYS(SYS_setauid, &au));
    flags = HAS_GRAPHIC_ACCESS | HAS_TTY | HAS_AUTHENTICATED;
    t_show("A_SETSFLAGS(authenticated) by the superuser", SYS(SYS_auditon, A_SETSFLAGS, &flags, sizeof flags));
    flags |= HAS_CONSOLE_ACCESS;
    t_show("A_SETSFLAGS(console), which no mask has", SYS(SYS_auditon, A_SETSFLAGS, &flags, sizeof flags));
    flags = 0;
    SYS(SYS_auditon, A_GETSFLAGS, &flags, sizeof flags);
    t_printf("flags 0x%llx\n", flags);

    // The sessions' ports: the caller's own, and for the superuser any.
    long self = SYS(SYS_audit_session_self);
    long again = SYS(SYS_audit_session_self);
    t_check("audit_session_self is a port, the same one each time", self > 0 && self == again);
    u32 other = 0, mine = 0;
    t_show("audit_session_port(the system's)", SYS(SYS_audit_session_port, system, &other));
    t_show("audit_session_port(-1)", SYS(SYS_audit_session_port, -1, &mine));
    t_check("the caller's own by -1 is the same port, the system's another", mine == (u32)self && other != 0 && other != mine);
    t_show("audit_session_port(a session there is none of)", SYS(SYS_audit_session_port, 77, &mine));
    t_show("audit_session_join(a name that is no port)", SYS(SYS_audit_session_join, 0x7777));
    t_show("audit_session_join(the task's own port)", SYS(SYS_audit_session_join, TRAP(TRAP_task_self)));

    // A child is in its parent's session, and so is the program it runs;
    // a user is a member like any other: it may clear what the member mask
    // has and set nothing, and another session's port is not its to ask.
    long child_flag = 0;
    long pid = t_syscall2(&child_flag, SYS_fork, 0, 0, 0);
    if (child_flag) {
        show("a fork's child");
        t_show("setuid(501)", SYS(SYS_setuid, 501));
        t_show("audit_session_port(the system's) as a user", SYS(SYS_audit_session_port, system, &other));
        t_show("setaudit_addr as a user", session(501, login, 0, 0));
        struct info of;
        t_memset(&of, 0, sizeof of);
        of.asid = system;
        t_show("A_GETSINFO_ADDR(the system's) as a user", SYS(SYS_auditon, A_GETSINFO_ADDR, &of, sizeof of));
        t_printf("the system's: audit user %u flags 0x%llx masks %x %x\n", of.auid, of.flags, of.mask[0], of.mask[1]);
        flags = HAS_GRAPHIC_ACCESS | HAS_TTY;
        t_show("A_SETSFLAGS(not authenticated) by a member", SYS(SYS_auditon, A_SETSFLAGS, &flags, sizeof flags));
        flags |= HAS_AUTHENTICATED;
        t_show("A_SETSFLAGS(authenticated) by a member", SYS(SYS_auditon, A_SETSFLAGS, &flags, sizeof flags));
        char *args[] = {argv[0], "show", "the program it runs", 0};
        t_show("execve", SYS(SYS_execve, argv[0], args, envp));
        t_exit(1);
    }
    int status = 0;
    SYS(SYS_wait4, pid, &status, 0, 0);
    // What the child cleared is the session's, whoever asks.
    struct info of;
    t_memset(&of, 0, sizeof of);
    of.asid = login;
    t_show("A_GETSINFO_ADDR(the login's)", SYS(SYS_auditon, A_GETSINFO_ADDR, &of, sizeof of));
    t_printf("the login's: audit user %u flags 0x%llx masks %x %x\n", of.auid, of.flags, of.mask[0], of.mask[1]);
    of.asid = 77;
    t_show("A_GETSINFO_ADDR(a session there is none of)", SYS(SYS_auditon, A_GETSINFO_ADDR, &of, sizeof of));

    // Joining: whoever holds a session's port is in it for the asking.
    t_show("audit_session_join(the system's)", SYS(SYS_audit_session_join, other));
    show("joined to the system's");
    t_show("audit_session_join(the login's)", SYS(SYS_audit_session_join, self));
    show("joined to the login's again");
    // A session named by an id it has: the caller moves into it.
    t_show("setaudit_addr(the system's id)", session(-1, system, IS_INITIAL, 0));
    show("moved to the system's");
    return 0;
}
