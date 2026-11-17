/* Code that runs across a page boundary while the second page changes
 * between runs (i386 and x86-64 alike: the bytes mean the same in both).
 * A function of nops at the end of one page, its `lea eax, [ecx+1]; ret`
 * on the next, is called argv[1] times (default 2000) in each case:
 * - protect: the second page made writable, a byte of data on it written,
 *   and made executable again, between calls (a JIT's W^X toggling);
 * - rwx: both pages writable and executable, data on the second written
 *   between calls with no protection change (data beside code);
 * - remap: the second page decommitted, committed again and its code
 *   written again, between calls;
 * - fault: the second page decommitted before a call; the call's fetch
 *   there faults (an access violation, as Windows raises it), the vectored
 *   handler commits it again with its code, and the call goes on.
 * - imm: on a page of its own, writable and executable, `mov eax, imm32;
 *   test ecx, ecx; jnz chain; ret`, `chain` being 1000 blocks of `jnz` to
 *   the next (code that runs, or is reached, beside the patched block),
 *   with the immediate rewritten before every call (a JIT patching a
 *   constant into code it runs, with no flush), argv[1] times; then, after
 *   argv[2] ms (default 2500) with no rewrite, once more and called again
 *   (argv[3] "imm": this case alone; "noimm": all but it);
 * - blocks: on a page of its own, writable and executable, a function of
 *   BLOCKS blocks, each `add eax, imm32` and a branch to the next, whose
 *   result is their sum, with one block's immediate rewritten before each of
 *   argv[1] calls, in turn; meanwhile a second thread runs a function on the
 *   same page that nothing rewrites, and every eighth rewrite it also calls
 *   the rewritten one and must get the new sum (code rewritten under one
 *   thread and run by another, with no flush). "blocks" as argv[3]: this
 *   case alone.
 * Every call must return its argument plus one (imm: the immediate;
 * blocks: the sum), and
 * every case must end:
 * a block crossing into a page whose translation is retired is lifted
 * again or faults, never retried for ever. One line a case, "pagecross:
 * CASE ok N", then "pagecross ok". */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define PAGE 4096
#define NOPS 14
typedef int (__fastcall *fn)(int);

static unsigned char *base;
static volatile LONG faults;

static void write_code(void)
{
    memset(base + PAGE - NOPS, 0x90, NOPS);
    base[PAGE] = 0x8d; base[PAGE + 1] = 0x41; base[PAGE + 2] = 0x01;  /* lea eax, [ecx+1] */
    base[PAGE + 3] = 0xc3;                                            /* ret */
}

static LONG CALLBACK on_fault(EXCEPTION_POINTERS *e)
{
    EXCEPTION_RECORD *r = e->ExceptionRecord;
    DWORD old;
    if (r->ExceptionCode != EXCEPTION_ACCESS_VIOLATION || r->NumberParameters < 2) return EXCEPTION_CONTINUE_SEARCH;
    if ((unsigned char *)r->ExceptionInformation[1] < base + PAGE || (unsigned char *)r->ExceptionInformation[1] >= base + 2 * PAGE)
        return EXCEPTION_CONTINUE_SEARCH;
    if (!VirtualAlloc(base + PAGE, PAGE, MEM_COMMIT, PAGE_READWRITE)) return EXCEPTION_CONTINUE_SEARCH;
    write_code();
    VirtualProtect(base + PAGE, PAGE, PAGE_EXECUTE_READ, &old);
    FlushInstructionCache(GetCurrentProcess(), base, 2 * PAGE);
    InterlockedIncrement(&faults);
    return EXCEPTION_CONTINUE_EXECUTION;
}

static int run(const char *name, int n, void (*between)(int))
{
    fn f = (fn)(base + PAGE - NOPS);
    DWORD t0 = GetTickCount();
    int i, bad = 0;
    for (i = 0; i < n; i++)
    {
        if (between) between(i);
        if (f(i) != i + 1) bad++;
    }
    printf("pagecross: %s %s %d (%lu ms)\n", name, bad ? "wrong" : "ok", n, GetTickCount() - t0);
    return bad;
}

static void protect(int i)
{
    DWORD old;
    VirtualProtect(base + PAGE, PAGE, PAGE_READWRITE, &old);
    base[PAGE + 256] = (unsigned char)i;
    VirtualProtect(base + PAGE, PAGE, PAGE_EXECUTE_READ, &old);
    FlushInstructionCache(GetCurrentProcess(), base + PAGE, PAGE);
}

static void rwx(int i)
{
    base[PAGE + 256] = (unsigned char)i;
}

static void remap(int i)
{
    DWORD old;
    VirtualFree(base + PAGE, PAGE, MEM_DECOMMIT);
    VirtualAlloc(base + PAGE, PAGE, MEM_COMMIT, PAGE_READWRITE);
    write_code();
    VirtualProtect(base + PAGE, PAGE, PAGE_EXECUTE_READ, &old);
    FlushInstructionCache(GetCurrentProcess(), base, 2 * PAGE);
}

static void unmap(int i)
{
    VirtualFree(base + PAGE, PAGE, MEM_DECOMMIT);
}

typedef int (__fastcall *imm_fn)(int);
#define CHAIN 1000

static int run_imm(int n, int quiet)
{
    unsigned char *c = VirtualAlloc(NULL, PAGE, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE);
    imm_fn f = (imm_fn)c;
    DWORD t0, t1;
    int i, bad = 0;
    if (!c) { printf("pagecross: imm: no memory\n"); return 1; }
    c[0] = 0xb8;                              /* mov eax, imm32 */
    c[5] = 0x85; c[6] = 0xc9;                 /* test ecx, ecx */
    c[7] = 0x75; c[8] = 1;                    /* jnz chain */
    c[9] = 0xc3;                              /* ret */
    for (i = 0; i < CHAIN; i++) { c[10 + 2 * i] = 0x75; c[11 + 2 * i] = 0; }   /* jnz next */
    c[10 + 2 * CHAIN] = 0xc3;
    t0 = GetTickCount();
    for (i = 0; i < n; i++)
    {
        memcpy(c + 1, &i, 4);
        if (f(0) != i) bad++;
    }
    t1 = GetTickCount();
    printf("pagecross: imm %s %d (%lu ms)\n", bad ? "wrong" : "ok", n, t1 - t0);
    Sleep(quiet);
    i = 0x5a5a5a5a;
    memcpy(c + 1, &i, 4);
    for (t0 = 0; t0 < 3; t0++) if (f(0) != i) bad++;
    printf("pagecross: imm after %d ms quiet %s\n", quiet, bad ? "wrong" : "ok");
    return bad;
}

#define BLOCKS 16
static unsigned char *bp;
static volatile LONG bseq, back, bstop, bwrong, bconst;
static volatile int bwant;

/* The second thread: the page's constant function over and over; at each
 * new `bseq`, the rewritten function once, which must give `bwant`. */
static DWORD WINAPI blocks_other(void *arg)
{
    imm_fn f = (imm_fn)bp, g = (imm_fn)(bp + 0x800);
    LONG seen = 0;
    while (!bstop)
    {
        LONG q = bseq;
        if (g(0) != 0x1234) InterlockedIncrement(&bwrong);
        InterlockedIncrement(&bconst);
        if (q != seen)
        {
            if (f(0) != bwant) InterlockedIncrement(&bwrong);
            seen = q;
            InterlockedExchange(&back, q);
        }
    }
    return 0;
}

static int run_blocks(int n)
{
    int imm[BLOCKS] = { 0 }, i, k, sum = 0, bad = 0;
    imm_fn f;
    HANDLE th;
    DWORD t0, t1;
    bp = VirtualAlloc(NULL, PAGE, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE);
    if (!bp) { printf("pagecross: blocks: no memory\n"); return 1; }
    f = (imm_fn)bp;
    bp[0] = 0x31; bp[1] = 0xc0;                           /* xor eax, eax */
    bp[2] = 0xeb; bp[3] = 16 - 4;                         /* jmp block 0 */
    for (k = 0; k < BLOCKS; k++)
    {
        unsigned char *b = bp + 16 * (k + 1);
        b[0] = 0x05; memset(b + 1, 0, 4);                 /* add eax, imm32 */
        b[5] = 0x85; b[6] = 0xc9;                         /* test ecx, ecx */
        b[7] = 0x74; b[8] = 16 - 9;                       /* jz next block */
        b[9] = 0xc3;                                      /* ret */
    }
    bp[16 * (BLOCKS + 1)] = 0xc3;
    bp[0x800] = 0xb8; i = 0x1234; memcpy(bp + 0x801, &i, 4); bp[0x805] = 0xc3;   /* mov eax, 0x1234; ret */
    if (f(0) != 0) bad++;
    bwant = 0;
    th = CreateThread(NULL, 0, blocks_other, NULL, 0, NULL);
    t0 = GetTickCount();
    for (i = 1; i <= n; i++)
    {
        k = i % BLOCKS;
        sum += i - imm[k];
        imm[k] = i;
        memcpy(bp + 16 * (k + 1) + 1, &i, 4);
        if (f(0) != sum) bad++;
        if (i % 8 == 0)
        {
            DWORD w = GetTickCount();
            bwant = sum;
            InterlockedExchange(&bseq, i);
            while (back != i)
                if (GetTickCount() - w > 60000) { printf("pagecross: blocks: the other thread is stuck at %d\n", i); bad++; break; }
        }
    }
    t1 = GetTickCount();
    bstop = 1;
    WaitForSingleObject(th, INFINITE);
    bad += bwrong;
    printf("pagecross: blocks %s %d (%lu ms; other thread %ld constant calls, %ld wrong)\n",
           bad ? "wrong" : "ok", n, t1 - t0, (long)bconst, (long)bwrong);
    return bad;
}

int main(int argc, char **argv)
{
    int n = argc > 1 ? atoi(argv[1]) : 2000, bad = 0;
    int quiet = argc > 2 ? atoi(argv[2]) : 2500;
    DWORD old;

    /* argv[3] "imm": that case alone (to time it against a run of 0) */
    if (argc > 3 && !strcmp(argv[3], "blocks"))
    {
        bad = run_blocks(n);
        printf(bad ? "pagecross: FAIL\n" : "pagecross ok\n");
        return bad != 0;
    }
    if (argc > 3 && !strcmp(argv[3], "imm"))
    {
        bad = run_imm(n, quiet);
        printf(bad ? "pagecross: FAIL\n" : "pagecross ok\n");
        return bad != 0;
    }

    base = VirtualAlloc(NULL, 2 * PAGE, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    if (!base) { printf("pagecross: no memory\n"); return 1; }
    write_code();
    VirtualProtect(base, 2 * PAGE, PAGE_EXECUTE_READ, &old);
    bad += run("protect", n, protect);

    VirtualProtect(base, 2 * PAGE, PAGE_EXECUTE_READWRITE, &old);
    bad += run("rwx", n, rwx);

    VirtualProtect(base, 2 * PAGE, PAGE_EXECUTE_READ, &old);
    bad += run("remap", n, remap);

    AddVectoredExceptionHandler(1, on_fault);
    bad += run("fault", n / 10, unmap);
    if (faults != n / 10) { printf("pagecross: fault saw %ld faults, want %d\n", (long)faults, n / 10); bad++; }

    if (!(argc > 3 && !strcmp(argv[3], "noimm"))) bad += run_imm(n, quiet);
    bad += run_blocks(n);

    printf(bad ? "pagecross: FAIL\n" : "pagecross ok\n");
    return bad != 0;
}
