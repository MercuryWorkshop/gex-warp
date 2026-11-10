// Restartable ranges: code a task registers with the kernel as code a
// thread may be taken out of (task_restartable_ranges_register), each
// with where a thread found in it goes on instead, and the task's word
// that every thread in one be sent there now
// (task_restartable_ranges_synchronize). The Objective-C runtime's
// objc_msgSend is such code, and that is how it knows no thread still
// reads a method cache it is about to free.
//
// A task registers once, and on a Mac its libobjc has, before a program's
// own code runs: so this is not a program the Mac's answers can be had
// for. It says what XNU's source says it does (osfmk/kern/restartable.c).
#include "t.h"

long t_trap8(long n, long a, long b, long c, long d, long e, long f, long g, long h);
// t_spin(words): sets words[0], then turns in a range for as long as
// words[1] is zero. 0 when it left by seeing words[1], 7 from the range's
// recovery address.
long t_spin(volatile int *words);
extern char t_spin_start[], t_spin_end[], t_spin_recover[];
#if defined(__arm64__)
__asm__(".text\n.globl _t_trap8\n.p2align 2\n_t_trap8:\n"
        "  neg x16, x0\n  mov x0, x1\n  mov x1, x2\n  mov x2, x3\n  mov x3, x4\n"
        "  mov x4, x5\n  mov x5, x6\n  mov x6, x7\n  ldr x7, [sp]\n  svc #0x80\n  ret\n");
__asm__(".text\n.globl _t_spin, _t_spin_start, _t_spin_end, _t_spin_recover\n.p2align 2\n_t_spin:\n"
        "  mov w1, #1\n  str w1, [x0]\n"
        "_t_spin_start:\n1:  ldr w1, [x0, #4]\n  cbz w1, 1b\n  mov x0, #0\n  ret\n_t_spin_end:\n"
        "_t_spin_recover:\n  mov x0, #7\n  ret\n");
#elif defined(__x86_64__)
__asm__(".text\n.globl _t_trap8\n_t_trap8:\n"
        "  movq %rdi, %rax\n  orq $0x1000000, %rax\n  movq %rsi, %rdi\n  movq %rdx, %rsi\n"
        "  movq %rcx, %rdx\n  movq %r8, %r10\n  movq %r9, %r8\n  movq 8(%rsp), %r9\n"
        "  movq 16(%rsp), %r11\n  movq %r11, 8(%rsp)\n  movq 24(%rsp), %r11\n  movq %r11, 16(%rsp)\n"
        "  syscall\n  ret\n");
__asm__(".text\n.globl _t_spin, _t_spin_start, _t_spin_end, _t_spin_recover\n_t_spin:\n"
        "  movl $1, (%rdi)\n"
        "_t_spin_start:\n1:  movl 4(%rdi), %eax\n  testl %eax, %eax\n  jz 1b\n  xorl %eax, %eax\n  ret\n_t_spin_end:\n"
        "_t_spin_recover:\n  movl $7, %eax\n  ret\n");
#endif

enum { TRAP_port_mod_refs = 19, TRAP_mach_msg2 = 47 };
enum { RIGHT_RECEIVE = 1, COPY_SEND = 19, MAKE_SEND_ONCE = 21 };
enum { SEND_MSG = 1, RCV_MSG = 2 };
#define KOBJECT_CALL 0x200000000ull

struct message {
    struct {
        u32 bits, size, remote, local, voucher;
        i32 id;
    } h;
    u32 body[40];
};

static u32 task;

// A MIG request to the task's port and its answer's code.
static long ask(struct message *m, u32 size) {
    u32 reply = (u32)TRAP(TRAP_mach_reply_port);
    m->h.bits = COPY_SEND | MAKE_SEND_ONCE << 8;
    m->h.remote = task, m->h.local = reply, m->h.voucher = 0;
    long r = t_trap8(TRAP_mach_msg2, (long)m, (long)(SEND_MSG | RCV_MSG | KOBJECT_CALL), (long)(m->h.bits | (u64)size << 32), (long)(m->h.remote | (u64)m->h.local << 32),
                     (long)((u64)(u32)m->h.id << 32), (long)((u64)reply << 32), (long)sizeof *m, 0);
    TRAP(TRAP_port_mod_refs, task, reply, RIGHT_RECEIVE, -1);
    return r ? r : (long)(i32)m->body[2];
}

struct range {
    u64 location;
    u16 length, recovery;
    u32 flags;
};

static long reg(const struct range *ranges, u32 count) {
    static struct message m;
    t_memset(&m, 0, sizeof m);
    m.h.id = 8000;
    m.body[1] = 1, m.body[2] = count;
    for (u32 i = 0; i < count; i++) {
        u32 *at = m.body + 3 + 4 * i;
        at[0] = (u32)ranges[i].location, at[1] = (u32)(ranges[i].location >> 32);
        at[2] = ranges[i].length | (u32)ranges[i].recovery << 16, at[3] = ranges[i].flags;
    }
    return ask(&m, sizeof m.h + 12 + 16 * count);
}

static long synchronize(void) {
    static struct message m;
    t_memset(&m, 0, sizeof m);
    m.h.id = 8001;
    return ask(&m, sizeof m.h);
}

static void nap(long us) {
    struct t_timeval tv = {0, us};
    SYS(SYS_select, 0, 0, 0, 0, &tv);
}

static volatile int turning[2], working[2], napping[2];

static void *turns(void *arg) { return (void *)t_spin((volatile int *)arg); }

// Counts, in no range, until it is told to stop.
static void *works(void *arg) {
    volatile int *w = arg;
    w[0] = 1;
    long n = 0;
    while (!w[1]) n++;
    return (void *)(n > 0 ? 3l : -1l);
}

// In a system call for a long while.
static void *naps(void *arg) {
    volatile int *w = arg;
    w[0] = 1;
    nap(400000);
    return (void *)5l;
}

static long last;
static void *keep(void *(*fn)(void *), void *arg) {
    last = (long)fn(arg);
    return 0;
}
static void *turns_kept(void *arg) { return keep(turns, arg); }

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    task = (u32)TRAP(TRAP_task_self);
    u16 length = (u16)(t_spin_end - t_spin_start), recovery = (u16)(t_spin_recover - t_spin_start);
    t_show("ask before there are any ranges", synchronize());

    // What is refused, which registers nothing.
    struct range far = {(u64)t_spin_start, length, 5000, 0}, flagged = {(u64)t_spin_start, length, recovery, 1};
    struct range over[2] = {{(u64)t_spin_start, length, recovery, 0}, {(u64)t_spin_start + 4, 4, 4, 0}};
    t_show("a recovery address too far from its range", reg(&far, 1));
    t_show("a range with flags", reg(&flagged, 1));
    t_show("two ranges that overlap", reg(over, 2));
    struct range r = {(u64)t_spin_start, length, recovery, 0};
    t_show("register the range", reg(&r, 1));
    t_show("register again", reg(&r, 1));

    // A thread that turns in the range leaves it by its recovery address
    // when the task asks, and not before.
    int th = t_thread_start(turns_kept, (void *)turning);
    while (!turning[0]) nap(1000);
    nap(30000);
    t_check("a thread turns in the range", last == 0);
    u64 t0 = t_nanos();
    t_show("ask", synchronize());
    t_thread_join(th);
    t_printf("the thread left the range with %ld\n", last);
    t_check("soon", t_nanos() - t0 < 5000000000ull);

    // Threads that are in no range go on as they were: one at work, one
    // in a system call, which the task does not wait for.
    int w = t_thread_start(works, (void *)working), s = t_thread_start(naps, (void *)napping);
    while (!working[0] || !napping[0]) nap(1000);
    nap(20000);
    t0 = t_nanos();
    t_show("ask with one thread at work and one asleep", synchronize());
    t_check("answered before the sleeper wakes", t_nanos() - t0 < 200000000ull);
    t_show("and again", synchronize());
    working[1] = 1;
    t_thread_join(w);
    t_thread_join(s);

    // A thread in the range once more, and one that leaves it by itself.
    last = -1, turning[0] = 0;
    th = t_thread_start(turns_kept, (void *)turning);
    while (!turning[0]) nap(1000);
    t_show("ask again", synchronize());
    t_thread_join(th);
    t_printf("the thread left the range with %ld\n", last);
    last = -1, turning[0] = 0;
    th = t_thread_start(turns_kept, (void *)turning);
    while (!turning[0]) nap(1000);
    turning[1] = 1;
    t_thread_join(th);
    t_printf("a thread nobody asked about left it with %ld\n", last);
    t_show("ask with no other thread", synchronize());
    return 0;
}
