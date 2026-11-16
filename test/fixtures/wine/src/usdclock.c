/* KUSER_SHARED_DATA's clocks as a program reads them, by constant address,
 * while it spins: SystemTime, InterruptTime and TickCount must advance at
 * about the rate of the elapsed time (QueryPerformanceCounter) and never be
 * seen half written (a KSYSTEM_TIME read High1Time, LowPart, High2Time, again
 * while the highs differ, never goes backwards); and, after one of its threads
 * has ended, across a 50 ms waitable timer's wait GetTickCount64 must have
 * moved by about 50 ms, as sync's "timer fires" reads it. The host advances a
 * Wine process's copy of the page (gex.usd_tick).
 *
 *   usdclock [SECS]   spin SECS seconds (default 2), then say each check
 *
 * gex-nt-cc: clang --target=ARCH-w64-mingw32 -fms-extensions -fuse-ld=lld */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>

#define KUSER 0x7ffe0000u
#define AT(off) (*(volatile ULONG *)(ULONG_PTR)(KUSER + (off)))
#define INTERRUPT 0x008
#define SYSTEM 0x014
#define TICK 0x320

static int failures;

static void check(int ok, const char *what, const char *detail) {
    printf("%s %s%s%s\n", ok ? "ok" : "FAIL", what, detail ? ": " : "", detail ? detail : "");
    failures += !ok;
}

/* A KSYSTEM_TIME read as Windows reads one; *spins counts the retries. */
static ULONGLONG ksystem_time(ULONG off, unsigned *spins) {
    for (;;) {
        ULONG hi = AT(off + 4), lo = AT(off), hi2 = AT(off + 8);
        if (hi == hi2) return (ULONGLONG)hi << 32 | lo;
        (*spins)++;
    }
}

static DWORD WINAPI quit(void *arg) { (void)arg; return 0; }

int main(int argc, char **argv) {
    double secs = argc > 1 ? atof(argv[1]) : 2.0;
    LARGE_INTEGER f, q0, q;
    ULONGLONG s0, i0, t0, s, i, t, ps, pi, pt;
    unsigned spins = 0, back = 0, reads = 0;
    double el, rs, ri, rt;
    char d[256];

    QueryPerformanceFrequency(&f);
    QueryPerformanceCounter(&q0);
    ps = s0 = ksystem_time(SYSTEM, &spins);
    pi = i0 = ksystem_time(INTERRUPT, &spins);
    pt = t0 = ksystem_time(TICK, &spins);
    do {
        s = ksystem_time(SYSTEM, &spins);
        i = ksystem_time(INTERRUPT, &spins);
        t = ksystem_time(TICK, &spins);
        back += (s < ps) + (i < pi) + (t < pt);
        ps = s; pi = i; pt = t;
        reads++;
        QueryPerformanceCounter(&q);
        el = (double)(q.QuadPart - q0.QuadPart) / f.QuadPart;
    } while (el < secs);

    /* SystemTime and InterruptTime in 100 ns, TickCount in ms; the page
     * moves every 16 ms, so a reading lags by up to that */
    rs = (s - s0) / 1e7 / el; ri = (i - i0) / 1e7 / el; rt = (t - t0) / 1e3 / el;
    snprintf(d, sizeof(d), "%.3f s of SystemTime in %.3f s (rate %.3f)", (s - s0) / 1e7, el, rs);
    check(rs > 0.9 && rs < 1.1, "usdclock system time advances", d);
    snprintf(d, sizeof(d), "%.3f s of InterruptTime in %.3f s (rate %.3f)", (i - i0) / 1e7, el, ri);
    check(ri > 0.9 && ri < 1.1, "usdclock interrupt time advances", d);
    snprintf(d, sizeof(d), "%.3f s of TickCount in %.3f s (rate %.3f)", (t - t0) / 1e3, el, rt);
    check(rt > 0.9 && rt < 1.1, "usdclock tick count advances", d);
    snprintf(d, sizeof(d), "TickCount %llu, InterruptTime %llu ms", t, i / 10000);
    check(t + 32 >= i / 10000 && t <= i / 10000 + 32, "usdclock tick count is the interrupt time in ms", d);
    snprintf(d, sizeof(d), "%u reads, %u went backwards, %u retries of a half-written value", reads, back, spins);
    check(back == 0 && reads > 100, "usdclock never torn", d);

    /* A 50 ms waitable timer, ten times, after a thread of the process has
     * ended (its end is not the process's): the tick count across each wait
     * (the page moves every 16 ms, a timer's tick late by some ms more: 25
     * ms at the least) and the elapsed time */
    {
        HANDLE th = CreateThread(0, 0, quit, 0, 0, 0);
        WaitForSingleObject(th, INFINITE);
        CloseHandle(th);
        HANDLE wt = CreateWaitableTimerW(0, FALSE, 0);
        LARGE_INTEGER due = {.QuadPart = -500000};
        ULONGLONG lo = ~0ull, hi = 0;
        int n = 0;
        char *o = d;
        for (int k = 0; k < 10; k++) {
            ULONGLONG a = GetTickCount64(), b;
            QueryPerformanceCounter(&q0);
            SetWaitableTimer(wt, &due, 0, 0, 0, FALSE);
            if (WaitForSingleObject(wt, 5000) != WAIT_OBJECT_0) continue;
            b = GetTickCount64();
            QueryPerformanceCounter(&q);
            if (b - a < lo) lo = b - a;
            if (b - a > hi) hi = b - a;
            if (o < d + sizeof(d) - 24) o += snprintf(o, d + sizeof(d) - o, "%s%llu/%.0f", k ? " " : "", b - a, (q.QuadPart - q0.QuadPart) * 1000.0 / f.QuadPart);
            n++;
        }
        CloseHandle(wt);
        check(n == 10 && lo >= 25 && hi <= 200, "usdclock a 50 ms timer's wait moves the tick count (tick ms/elapsed ms)", d);
    }
    printf("%d failures\n", failures);
    return failures != 0;
}
