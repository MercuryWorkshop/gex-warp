// kqueue and kevent: descriptors to read and write, timers, events a
// program posts to itself, a process ending; added, switched off and on,
// told once or each time; and waited for, for a while or for ever.
#include "t.h"

enum { SYS_kqueue = 362, SYS_kevent = 363, SYS_kevent64 = 369, SYS_kevent_qos = 374 };
enum { READ = -1, WRITE = -2, PROC = -5, SIGNAL = -6, TIMER = -7, USER = -10 };
enum { ADD = 1, DELETE = 2, ENABLE = 4, DISABLE = 8, ONESHOT = 0x10, CLEAR = 0x20, RECEIPT = 0x40, DISPATCH = 0x80, ERROR = 0x4000, EOF_ = 0x8000 };
enum { TRIGGER = 0x01000000, FFOR = 0x80000000, FFCOPY = 0xc0000000, NOTE_EXIT = 0x80000000, USECONDS = 2, ABSOLUTE = 8, MACHTIME = 0x100 };
enum { SPECIFIC = 0x100, SYS_guarded_close_np = 442, SYS_guarded_kqueue_np = 443 };

struct kevent {
    u64 ident;
    short filter;
    u16 flags;
    u32 fflags;
    i64 data;
    u64 udata;
};
struct kevent64 {
    u64 ident;
    short filter;
    u16 flags;
    u32 fflags;
    i64 data;
    u64 udata, ext[2];
};
struct kevent_qos {
    u64 ident;
    short filter;
    u16 flags;
    int qos;
    u64 udata;
    u32 fflags, xflags;
    i64 data;
    u64 ext[4];
};

static long kq;
static struct kevent got[8];
static const struct t_timespec now_ = {0, 0};

// One change, and nothing waited for.
static long change(u64 ident, int filter, int flags, u32 fflags, i64 data, u64 udata) {
    struct kevent e = {ident, (short)filter, (u16)flags, fflags, data, udata};
    return SYS(SYS_kevent, kq, &e, 1, 0, 0, &now_);
}
// What is ready now, or within `ms` (-1: however long it takes).
static long ready(int ms) {
    struct t_timespec t = {ms / 1000, (long)(ms % 1000) * 1000000};
    for (int i = 0; i < 8; i++) got[i] = (struct kevent){0xeeee, 0, 0, 0, 0, 0};
    return SYS(SYS_kevent, kq, 0, 0, got, 8, ms < 0 ? 0 : &t);
}
static void say(const char *what, long n) {
    t_printf("%s: %ld", what, n);
    for (long i = 0; i < n && i < 8; i++)
        t_printf(" [%s%lu filter %d flags 0x%x fflags 0x%x data %ld udata %lu]", "", (unsigned long)got[i].ident % 1000, got[i].filter, got[i].flags, got[i].fflags, (long)got[i].data, (unsigned long)got[i].udata);
    t_printf("\n");
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    kq = SYS(SYS_kqueue);
    t_check("kqueue", kq >= 3);
    say("an empty queue", ready(0));
    long wr = -1;
    long rd = t_syscall2(&wr, SYS_pipe, 0, 0, 0);
    // (The descriptors' numbers are told apart by what is asked to be
    // handed back with them.)
#define R 1000 + rd - rd
#define W 2000 + wr - wr

    // A pipe to read: nothing, then what is written, each time asked.
    t_show("add a read", change(rd, READ, ADD, 0, 0, 11));
    say("nothing to read", ready(0));
    SYS(SYS_write, wr, "hello", 5);
    say("five bytes", ready(0));
    say("still there", ready(0));
    char buf[64];
    SYS(SYS_read, rd, buf, 2);
    say("three left", ready(0));
    SYS(SYS_read, rd, buf, 3);
    say("read dry", ready(0));

    // Told once of each change.
    t_show("the same, told once (the note is changed, not added twice)", change(rd, READ, ADD | CLEAR, 0, 0, 12));
    SYS(SYS_write, wr, "ab", 2);
    say("written to", ready(0));
    say("not told again", ready(0));
    SYS(SYS_write, wr, "c", 1);
    say("written to again", ready(0));
    SYS(SYS_read, rd, buf, 3);

    // Off, on, once, and until asked again.
    t_show("switched off", change(rd, READ, DISABLE, 0, 0, 0));
    SYS(SYS_write, wr, "x", 1);
    say("off", ready(0));
    t_show("switched on", change(rd, READ, ENABLE, 0, 0, 0));
    say("on", ready(0));
    t_show("once", change(rd, READ, ADD | ONESHOT, 0, 0, 13));
    say("once", ready(0));
    say("and gone", ready(0));
    t_show("delete what is gone", change(rd, READ, DELETE, 0, 0, 0));
    t_show("until asked again", change(rd, READ, ADD | DISPATCH, 0, 0, 14));
    say("said", ready(0));
    say("and silent", ready(0));
    t_show("asked again", change(rd, READ, ENABLE, 0, 0, 0));
    say("said again", ready(0));
    t_show("delete", change(rd, READ, DELETE, 0, 0, 0));
    SYS(SYS_read, rd, buf, 1);

    // A pipe to write is ready while there is room.
    t_show("add a write", change(wr, WRITE, ADD, 0, 0, 21));
    long n = ready(0);
    t_printf("room to write: %ld, filter %d flags 0x%x udata %lu, some room %d\n", n, got[0].filter, got[0].flags, (unsigned long)got[0].udata, got[0].data > 0);
    t_show("delete", change(wr, WRITE, DELETE, 0, 0, 0));

    // What goes wrong is said in an event when there is room for one, and
    // by the call when there is not.
    struct kevent bad[2] = {{99, READ, ADD, 0, 0, 1}, {rd, READ, DELETE, 0, 0, 2}};
    n = SYS(SYS_kevent, kq, bad, 2, got, 8, &now_);
    t_printf("two that fail, with room: %ld [flags 0x%x data %ld udata %lu] [flags 0x%x data %ld udata %lu]\n", n, got[0].flags, (long)got[0].data, (unsigned long)got[0].udata, got[1].flags,
        (long)got[1].data, (unsigned long)got[1].udata);
    t_show("one that fails, without", SYS(SYS_kevent, kq, bad, 1, 0, 0, &now_));
    struct kevent told = {rd, READ, ADD | RECEIPT, 0, 0, 3};
    n = SYS(SYS_kevent, kq, &told, 1, got, 8, &now_);
    t_printf("one that asks how it went: %ld [flags 0x%x data %ld udata %lu]\n", n, got[0].flags, (long)got[0].data, (unsigned long)got[0].udata);
    t_show("delete", change(rd, READ, DELETE, 0, 0, 0));
    t_show("a filter there is none of", change(1, 5, ADD, 0, 0, 0));
    t_show("kevent on what is no queue", SYS(SYS_kevent, rd, 0, 0, got, 8, &now_));
    t_show("kevent on nothing", SYS(SYS_kevent, 99, 0, 0, got, 8, &now_));

    // An event the program posts to itself.
    t_show("add a user event", change(7, USER, ADD | CLEAR, 0, 0, 31));
    say("not posted", ready(0));
    t_show("post it, with flags", change(7, USER, 0, TRIGGER | FFOR | 0x5, 0, 0));
    say("posted", ready(0));
    say("once", ready(0));
    t_show("post it twice", change(7, USER, 0, TRIGGER | FFOR | 0x2, 0, 0) + change(7, USER, 0, TRIGGER | FFOR | 0x8, 0, 0));
    say("posted, the flags together", ready(0));
    t_show("delete", change(7, USER, DELETE, 0, 0, 0));

    // Time: none to wait, a little, and a timer's.
    u64 t0 = t_nanos();
    say("nothing for 30 ms", ready(30));
    u64 waited = (t_nanos() - t0) / 1000000;
    t_check("  which were waited", waited >= 25 && waited < 10000);
    t_show("a timer, every 20 ms", change(1, TIMER, ADD, 0, 20, 41));
    say("too soon", ready(0));
    t0 = t_nanos();
    n = ready(-1);
    waited = (t_nanos() - t0) / 1000000;
    t_printf("waited for: %ld, filter %d udata %lu, fired %d\n", n, got[0].filter, (unsigned long)got[0].udata, got[0].data >= 1);
    t_check("  about that long", waited >= 10 && waited < 10000);
    n = ready(1000);
    t_printf("and again: %ld, fired %d\n", n, got[0].data >= 1);
    t_show("delete", change(1, TIMER, DELETE, 0, 0, 0));
    t_show("a timer, once, in microseconds", change(2, TIMER, ADD | ONESHOT, USECONDS, 5000, 42));
    n = ready(1000);
    // (How many times it is said to have fired by then depends on how late
    // the asking is: a busy Mac says twice or more of a timer that fires once.)
    t_printf("fired: %ld, filter %d flags 0x%x, fired %d\n", n, got[0].filter, got[0].flags, got[0].data >= 1);
    say("and not again", ready(30));
    // A timer sooner than the time given to wait.
    t_show("a timer, once", change(3, TIMER, ADD | ONESHOT, 0, 10, 43));
    t0 = t_nanos();
    n = ready(5000);
    t_check("a wait ends at the first timer", n == 1 && (t_nanos() - t0) / 1000000 < 4500);

    // Another process: what it writes wakes a wait, its end is an end of
    // file, and its ending is an event.
    t_show("add a read", change(rd, READ, ADD, 0, 0, 51));
    long second = -1;
    long pid = t_syscall2(&second, SYS_fork, 0, 0, 0);
    if (pid >= 0 && second) {
        // (select's time is seconds and microseconds: 20 ms.)
        struct t_timespec nap = {0, 20000};
        // A child has none of its parent's queues.
        if (SYS(SYS_kevent, kq, 0, 0, got, 8, &now_) != -9) SYS(SYS_write, 1, "the child has the queue\n", 24);
        SYS(SYS_close, rd);
        SYS(SYS_select, 0, 0, 0, 0, &nap);
        SYS(SYS_write, wr, "child", 5);
        SYS(SYS_select, 0, 0, 0, 0, &nap);
        t_exit(7);
    }
    SYS(SYS_close, wr);
    t_show("watch the child end", change(pid, PROC, ADD, NOTE_EXIT, 0, 52));
    // What it writes, its end and its pipe's are three events, the last
    // two in either order, and any of them together if it is quick.
    int wrote = 0, ended = 0, eof = 0;
    for (int tries = 0; tries < 8 && !(wrote && ended && eof); tries++) {
        n = ready(tries ? 2000 : -1);
        for (long i = 0; i < n; i++) {
            if (got[i].filter == PROC) ended = (got[i].fflags & NOTE_EXIT) != 0 && got[i].udata == 52 && got[i].ident == (u64)pid;
            if (got[i].filter == READ && got[i].data == 5 && got[i].udata == 51 && !wrote) wrote = SYS(SYS_read, rd, buf, 5) == 5;
            else if (got[i].filter == READ) eof = (got[i].flags & EOF_) != 0 && got[i].data == 0;
        }
        // (An end of file is said each time asked, until it is not wanted.)
        if (eof) change(rd, READ, DELETE, 0, 0, 0);
    }
    t_check("the child wrote", wrote);
    t_check("the child ended", ended);
    t_check("its pipe is at its end", eof);
    int status = 0;
    SYS(SYS_wait4, pid, &status, 0, 0);
    t_show("watch a process there is none of", change(99999, PROC, ADD, NOTE_EXIT, 0, 0));

    // A signal: each raising counted from when it is watched, whatever the
    // process does with it (here nothing: it is ignored). 30 is SIGUSR1.
    long me = SYS(SYS_getpid);
    t_sigaction(30, (void *)1, 0, 0, 0);
    SYS(SYS_kill, me, 30);
    t_show("watch a signal", change(30, SIGNAL, ADD, 0, 0, 81));
    say("none since", ready(0));
    SYS(SYS_kill, me, 30);
    SYS(SYS_kill, me, 30);
    say("raised twice", ready(0));
    say("and told once", ready(0));
    // From another process, while this one waits. (A wait a signal ends
    // may say so, and is made again.)
    second = -1;
    pid = t_syscall2(&second, SYS_fork, 0, 0, 0);
    if (pid >= 0 && second) {
        struct t_timespec nap = {0, 20000};
        SYS(SYS_select, 0, 0, 0, 0, &nap);
        SYS(SYS_kill, me, 30);
        t_exit(0);
    }
    do n = ready(-1);
    while (n == -4);
    say("raised by another", n);
    SYS(SYS_wait4, pid, &status, 0, 0);
    t_show("delete", change(30, SIGNAL, DELETE, 0, 0, 0));
    t_show("watch a signal there is none of", change(99, SIGNAL, ADD, 0, 0, 0));

    // A file on a disk: something to read while its descriptor has not
    // read to its end, and how much.
    long file = SYS(SYS_open, "k.txt", 0x602, 0644); // O_RDWR | O_CREAT | O_TRUNC
    SYS(SYS_write, file, "0123456789", 10);
    t_show("watch a file", change(file, READ, ADD, 0, 0, 91));
    say("at its end", ready(0));
    SYS(SYS_lseek, file, 4, 0);
    say("six left", ready(0));
    SYS(SYS_read, file, buf, 6);
    say("read to its end", ready(0));
    SYS(SYS_write, file, "ab", 2);
    SYS(SYS_lseek, file, 10, 0);
    say("two written past it", ready(0));
    t_show("delete", change(file, READ, DELETE, 0, 0, 0));
    SYS(SYS_close, file);
    SYS(SYS_unlink, "k.txt");

    // Two notes on one thing, told apart by what each hands back.
    t_show("two notes on one event", change(5, USER, ADD | CLEAR | SPECIFIC, 0, 0, 101) + change(5, USER, ADD | CLEAR | SPECIFIC, 0, 0, 102));
    t_show("post the second", change(5, USER, SPECIFIC, TRIGGER, 0, 102));
    say("the second alone", ready(0));
    t_show("post one there is none of", change(5, USER, SPECIFIC, TRIGGER, 0, 103));
    t_show("post it by no name", change(5, USER, 0, TRIGGER, 0, 0));
    t_show("delete the first", change(5, USER, DELETE | SPECIFIC, 0, 0, 101));
    t_show("delete it again", change(5, USER, DELETE | SPECIFIC, 0, 0, 101));
    t_show("delete the second", change(5, USER, DELETE | SPECIFIC, 0, 0, 102));

#if defined(__arm64__)
    // A timer set for a moment on the clock mach_absolute_time reads, in
    // its units: 10 ms from now. (Not for x86-64, where the Mac these are
    // checked against is Rosetta: its clock is not the kernel's.)
    struct {
        u32 numer, denom;
    } base = {1, 1};
    TRAP(TRAP_mach_timebase_info, &base);
    u64 moment = (u64)TRAP(TRAP_mach_absolute_time) + 10000000ull * base.denom / base.numer;
    t_show("a timer for a moment", change(6, TIMER, ADD, MACHTIME | ABSOLUTE, (i64)moment, 44));
    say("not yet", ready(0));
    t0 = t_nanos();
    n = ready(5000);
    waited = (t_nanos() - t0) / 1000000;
    t_printf("then: %ld, filter %d flags 0x%x udata %lu\n", n, got[0].filter, got[0].flags, (unsigned long)got[0].udata);
    t_check("  about then", waited >= 5 && waited < 10000);
    say("and once", ready(30));
#endif

    // A queue with a guard on its descriptor, which libdispatch's is.
    u64 guard = 0x1234;
    long guarded = SYS(SYS_guarded_kqueue_np, &guard, 3);
    t_check("guarded_kqueue_np", guarded >= 3);
    t_show("kevent on it", SYS(SYS_kevent, guarded, 0, 0, got, 8, &now_));
    t_show("guarded_close_np", SYS(SYS_guarded_close_np, guarded, &guard));
    t_show("kevent on it closed", SYS(SYS_kevent, guarded, 0, 0, got, 8, &now_));

    // The wider calls, each with a queue of its own: a queue is for the
    // call it was first used with.
    struct kevent64 e64 = {8, USER, ADD | CLEAR, 0, 0, 61, {0xa, 0xb}}, g64[2];
    t_show("kevent64 on a queue kevent has used", SYS(SYS_kevent64, kq, &e64, 1, 0, 0, 0, &now_));
    t_show("close the queue", SYS(SYS_close, kq));
    t_show("kevent on it closed", SYS(SYS_kevent, kq, 0, 0, got, 8, &now_));
    kq = SYS(SYS_kqueue);
    t_show("kevent64: add", SYS(SYS_kevent64, kq, &e64, 1, 0, 0, 0, &now_));
    t_show("kevent on a queue kevent64 has used", ready(0));
    e64 = (struct kevent64){8, USER, 0, TRIGGER, 0, 0, {0, 0}};
    n = SYS(SYS_kevent64, kq, &e64, 1, g64, 2, 0, &now_);
    t_printf("kevent64: %ld [%lu filter %d fflags 0x%x udata %lu ext 0x%lx 0x%lx]\n", n, (unsigned long)g64[0].ident, g64[0].filter, g64[0].fflags & 0xffffff, (unsigned long)g64[0].udata,
        (unsigned long)g64[0].ext[0], (unsigned long)g64[0].ext[1]);
    t_show("kevent64: nothing, and not waiting (KEVENT_FLAG_IMMEDIATE)", SYS(SYS_kevent64, kq, 0, 0, g64, 2, 1, 0));
    // (The pipe is at its end, which is something to read.)
    e64 = (struct kevent64){rd, READ, ADD, 0, 0, 62, {0, 0}};
    t_show("kevent64: a change, asking only what went wrong with it (KEVENT_FLAG_ERROR_EVENTS)", SYS(SYS_kevent64, kq, &e64, 1, g64, 2, 2, &now_));
    n = SYS(SYS_kevent64, kq, 0, 0, g64, 2, 0, &now_);
    t_printf("kevent64: and what is ready: %ld [filter %d udata %lu]\n", n, g64[0].filter, (unsigned long)g64[0].udata);
    e64 = (struct kevent64){99, READ, ADD, 0, 0, 63, {0, 0}};
    n = SYS(SYS_kevent64, kq, &e64, 1, g64, 2, 2, &now_);
    t_printf("kevent64: a change that fails, asking only that: %ld [flags 0x%x data %ld udata %lu]\n", n, g64[0].flags, (long)g64[0].data, (unsigned long)g64[0].udata);
    SYS(SYS_close, kq);
    kq = SYS(SYS_kqueue);
    struct kevent_qos eq = {9, USER, ADD | CLEAR, 0, 71, TRIGGER, 0, 0, {0, 0, 0, 0}}, gq[2];
    n = SYS(SYS_kevent_qos, kq, &eq, 1, gq, 2, 0, 0, 1);
    t_printf("kevent_qos: %ld [%lu filter %d udata %lu]\n", n, (unsigned long)gq[0].ident, gq[0].filter, (unsigned long)gq[0].udata);
    // A note of the kind libdispatch makes -- told once and then off, one
    // of several told apart -- is not removed while it is off: it goes
    // when it is next put on, and says so.
    SYS(SYS_close, kq);
    kq = SYS(SYS_kqueue);
    enum { D2 = DISPATCH | SPECIFIC };
    t_show("a note told once and then off", change(1, USER, ADD | D2 | CLEAR, 0, 0, 11));
    t_show("removed before it says anything", change(1, USER, DELETE | SPECIFIC, 0, 0, 11));
    change(1, USER, ADD | D2 | CLEAR, 0, 0, 11);
    change(1, USER, SPECIFIC, TRIGGER, 0, 11);
    say("it says its thing", ready(0));
    t_show("removed while off", change(1, USER, DELETE | SPECIFIC, 0, 0, 11));
    say("and nothing of it", ready(0));
    t_show("removed again", change(1, USER, DELETE | SPECIFIC, 0, 0, 11));
    t_show("posted, on its way out", change(1, USER, SPECIFIC, TRIGGER, 0, 11));
    say("which is not said", ready(0));
    t_show("put on", change(1, USER, ENABLE | SPECIFIC, 0, 0, 11));
    say("it says it is gone", ready(0));
    say("once", ready(0));
    t_show("and it is", change(1, USER, DELETE | SPECIFIC, 0, 0, 11));
    change(2, USER, ADD | D2 | CLEAR, 0, 0, 22);
    change(2, USER, SPECIFIC, TRIGGER, 0, 22);
    say("another says its thing", ready(0));
    t_show("removed and put on at once", change(2, USER, DELETE | ENABLE | SPECIFIC, 0, 0, 22));
    say("gone without a word", ready(0));
    change(4, USER, ADD | D2 | CLEAR, 0, 0, 44);
    t_show("one switched off by hand", change(4, USER, DISABLE | SPECIFIC, 0, 0, 44));
    t_show("removed", change(4, USER, DELETE | SPECIFIC, 0, 0, 44));
    t_show("put on", change(4, USER, ENABLE | SPECIFIC, 0, 0, 44));
    say("it says it is gone", ready(0));
    t_show("a timer of that kind, for once", change(7, TIMER, ADD | D2 | ONESHOT, 0, 0, 77));
    say("it fires", ready(1000));
    t_show("removed", change(7, TIMER, DELETE | SPECIFIC, 0, 0, 77));
    t_show("put on", change(7, TIMER, ENABLE | SPECIFIC, 0, 0, 77));
    say("it says it is gone", ready(0));
    t_show("a timer for once, of the plain kind", change(8, TIMER, ADD | ONESHOT, 0, 0, 88));
    say("it fires", ready(1000));
    t_show("and is gone", change(8, TIMER, DELETE, 0, 0, 88));
    SYS(SYS_close, kq);
    kq = SYS(SYS_kqueue);
    // Two changes at once, each 72 bytes: the second is of no note, and
    // is all that is answered; what the first posted is told after, with
    // another.
    struct kevent_qos two[2] = {{10, USER, ADD | CLEAR, 0, 72, TRIGGER, 0, 0, {0, 0, 0, 0}}, {11, USER, DELETE, 0, 73, 0, 0, 0, {0, 0, 0, 0}}}, g3[3];
    t_memset(g3, 0, sizeof g3);
    n = SYS(SYS_kevent_qos, kq, two, 2, g3, 3, 0, 0, 1);
    t_printf("kevent_qos, two changes: %ld [%lu flags 0x%x data %ld udata %lu]\n", n, (unsigned long)g3[0].ident, g3[0].flags, (long)g3[0].data, (unsigned long)g3[0].udata);
    struct kevent_qos more = {12, USER, ADD | CLEAR, 0, 74, TRIGGER, 0, 0, {1, 2, 3, 4}};
    t_memset(g3, 0, sizeof g3);
    n = SYS(SYS_kevent_qos, kq, &more, 1, g3, 3, 0, 0, 1);
    t_printf("kevent_qos, two events: %ld", n);
    for (int k = 10; k <= 12; k++)
        for (long i = 0; i < n && i < 3; i++)
            if (g3[i].ident == (u64)k) t_printf(" [%d udata %lu ext %lu %lu %lu %lu]", k, (unsigned long)g3[i].udata, (unsigned long)g3[i].ext[0], (unsigned long)g3[i].ext[1], (unsigned long)g3[i].ext[2], (unsigned long)g3[i].ext[3]);
    t_printf("\n");
    SYS(SYS_close, kq);

    // A watch is of a descriptor: it goes when the descriptor is closed,
    // though another descriptor still has the file open, and a file that
    // then gets the number is not watched.
    static const struct t_timespec none = {0, 0};
    struct kevent got1[4];
    long q = SYS(SYS_kqueue), w1 = -1, w2 = -1;
    long r1 = t_syscall2(&w1, SYS_pipe, 0, 0, 0);
    long held = SYS(SYS_dup, r1);
    struct kevent watch = {(u64)r1, READ, ADD, 0, 0, 0x1111};
    t_show("watch a pipe", SYS(SYS_kevent, q, &watch, 1, 0, 0, &none));
    SYS(SYS_close, r1);
    SYS(SYS_write, w1, "x", 1);
    t_show("written to, its watched descriptor closed and another still open", SYS(SYS_kevent, q, 0, 0, got1, 4, &none));
    long r2 = t_syscall2(&w2, SYS_pipe, 0, 0, 0);
    t_check("another pipe has the number", r2 == r1);
    SYS(SYS_write, w2, "y", 1);
    t_show("which is written to, unwatched", SYS(SYS_kevent, q, 0, 0, got1, 4, &none));
    watch = (struct kevent){(u64)r2, READ, ADD, 0, 0, 0x2222};
    SYS(SYS_kevent, q, &watch, 1, 0, 0, &none);
    n = SYS(SYS_kevent, q, 0, 0, got1, 4, &none);
    t_printf("and watched: %ld, udata 0x%lx\n", n, n > 0 ? (unsigned long)got1[0].udata : 0ul);
    (void)held;
    return 0;
}
