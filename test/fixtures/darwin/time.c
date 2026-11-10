// Time: the wall clock, Mach's absolute time and its timebase, and the
// calls that wait.
#include "t.h"

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    struct t_timeval tv = {0, 0}, tv2;
    t_show("gettimeofday", SYS(SYS_gettimeofday, &tv, 0, 0));
    // After 2025, and microseconds in range.
    t_check("the wall clock", tv.tv_sec > 1735689600 && tv.tv_usec >= 0 && tv.tv_usec < 1000000);
    struct {
        u32 numer, denom;
    } tb = {0, 0};
    t_show("mach_timebase_info", TRAP(TRAP_mach_timebase_info, &tb));
    t_check("a timebase", tb.numer > 0 && tb.denom > 0);
    u64 t0 = t_nanos();
    t_check("mach_absolute_time runs", t0 > 0);

    // select as a sleep: 50 ms.
    struct t_timeval wait = {0, 50000};
    t_show("select with no descriptors", SYS(SYS_select, 0, 0, 0, 0, &wait));
    u64 t1 = t_nanos();
    t_check("it took 50 ms", t1 - t0 >= 50000000ull && t1 - t0 < 2000000000ull);
    t_check("and left the timeout alone", wait.tv_sec == 0 && wait.tv_usec == 50000);
    SYS(SYS_gettimeofday, &tv2, 0, 0);
    long us = (tv2.tv_sec - tv.tv_sec) * 1000000 + (tv2.tv_usec - tv.tv_usec);
    t_check("the wall clock moved with it", us >= 50000 && us < 5000000);

    // poll with a timeout, and with something to read.
    long wr = -1;
    long rd = t_syscall2(&wr, SYS_pipe, 0, 0, 0);
    struct {
        int fd;
        short events, revents;
    } pfd = {(int)rd, 1, 0};
    t0 = t_nanos();
    t_show("poll an empty pipe, 30 ms", SYS(SYS_poll, &pfd, 1, 30));
    t_check("it waited", t_nanos() - t0 >= 30000000ull);
    SYS(SYS_write, wr, "x", 1);
    t_show("poll a pipe with a byte", SYS(SYS_poll, &pfd, 1, -1));
    t_show("revents", pfd.revents);
    u32 set[32];
    t_memset(set, 0, sizeof set);
    set[rd / 32] |= 1u << (rd % 32);
    t_show("select it readable", SYS(SYS_select, rd + 1, set, 0, 0, 0));
    t_check("its bit is left set", set[rd / 32] == 1u << (rd % 32));
    char c;
    SYS(SYS_read, rd, &c, 1);
    wait.tv_usec = 20000;
    t_show("select it empty, 20 ms", SYS(SYS_select, rd + 1, set, 0, 0, &wait));
    t_check("its bit is cleared", set[rd / 32] == 0);

    // __semwait_signal(cond, mutex, timeout, relative, seconds, nanoseconds)
    // waits on a Mach semaphore, and with none there is nothing to wait on.
    t_show("__semwait_signal on no semaphore", SYS(SYS___semwait_signal, 0, 0, 1, 1, 0, 40000000));

    struct {
        struct t_timeval ru_utime, ru_stime;
        long rest[14];
    } ru;
    t_memset(&ru, 0xff, sizeof ru);
    t_show("getrusage", SYS(SYS_getrusage, 0, &ru));
    t_check("its times are times", ru.ru_utime.tv_sec >= 0 && ru.ru_utime.tv_usec >= 0 && ru.ru_utime.tv_usec < 1000000);
    t_show("getrusage of nobody", SYS(SYS_getrusage, 7, &ru));
    u64 lim[2] = {0, 0};
    t_show("getrlimit NOFILE", SYS(SYS_getrlimit, 8, lim));
    t_check("soft within hard", lim[0] > 0 && lim[0] <= lim[1]);
    t_show("getrlimit STACK", SYS(SYS_getrlimit, 3, lim));
    t_check("a stack limit", lim[0] >= 512 * 1024);
    t_show("getrlimit of nothing", SYS(SYS_getrlimit, 99, lim));
    u8 rnd[32];
    t_memset(rnd, 0, sizeof rnd);
    t_show("getentropy", SYS(SYS_getentropy, rnd, sizeof rnd));
    int any = 0;
    for (unsigned i = 0; i < sizeof rnd; i++) any |= rnd[i];
    t_check("it is not all zero", any != 0);
    t_show("getentropy of too much", SYS(SYS_getentropy, rnd, 257));
    t_show("issetugid", SYS(SYS_issetugid));
    t_check("thread_selfid", SYS(SYS_thread_selfid) > 0);
    return 0;
}
