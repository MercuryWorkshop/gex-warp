// Mach ports: names and the rights under them, messages sent and received
// (with a reply port, with a right in the body, through a port set, to a
// thread that waits), the kernel's own ports answering MIG requests, and
// semaphores -- which is how the libc sleeps.
#include "t.h"

// A trap with eight arguments: mach_msg2's.
long t_trap8(long n, long a, long b, long c, long d, long e, long f, long g, long h);
#if defined(__arm64__)
__asm__(".text\n.globl _t_trap8\n.p2align 2\n_t_trap8:\n"
        "  neg x16, x0\n  mov x0, x1\n  mov x1, x2\n  mov x2, x3\n  mov x3, x4\n"
        "  mov x4, x5\n  mov x5, x6\n  mov x6, x7\n  ldr x7, [sp]\n  svc #0x80\n  ret\n");
#elif defined(__x86_64__)
// Six in registers; the kernel reads the seventh and eighth from the stack,
// past the return address.
__asm__(".text\n.globl _t_trap8\n_t_trap8:\n"
        "  movq %rdi, %rax\n  orq $0x1000000, %rax\n  movq %rsi, %rdi\n  movq %rdx, %rsi\n"
        "  movq %rcx, %rdx\n  movq %r8, %r10\n  movq %r9, %r8\n  movq 8(%rsp), %r9\n"
        "  movq 16(%rsp), %r11\n  movq %r11, 8(%rsp)\n  movq 24(%rsp), %r11\n  movq %r11, 16(%rsp)\n"
        "  syscall\n  ret\n");
#endif

enum {
    TRAP_port_allocate = 16, TRAP_port_deallocate = 18, TRAP_port_mod_refs = 19, TRAP_port_insert_right = 21,
    TRAP_port_insert_member = 22, TRAP_port_extract_member = 23, TRAP_semaphore_signal = 33,
    TRAP_semaphore_wait = 36, TRAP_semaphore_timedwait = 38, TRAP_mach_msg2 = 47, TRAP_port_type = 76,
};
enum { RIGHT_SEND = 0, RIGHT_RECEIVE = 1, RIGHT_SEND_ONCE = 2, RIGHT_PORT_SET = 3, RIGHT_DEAD_NAME = 4 };
enum { MOVE_SEND = 17, MOVE_SEND_ONCE = 18, COPY_SEND = 19, MAKE_SEND = 20, MAKE_SEND_ONCE = 21 };
enum { SEND_MSG = 1, RCV_MSG = 2, RCV_LARGE = 4, RCV_TIMEOUT = 0x100 };
#define COMPLEX 0x80000000u
// What mach_msg2 is told of the destination: a queue, or the kernel.
#define MQ_CALL 0x400000000ull
#define KOBJECT_CALL 0x200000000ull
#define RCV_INTERRUPTED 0x10004005

struct header {
    u32 bits, size, remote, local, voucher;
    i32 id;
};
struct port_desc {
    u32 name, pad1;
    u16 pad2;
    u8 disposition, type;
};
// A message with room to receive anything the tests send.
struct message {
    struct header h;
    u32 body[24];
};

static u32 task;

// mach_msg, as libsyscall makes it: the header in registers.
static long msg(struct message *m, u64 option, u32 send_size, u32 rcv_size, u32 rcv_name, u32 timeout) {
    u32 descs = (option & SEND_MSG) && (m->h.bits & COMPLEX) ? m->body[0] : 0;
    long r;
    do {
        r = t_trap8(TRAP_mach_msg2, (long)m, (long)option, (long)(m->h.bits | (u64)send_size << 32),
                    (long)(m->h.remote | (u64)m->h.local << 32), (long)(m->h.voucher | (u64)(u32)m->h.id << 32),
                    (long)(descs | (u64)rcv_name << 32), (long)rcv_size, timeout);
        // The send has been done; an interrupted receive is asked again.
        option &= ~(u64)SEND_MSG;
    } while (r == RCV_INTERRUPTED);
    return r;
}

static long send(struct message *m, u32 size) { return msg(m, SEND_MSG | MQ_CALL, size, 0, 0, 0); }
static long receive(struct message *m, u32 from, u32 room) { return msg(m, RCV_MSG | RCV_TIMEOUT, 0, room, from, 0); }

static u32 allocate(int right) {
    u32 name = 0;
    long r = TRAP(TRAP_port_allocate, task, right, &name);
    return r ? 0 : name;
}

static void show_type(const char *what, u32 name) {
    u32 type = 0x55555555;
    long r = TRAP(TRAP_port_type, task, name, &type);
    if (r) t_printf("%s: no type, %ld\n", what, r);
    else t_printf("%s: type %x\n", what, type);
}

static void rights(void) {
    t_check("task, host and thread have names", task && TRAP(TRAP_host_self) && TRAP(TRAP_thread_self));
    show_type("the task's port", task);
    u32 p = 0;
    t_show("allocate a receive right", TRAP(TRAP_port_allocate, task, RIGHT_RECEIVE, &p));
    t_check("a name", p != 0 && p != task);
    show_type("it", p);
    t_show("make a send right from it", TRAP(TRAP_port_insert_right, task, p, p, MAKE_SEND));
    show_type("now", p);
    t_show("another reference", TRAP(TRAP_port_mod_refs, task, p, RIGHT_SEND, 1));
    t_show("deallocate one", TRAP(TRAP_port_deallocate, task, p));
    t_show("and the other", TRAP(TRAP_port_deallocate, task, p));
    show_type("the receive right alone", p);
    t_show("deallocate with none left", TRAP(TRAP_port_deallocate, task, p));
    t_show("drop the receive right", TRAP(TRAP_port_mod_refs, task, p, RIGHT_RECEIVE, -1));
    show_type("the name", p);

    t_show("allocate a right there is not", TRAP(TRAP_port_allocate, task, 99, &p));
    t_show("deallocate a name there is not", TRAP(TRAP_port_deallocate, task, 0x123457));
    t_show("mod_refs of one", TRAP(TRAP_port_mod_refs, task, 0x123457, RIGHT_SEND, 1));
    show_type("a name there is not", 0x123457);
    t_show("allocate in no task", TRAP(TRAP_port_allocate, 0, RIGHT_RECEIVE, &p));

    // A port whose receive right goes while a send right is held: the name
    // is a dead one.
    p = allocate(RIGHT_RECEIVE);
    TRAP(TRAP_port_insert_right, task, p, p, MAKE_SEND);
    t_show("drop a receive right with a sender", TRAP(TRAP_port_mod_refs, task, p, RIGHT_RECEIVE, -1));
    show_type("the name", p);
    static struct message m;
    m.h = (struct header){COPY_SEND, 0, p, 0, 0, 7};
    t_show("send to it", send(&m, sizeof m.h));
    t_show("deallocate the dead name", TRAP(TRAP_port_deallocate, task, p));
    show_type("and then", p);
}

static void messages(void) {
    static struct message m, got;
    u32 p = allocate(RIGHT_RECEIVE), reply = allocate(RIGHT_RECEIVE);
    TRAP(TRAP_port_insert_right, task, p, p, MAKE_SEND);

    t_show("receive with nothing there", receive(&got, p, sizeof got));
    u64 t0 = t_nanos();
    t_show("and for 30 ms", msg(&got, RCV_MSG | RCV_TIMEOUT, 0, sizeof got, p, 30));
    t_check("it waited", t_nanos() - t0 >= 30000000ull);
    t_show("receive from no name", receive(&got, 0x123457, sizeof got));
    m.h = (struct header){COPY_SEND, 0, 0x123457, 0, 0, 1};
    t_show("send to no name", send(&m, sizeof m.h));
    m.h = (struct header){COPY_SEND, 0, reply, 0, 0, 1};
    t_show("send by a right not held", send(&m, sizeof m.h));

    // A request, with a reply port: a send-once right made from `reply`.
    m.h = (struct header){COPY_SEND | MAKE_SEND_ONCE << 8, 0, p, reply, 0, 1234};
    m.body[0] = 0xfeedface;
    t_show("send", send(&m, sizeof m.h + 4));
    t_memset(&got, 0x55, sizeof got);
    t_show("receive", receive(&got, p, sizeof got));
    t_printf("  bits %x, %u bytes, id %d, body %x; trailer type %u, %u bytes\n", got.h.bits, got.h.size, got.h.id, got.body[0], got.body[1],
             got.body[2]);
    t_check("on the port it was sent to", got.h.local == p);
    t_check("with a new name to reply by", got.h.remote != 0 && got.h.remote != reply && got.h.remote != p);
    show_type("which", got.h.remote);
    u32 once = got.h.remote;
    m.h = (struct header){MOVE_SEND_ONCE, 0, once, 0, 0, 1334};
    t_show("reply by it", send(&m, sizeof m.h));
    show_type("the name, used", once);
    t_show("receive the reply", receive(&got, reply, sizeof got));
    t_printf("  bits %x, %u bytes, id %d, %s\n", got.h.bits, got.h.size, got.h.id, got.h.local == reply ? "on the reply port" : "elsewhere");

    // Too large for the room: gone, or with RCV_LARGE kept and measured.
    m.h = (struct header){COPY_SEND, 0, p, 0, 0, 2};
    send(&m, sizeof m.h + 64);
    t_show("receive 88 bytes into 40", receive(&got, p, 40));
    t_show("and again", receive(&got, p, sizeof got));
    send(&m, sizeof m.h + 64);
    t_memset(&got, 0, sizeof got);
    t_show("with RCV_LARGE", msg(&got, RCV_MSG | RCV_TIMEOUT | RCV_LARGE, 0, 40, p, 0));
    t_printf("  it would be %u bytes\n", got.h.size);
    t_show("and again, with room", receive(&got, p, sizeof got));
    t_printf("  %u bytes, id %d\n", got.h.size, got.h.id);

    // A right in the body: a send right to `reply`, made as it is sent.
    m.h = (struct header){COMPLEX | COPY_SEND, 0, p, 0, 0, 3};
    m.body[0] = 1;
    struct port_desc *d = (struct port_desc *)&m.body[1];
    *d = (struct port_desc){reply, 0, 0, MAKE_SEND, 0};
    t_show("send a right", send(&m, sizeof m.h + 4 + sizeof *d));
    t_memset(&got, 0, sizeof got);
    t_show("receive it", receive(&got, p, sizeof got));
    d = (struct port_desc *)&got.body[1];
    t_printf("  bits %x, %u descriptor, disposition %d, type %d, %s\n", got.h.bits, got.body[0], d->disposition, d->type,
             d->name == reply ? "under the name the port has here" : "under another name");
    show_type("that port", reply);
    m.h = (struct header){COMPLEX | COPY_SEND, 0, p, 0, 0, 3};
    *(struct port_desc *)&m.body[1] = (struct port_desc){0x123457, 0, 0, COPY_SEND, 0};
    t_show("send a right not held", send(&m, sizeof m.h + 4 + sizeof *d));

    // Through a port set.
    u32 set = allocate(RIGHT_PORT_SET);
    show_type("a port set", set);
    t_show("put the port in it", TRAP(TRAP_port_insert_member, task, p, set));
    m.h = (struct header){COPY_SEND, 0, p, 0, 0, 4};
    send(&m, sizeof m.h);
    t_show("receive from the port itself", receive(&got, p, sizeof got));
    t_show("receive from the set", receive(&got, set, sizeof got));
    t_printf("  id %d, %s\n", got.h.id, got.h.local == p ? "from the member" : "from elsewhere");
    t_show("send to the set", ({ m.h = (struct header){COPY_SEND, 0, set, 0, 0, 4}; send(&m, sizeof m.h); }));
    t_show("take it out", TRAP(TRAP_port_extract_member, task, p, set));
    t_show("put a name that is no port in", TRAP(TRAP_port_insert_member, task, 0x123457, set));
}

static u32 shared_port, shared_sem;

static void *receiver(void *arg) {
    static struct message got;
    long r = msg(&got, RCV_MSG, 0, sizeof got, shared_port, 0);
    *(long *)arg = r ? -1 : got.h.id;
    return 0;
}

static void *waiter(void *arg) {
    long r;
    do r = TRAP(TRAP_semaphore_wait, shared_sem);
    while (r == 14);
    *(long *)arg = r;
    return 0;
}

static void napms(int ms) {
    struct t_timeval tv = {0, ms * 1000};
    SYS(SYS_select, 0, 0, 0, 0, &tv);
}

static void threads(void) {
    static struct message m;
    shared_port = allocate(RIGHT_RECEIVE);
    TRAP(TRAP_port_insert_right, task, shared_port, shared_port, MAKE_SEND);
    long got = -2;
    int th = t_thread_start(receiver, &got);
    napms(30);
    t_check("a thread waits for a message", got == -2);
    m.h = (struct header){COPY_SEND, 0, shared_port, 0, 0, 77};
    t_show("send it one", send(&m, sizeof m.h));
    t_thread_join(th);
    t_printf("it received id %ld\n", got);
}

// A MIG request to one of the kernel's ports and its reply, through a reply
// port of this thread's.
static long rpc(struct message *m, u32 to, u32 size) {
    u32 reply = (u32)TRAP(TRAP_mach_reply_port);
    m->h.bits = (m->h.bits & COMPLEX) | COPY_SEND | MAKE_SEND_ONCE << 8;
    m->h.remote = to, m->h.local = reply, m->h.voucher = 0;
    long r = msg(m, SEND_MSG | RCV_MSG | KOBJECT_CALL, size, sizeof *m, reply, 0);
    TRAP(TRAP_port_mod_refs, task, reply, RIGHT_RECEIVE, -1);
    return r;
}

static void kernel_ports(void) {
    static struct message m;
    // NDR record, then the arguments.
    static const u32 ndr[2] = {0, 1};
    u32 host = (u32)TRAP(TRAP_host_self);

    // _host_page_size(host, out size)
    t_memset(&m, 0, sizeof m);
    m.h.id = 202;
    t_show("ask the host its page size", rpc(&m, host, sizeof m.h));
    // 16 KiB on arm64. An x86-64 process has 4 KiB pages, but Rosetta, its
    // reference here, answers with the Mac's under it.
#if defined(__arm64__)
    int page_ok = m.body[3] == 16384;
#else
    int page_ok = m.body[3] == 4096 || m.body[3] == 16384;
#endif
    t_printf("  reply %d, %u bytes, bits %x: code %d, %s\n", m.h.id, m.h.size, m.h.bits, m.body[2], page_ok ? "a page's size" : "something else");

    // host_info(host, flavor, out info, in-out count): what libpthread
    // asks of the scheduler's priorities as it starts, and what a program
    // asks of the machine.
    static const struct {
        const char *what;
        u32 flavor, room;
    } infos[] = {
        {"its priorities", 5, 8}, {"them, with too little room", 5, 3}, {"them, with more than enough", 5, 64},
        {"the machine", 1, 12},   {"the machine as it was once told", 1, 5}, {"it, with too little room", 1, 4},
        {"its scheduler", 3, 2},  {"its preferred architecture", 12, 2},    {"whether it has the message trap", 8, 0},
        {"a flavor there is none of", 99, 16},
    };
    for (unsigned i = 0; i < sizeof infos / sizeof *infos; i++) {
        t_memset(&m, 0, sizeof m);
        m.h.id = 200;
        m.body[0] = ndr[0], m.body[1] = ndr[1], m.body[2] = infos[i].flavor, m.body[3] = infos[i].room;
        t_printf("ask the host %s", infos[i].what);
        long r = rpc(&m, host, sizeof m.h + 16);
        t_printf(": %ld, reply %d, %u bytes, code %d", r, m.h.id, m.h.size, m.body[2]);
        u32 *w = &m.body[4];
        if (m.body[2] != 0) {
            t_printf("\n");
            continue;
        }
        t_printf(", %u words\n", m.body[3]);
        if (infos[i].flavor == 5) {
            t_printf(" ");
            for (u32 k = 0; k < m.body[3]; k++) t_printf(" %u", w[k]);
            t_printf("\n");
        } else if (infos[i].flavor == 1) {
            // The processors and the memory are the machine's own.
            int ok = w[0] >= 1 && w[1] >= 1 && w[1] <= w[0] && w[2] > 0;
            if (m.body[3] == 12) ok = ok && w[6] >= 1 && w[9] >= w[6] && (w[10] | w[11]) != 0;
#if defined(__arm64__)
            t_printf("  a processor of type %x, %u: %s\n", w[3], w[4], ok ? "some processors and memory" : "nothing");
#else
            // (Rosetta says the subtype it is itself.)
            t_printf("  a processor of type %x: %s\n", w[3], ok ? "some processors and memory" : "nothing");
#endif
        } else if (infos[i].flavor == 3) {
            t_printf("  %u %u\n", w[0], w[1]);
        } else if (infos[i].flavor == 12) {
            t_printf("  type %x\n", w[0]);
        }
    }

    // A routine no port has.
    t_memset(&m, 0, sizeof m);
    m.h.id = 9999;
    t_show("ask the task for nothing it does", rpc(&m, task, sizeof m.h));
    t_printf("  reply %d, %u bytes: code %d\n", m.h.id, m.h.size, m.body[2]);

    // host_get_clock_service(host, SYSTEM_CLOCK, out clock), clock_get_time.
    t_memset(&m, 0, sizeof m);
    m.h.id = 206;
    m.body[0] = ndr[0], m.body[1] = ndr[1], m.body[2] = 0;
    t_show("ask the host for its clock", rpc(&m, host, sizeof m.h + 12));
    struct port_desc *d = (struct port_desc *)&m.body[1];
    t_printf("  reply %d, %u bytes, bits %x: %u descriptor, disposition %d\n", m.h.id, m.h.size, m.h.bits, m.body[0], d->disposition);
    u32 clock = d->name;
    show_type("the clock", clock);
    t_memset(&m, 0, sizeof m);
    m.h.id = 1000;
    t_show("ask it the time", rpc(&m, clock, sizeof m.h));
    t_printf("  reply %d, %u bytes: code %d, %s\n", m.h.id, m.h.size, m.body[2], m.body[3] > 0 && m.body[4] < 1000000000 ? "a time" : "no time");

    // semaphore_create(task, out semaphore, policy, value): one, already
    // signalled once.
    t_memset(&m, 0, sizeof m);
    m.h.id = 3418;
    m.body[0] = ndr[0], m.body[1] = ndr[1], m.body[2] = 0, m.body[3] = 1;
    t_show("ask the task for a semaphore", rpc(&m, task, sizeof m.h + 16));
    t_printf("  reply %d, %u bytes, bits %x: %u descriptor, disposition %d\n", m.h.id, m.h.size, m.h.bits, m.body[0], d->disposition);
    u32 sem = d->name;
    show_type("the semaphore", sem);
    t_show("wait on it", TRAP(TRAP_semaphore_wait, sem));
    u64 t0 = t_nanos();
    t_show("and again, for 30 ms", TRAP(TRAP_semaphore_timedwait, sem, 0, 30000000));
    t_check("it waited", t_nanos() - t0 >= 30000000ull);
    t_show("signal it", TRAP(TRAP_semaphore_signal, sem));
    t_show("wait", TRAP(TRAP_semaphore_timedwait, sem, 0, 30000000));
    t_show("signal what is no semaphore", TRAP(TRAP_semaphore_signal, task));
    t_show("wait on it", TRAP(TRAP_semaphore_wait, 0x123457));

    shared_sem = sem;
    long got = -2;
    int th = t_thread_start(waiter, &got);
    napms(30);
    t_check("a thread waits on the semaphore", got == -2);
    t_show("signal it", TRAP(TRAP_semaphore_signal, sem));
    t_thread_join(th);
    t_printf("its wait answered %ld\n", got);

    // How the libc sleeps: __semwait_signal on a semaphore nobody signals,
    // for a time from now.
    t0 = t_nanos();
    t_show("__semwait_signal for 40 ms", SYS(SYS___semwait_signal, sem, 0, 1, 1, 0, 40000000));
    t_check("it slept", t_nanos() - t0 >= 40000000ull && t_nanos() - t0 < 2000000000ull);
    t_show("__semwait_signal on no semaphore", SYS(SYS___semwait_signal, 0, 0, 1, 1, 0, 1000));

    // semaphore_destroy(task, semaphore)
    t_memset(&m, 0, sizeof m);
    m.h.id = 3419;
    m.h.bits = COMPLEX;
    m.body[0] = 1;
    *d = (struct port_desc){sem, 0, 0, COPY_SEND, 0};
    t_show("destroy it", rpc(&m, task, sizeof m.h + 4 + sizeof *d));
    t_printf("  reply %d, %u bytes: code %d\n", m.h.id, m.h.size, m.body[2]);
    t_show("signal it now", TRAP(TRAP_semaphore_signal, sem));

    // mach_port_extract_right(task, name, how): a right made from one of
    // the task's own and handed back in the reply, which is how a library
    // gets a send-once right to a port it holds only the receive right of.
    u32 port = allocate(RIGHT_RECEIVE);
    static const struct {
        const char *what;
        u32 how;
        int named;
    } takes[] = {
        {"a send-once right made of it", MAKE_SEND_ONCE, 1}, {"a send right made of it", MAKE_SEND, 1},
        {"a right of no name", MAKE_SEND, 0},                 {"a right in no such way", 121, 1},
    };
    u32 once = 0;
    for (unsigned i = 0; i < sizeof takes / sizeof *takes; i++) {
        t_memset(&m, 0, sizeof m);
        m.h.id = 3215;
        m.body[0] = ndr[0], m.body[1] = ndr[1], m.body[2] = takes[i].named ? port : 0x123457, m.body[3] = takes[i].how;
        t_printf("ask the task for %s", takes[i].what);
        long r = rpc(&m, task, sizeof m.h + 16);
        if (m.h.bits & COMPLEX) {
            t_printf(": %ld, reply %d, %u bytes, disposition %d, %s\n", r, m.h.id, m.h.size, d->disposition,
                     d->name == port ? "the port's own name" : d->name ? "a name of its own" : "no name");
            if (takes[i].how == MAKE_SEND_ONCE) once = d->name;
        } else {
            t_printf(": %ld, reply %d, %u bytes, code %d\n", r, m.h.id, m.h.size, m.body[2]);
        }
    }
    show_type("the port", port);
    show_type("the send-once right", once);
    t_memset(&m, 0, sizeof m);
    m.h.bits = MOVE_SEND_ONCE, m.h.remote = once, m.h.id = 77, m.body[0] = 4242;
    t_show("send by it", send(&m, sizeof m.h + 4));
    t_memset(&m, 0, sizeof m);
    t_show("receive", receive(&m, port, sizeof m));
    t_printf("  id %d, %u\n", m.h.id, m.body[0]);
    show_type("the send-once right, used", once);
}

#if defined(__arm64__)
// A timer: a port of the task's that the kernel sends a message to at a
// moment on mach_absolute_time's clock, which is what a run loop waits
// on. (Not for x86-64, where the Mac these are checked against is Rosetta
// and its clock is not the kernel's.)
enum { TRAP_mk_timer_create = 91, TRAP_mk_timer_destroy = 92, TRAP_mk_timer_arm = 93, TRAP_mk_timer_cancel = 94 };

static void timers(void) {
    struct {
        u32 numer, denom;
    } base = {1, 1};
    TRAP(TRAP_mach_timebase_info, &base);
    u64 ms = 1000000ull * base.denom / base.numer;
    u32 timer = (u32)TRAP(TRAP_mk_timer_create);
    show_type("a timer", timer);
    struct message got;
    t_show("nothing yet", receive(&got, timer, sizeof got));
    u64 t0 = (u64)TRAP(TRAP_mach_absolute_time);
    t_show("arm it for 10 ms on", TRAP(TRAP_mk_timer_arm, timer, t0 + 10 * ms));
    t_show("too soon", receive(&got, timer, sizeof got));
    got.h.id = 77;
    long r = msg(&got, RCV_MSG | RCV_TIMEOUT, 0, sizeof got, timer, 2000);
    u64 took = ((u64)TRAP(TRAP_mach_absolute_time) - t0) / ms;
    t_printf("its message: %ld, id %d, %u bytes, for the timer %d\n", r, got.h.id, got.h.size, got.h.local == timer);
    t_check("  at about that moment", took >= 5 && took < 10000);
    t_show("once", receive(&got, timer, sizeof got));
    u64 was = 99;
    t_show("cancel it, not armed", TRAP(TRAP_mk_timer_cancel, timer, &was));
    t_printf("  armed for %lu\n", (unsigned long)was);
    u64 when = (u64)TRAP(TRAP_mach_absolute_time) + 60000 * ms;
    t_show("arm it for a minute on", TRAP(TRAP_mk_timer_arm, timer, when));
    t_show("cancel it", TRAP(TRAP_mk_timer_cancel, timer, &was));
    t_check("  which says it was armed", was != 0);
    t_show("and nothing comes for 30 ms", msg(&got, RCV_MSG | RCV_TIMEOUT, 0, sizeof got, timer, 30));
    // In a set, waited on for ever: the wait ends at the timer's moment.
    u32 set = allocate(RIGHT_PORT_SET);
    t_show("put it in a set", TRAP(TRAP_port_insert_member, task, timer, set));
    t0 = (u64)TRAP(TRAP_mach_absolute_time);
    TRAP(TRAP_mk_timer_arm, timer, t0 + 10 * ms);
    r = msg(&got, RCV_MSG, 0, sizeof got, set, 0);
    took = ((u64)TRAP(TRAP_mach_absolute_time) - t0) / ms;
    t_printf("through the set: %ld, for the timer %d\n", r, got.h.local == timer);
    t_check("  at about that moment", took >= 5 && took < 10000);
    // Armed for a moment already gone: as soon as may be.
    TRAP(TRAP_mk_timer_arm, timer, t0);
    t_show("armed for the past", msg(&got, RCV_MSG | RCV_TIMEOUT, 0, sizeof got, set, 2000));
    t_show("arm what is no timer", TRAP(TRAP_mk_timer_arm, set, t0));
    t_show("destroy it", TRAP(TRAP_mk_timer_destroy, timer));
    show_type("and it is gone", timer);
    t_show("arm it gone", TRAP(TRAP_mk_timer_arm, timer, t0));
}
#endif

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    task = (u32)TRAP(TRAP_task_self);
    rights();
    messages();
    threads();
    kernel_ports();
#if defined(__arm64__)
    timers();
#endif
    return 0;
}
