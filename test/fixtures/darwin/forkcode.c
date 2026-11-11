// Code made at one address by processes a fork parted: each runs its own,
// in every order but the one `proc` checks (the child first, then its
// parent): the parent first, then a child it forked before; two children at
// once taking turns; a child forked after its sibling made its code, while
// that sibling still runs; a function unmapped and another mapped in its
// place while a relative still calls the first. (A loader puts a library
// where the address space has room, the same place in a parent and its
// child; a JIT the same.)
#include "t.h"

static long fork_(void) {
    long child_flag = -1;
    long pid = t_syscall2(&child_flag, SYS_fork, 0, 0, 0);
    if (pid < 0) return pid;
    return child_flag ? 0 : pid;
}

static void waited(const char *what, long pid) {
    int status = -1;
    long got = SYS(SYS_wait4, pid, &status, 0, 0);
    t_printf("%s: wait4 %s, status %x\n", what, got == pid ? "its pid" : "something else", status);
}

// A function that returns `n`, made at `where` (mapped there afresh) and
// called: the machine's `mov` of a constant to the register a result is
// in, and `ret`.
static int code_at(u8 *where, int n) {
    u8 *at = (u8 *)SYS(SYS_mmap, where, 0x4000, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0);
    if (at != where) return -1;
#if defined(__arm64__)
    u32 code[2] = {0x52800000u | (u32)n << 5, 0xd65f03c0u};
#else
    u8 code[6] = {0xb8, (u8)n, 0, 0, 0, 0xc3};
#endif
    for (unsigned i = 0; i < sizeof code; i++) at[i] = ((const u8 *)code)[i];
    if (SYS(SYS_mprotect, at, 0x4000, PROT_READ | PROT_EXEC)) return -2;
    return ((int (*)(void))at)();
}

// What is at `at` now, called.
static int call(u8 *at) {
    return ((int (*)(void))at)();
}

// A place there is room at, found and let go before anything forks.
static u8 *new_room(void) {
    u8 *room = (u8 *)SYS(SYS_mmap, 0, 0x4000, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    SYS(SYS_munmap, room, 0x4000);
    return room;
}

// Two pipes, one each way between a parent and a child, which take turns.
struct link {
    long to_rd, to_wr, from_rd, from_wr;
};

static void open_link(struct link *l) {
    l->to_rd = t_syscall2(&l->to_wr, SYS_pipe, 0, 0, 0);
    l->from_rd = t_syscall2(&l->from_wr, SYS_pipe, 0, 0, 0);
}

static void close_link(struct link *l) {
    SYS(SYS_close, l->to_rd);
    SYS(SYS_close, l->to_wr);
    SYS(SYS_close, l->from_rd);
    SYS(SYS_close, l->from_wr);
}

static void say(long fd) {
    char c = 0;
    SYS(SYS_write, fd, &c, 1);
}

static void hear(long fd) {
    char c;
    SYS(SYS_read, fd, &c, 1);
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;

    // The parent first, then a child it forked before.
    u8 *room = new_room();
    struct link l;
    open_link(&l);
    long pid = fork_();
    if (pid == 0) {
        hear(l.to_rd);
        t_printf("then a child forked before, at the same address: %d\n", code_at(room, 44));
        say(l.from_wr);
        t_exit(0);
    }
    t_printf("the parent's code first: %d\n", code_at(room, 33));
    say(l.to_wr);
    hear(l.from_rd);
    t_printf("the parent's again: %d\n", call(room));
    waited("the child forked before", pid);
    close_link(&l);

    // Two children at once, each with its own code there, taking turns.
    room = new_room();
    struct link a, b;
    open_link(&a);
    open_link(&b);
    long one = fork_();
    if (one == 0) {
        hear(a.to_rd);
        t_printf("one child's code: %d\n", code_at(room, 55));
        say(a.from_wr);
        hear(a.to_rd);
        t_printf("one child's again: %d\n", call(room));
        say(a.from_wr);
        t_exit(0);
    }
    long other = fork_();
    if (other == 0) {
        hear(b.to_rd);
        t_printf("the other's: %d\n", code_at(room, 66));
        say(b.from_wr);
        hear(b.to_rd);
        t_printf("the other's again: %d\n", call(room));
        say(b.from_wr);
        t_exit(0);
    }
    for (int i = 0; i < 2; i++) {
        say(a.to_wr);
        hear(a.from_rd);
        say(b.to_wr);
        hear(b.from_rd);
    }
    waited("one child", one);
    waited("the other", other);
    close_link(&a);
    close_link(&b);

    // A child forked after its sibling made code there, while that sibling
    // still runs.
    room = new_room();
    open_link(&a);
    one = fork_();
    if (one == 0) {
        t_printf("an older child's code: %d\n", code_at(room, 77));
        say(a.from_wr);
        hear(a.to_rd);
        t_printf("the older child's again: %d\n", call(room));
        t_exit(0);
    }
    hear(a.from_rd);
    other = fork_();
    if (other == 0) {
        t_printf("a younger child's: %d\n", code_at(room, 88));
        t_exit(0);
    }
    waited("the younger child", other);
    say(a.to_wr);
    waited("the older child", one);
    close_link(&a);

    // A function unmapped and another mapped in its place while a relative
    // still calls the first: the parent replaces it, then the child.
    room = new_room();
    open_link(&l);
    t_printf("code before a fork: %d\n", code_at(room, 99));
    pid = fork_();
    if (pid == 0) {
        t_printf("the child calls it: %d\n", call(room));
        say(l.from_wr);
        hear(l.to_rd);
        t_printf("the child's is the old one still: %d\n", call(room));
        SYS(SYS_munmap, room, 0x4000);
        t_printf("the child's new one: %d\n", code_at(room, 111));
        say(l.from_wr);
        t_exit(0);
    }
    hear(l.from_rd);
    SYS(SYS_munmap, room, 0x4000);
    t_printf("the parent's new one: %d\n", code_at(room, 100));
    say(l.to_wr);
    hear(l.from_rd);
    t_printf("the parent's is its own still: %d\n", call(room));
    waited("the child that replaced it too", pid);
    close_link(&l);
    return 0;
}
