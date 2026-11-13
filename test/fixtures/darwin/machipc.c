// Mach between two tasks. The parent makes a port its bootstrap port
// (task_set_special_port) and forks; the child finds it as its own
// (task_get_special_port, inherited across the fork) and sends the parent a
// message there with mach_msg_trap, the trap 10.14's libSystem sends with:
// a send right to a port of the child's in the body, memory out of line, a
// send-once right to reply by. The parent receives it (the trailer says who
// sent it), asks to be told when the child's port dies and when its own
// second port has no senders, and replies by the send-once right with a
// send right to that second port, which the child sends to. Then the child
// ends, and with it its rights: the parent is told its name for the
// child's port is a dead name, and its second port has no senders left.
// Only the parent prints; the child says how it went by its exit status.
#include "t.h"

// A trap with eight arguments: mach_msg2's.
long t_trap8(long n, long a, long b, long c, long d, long e, long f, long g, long h);
#if defined(__arm64__)
__asm__(".text\n.globl _t_trap8\n.p2align 2\n_t_trap8:\n"
        "  neg x16, x0\n  mov x0, x1\n  mov x1, x2\n  mov x2, x3\n  mov x3, x4\n"
        "  mov x4, x5\n  mov x5, x6\n  mov x6, x7\n  ldr x7, [sp]\n  svc #0x80\n  ret\n");
#elif defined(__x86_64__)
__asm__(".text\n.globl _t_trap8\n_t_trap8:\n"
        "  movq %rdi, %rax\n  orq $0x1000000, %rax\n  movq %rsi, %rdi\n  movq %rdx, %rsi\n"
        "  movq %rcx, %rdx\n  movq %r8, %r10\n  movq %r9, %r8\n  movq 8(%rsp), %r9\n"
        "  movq 16(%rsp), %r11\n  movq %r11, 8(%rsp)\n  movq 24(%rsp), %r11\n  movq %r11, 16(%rsp)\n"
        "  syscall\n  ret\n");
#endif

enum {
    TRAP_port_allocate = 16, TRAP_port_mod_refs = 19, TRAP_port_guard = 41, TRAP_port_unguard = 42, TRAP_mach_msg = 31, TRAP_mach_msg2 = 47, TRAP_port_type = 76,
};
enum { RIGHT_SEND = 0, RIGHT_RECEIVE = 1 };
enum { MOVE_SEND = 17, MOVE_SEND_ONCE = 18, COPY_SEND = 19, MAKE_SEND = 20, MAKE_SEND_ONCE = 21 };
enum { SEND_MSG = 1, RCV_MSG = 2, RCV_TIMEOUT = 0x100 };
#define COMPLEX 0x80000000u
#define MQ_CALL 0x400000000ull
#define KOBJECT_CALL 0x200000000ull
#define RCV_INTERRUPTED 0x10004005
// A trailer with the sender's audit token (MACH_RCV_TRAILER_AUDIT).
#define TRAILER_AUDIT (3u << 24)
#define TYPE_DEAD_NAME 0x100000u
#define TYPE_SEND 0x10000u

struct header {
    u32 bits, size, remote, local, voucher;
    i32 id;
};
struct port_desc {
    u32 name, pad1;
    u16 pad2;
    u8 disposition, type;
};
struct ool_desc {
    u64 address;
    u8 deallocate, copy, pad, type;
    u32 size;
} __attribute__((packed));
struct message {
    struct header h;
    u32 body[64];
};

static u32 task;
static const u32 ndr[2] = {0, 1};

// mach_msg2, as libsyscall makes it: the header in registers.
static long msg2(struct message *m, u64 option, u32 send_size, u32 rcv_size, u32 rcv_name, u32 timeout) {
    u32 descs = (option & SEND_MSG) && (m->h.bits & COMPLEX) ? m->body[0] : 0;
    long r;
    do {
        r = t_trap8(TRAP_mach_msg2, (long)m, (long)option, (long)(m->h.bits | (u64)send_size << 32),
                    (long)(m->h.remote | (u64)m->h.local << 32), (long)(m->h.voucher | (u64)(u32)m->h.id << 32),
                    (long)(descs | (u64)rcv_name << 32), (long)rcv_size, timeout);
        option &= ~(u64)SEND_MSG;
    } while (r == RCV_INTERRUPTED);
    return r;
}

// mach_msg_trap, the older trap: the message and its sizes as arguments,
// which is how 10.14's libSystem sends. A Mac of today ends a program that
// makes it (the guard exception for a trap no longer to be used), so the
// Mac's build sends the same message with mach_msg2.
static long msg1(struct message *m, u32 option, u32 send_size, u32 rcv_size, u32 rcv_name, u32 timeout) {
#ifndef T_STATIC
    return msg2(m, option | MQ_CALL, send_size, rcv_size, rcv_name, timeout);
#endif
    long r;
    do {
        r = TRAP(TRAP_mach_msg, m, option, send_size, rcv_size, rcv_name, timeout);
        option &= ~(u32)SEND_MSG;
    } while (r == RCV_INTERRUPTED);
    return r;
}

static u32 allocate(int right) {
    u32 name = 0;
    return TRAP(TRAP_port_allocate, task, right, &name) ? 0 : name;
}

static u32 type_of(u32 name) {
    u32 type = 0;
    TRAP(TRAP_port_type, task, name, &type);
    return type;
}

// A MIG request to the task's port, and its reply.
static long rpc(struct message *m, u32 size) {
    u32 reply = (u32)TRAP(TRAP_mach_reply_port);
    m->h.bits = (m->h.bits & COMPLEX) | COPY_SEND | MAKE_SEND_ONCE << 8;
    m->h.remote = task, m->h.local = reply, m->h.voucher = 0;
    long r = msg2(m, SEND_MSG | RCV_MSG | KOBJECT_CALL, size, sizeof *m, reply, 0);
    TRAP(TRAP_port_mod_refs, task, reply, RIGHT_RECEIVE, -1);
    return r;
}

// task_set_special_port(task, which, port), the port made a send right
// from the caller's receive right.
static long set_special(u32 which, u32 port) {
    static struct message m;
    t_memset(&m, 0, sizeof m);
    m.h.id = 3410, m.h.bits = COMPLEX;
    m.body[0] = 1;
    *(struct port_desc *)(m.body + 1) = (struct port_desc){port, 0, 0, MAKE_SEND, 0};
    m.body[4] = ndr[0], m.body[5] = ndr[1], m.body[6] = which;
    long r = rpc(&m, sizeof m.h + 28);
    return r ? r : (long)m.body[2];
}

// task_get_special_port(task, which): the name the right arrives under.
static u32 get_special(u32 which) {
    static struct message m;
    t_memset(&m, 0, sizeof m);
    m.h.id = 3409;
    m.body[0] = ndr[0], m.body[1] = ndr[1], m.body[2] = which;
    if (rpc(&m, sizeof m.h + 12) || !(m.h.bits & COMPLEX)) return 0;
    return ((struct port_desc *)(m.body + 1))->name;
}

// mach_port_get_context(task, name): 0 for a failure, as for none.
static u64 get_context(u32 name) {
    static struct message m;
    t_memset(&m, 0, sizeof m);
    m.h.id = 3228;
    m.body[0] = ndr[0], m.body[1] = ndr[1], m.body[2] = name;
    if (rpc(&m, sizeof m.h + 12) || m.body[2]) return 0;
    u64 c;
    t_memcpy(&c, m.body + 3, 8);
    return c;
}

// mach_port_request_notification(task, name, which, sync, notify).
static long ask_notice(u32 name, u32 which, u32 sync, u32 to) {
    static struct message m;
    t_memset(&m, 0, sizeof m);
    m.h.id = 3213, m.h.bits = COMPLEX;
    m.body[0] = 1;
    *(struct port_desc *)(m.body + 1) = (struct port_desc){to, 0, 0, MAKE_SEND_ONCE, 0};
    m.body[4] = ndr[0], m.body[5] = ndr[1], m.body[6] = name, m.body[7] = which, m.body[8] = sync;
    long r = rpc(&m, sizeof m.h + 36);
    return r ? r : (m.h.bits & COMPLEX) ? 0 : (long)m.body[2];
}

static const char said[] = "a page of the child's, carried out of line";

static int child(void) {
    int status = 0;
    // (Its own task's name: as the libc asks again after a fork.)
    task = (u32)TRAP(TRAP_task_self);
    u32 boot = get_special(4);
    if (!boot) return 1;
    u32 mine = allocate(RIGHT_RECEIVE);
    // To the bootstrap port: a send right to `mine`, the memory, a word;
    // and a send-once right made from `mine` to reply by.
    static struct message m;
    t_memset(&m, 0, sizeof m);
    m.h.bits = COMPLEX | COPY_SEND | MAKE_SEND_ONCE << 8, m.h.remote = boot, m.h.local = mine, m.h.id = 100;
    m.body[0] = 2;
    *(struct port_desc *)(m.body + 1) = (struct port_desc){mine, 0, 0, MAKE_SEND, 0};
    *(struct ool_desc *)(m.body + 4) = (struct ool_desc){(u64)said, 0, 1, 0, 1, sizeof said};
    m.body[8] = 42;
    if (msg1(&m, SEND_MSG, sizeof m.h + 36, 0, 0, 0)) status |= 2;
    // The reply, to `mine`: a send right to the parent's second port.
    t_memset(&m, 0, sizeof m);
    if (msg2(&m, RCV_MSG | RCV_TIMEOUT, 0, sizeof m, mine, 10000)) return status | 4;
    if (m.h.id != 200) status |= 8;
    struct port_desc *d = (struct port_desc *)(m.body + 1);
    if (!(m.h.bits & COMPLEX) || m.body[0] != 1 || !d->name || d->disposition != MOVE_SEND) return status | 16;
    if (m.body[4] != 7) status |= 32;
    // Sent to by the right it was given, which stays the child's.
    u32 second = d->name;
    t_memset(&m, 0, sizeof m);
    m.h.bits = COPY_SEND, m.h.remote = second, m.h.id = 300;
    if (msg2(&m, SEND_MSG | MQ_CALL, sizeof m.h, 0, 0, 0)) status |= 64;
    return status;
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    task = (u32)TRAP(TRAP_task_self);
    u32 boot = allocate(RIGHT_RECEIVE);
    t_printf("set the bootstrap port: %ld\n", set_special(4, boot));
    u32 back = get_special(4);
    t_printf("read back: %s\n", back == boot ? "the same name" : back ? "another name" : "none");
    u32 note = allocate(RIGHT_RECEIVE);

    long pid = 0, child_flag = -1;
    pid = t_syscall2(&child_flag, SYS_fork, 0, 0, 0);
    if (pid < 0) {
        t_printf("fork failed: %ld\n", pid);
        return 1;
    }
    if (child_flag) t_exit(child());

    // What the child sent to the bootstrap port, with who sent it.
    static struct message m;
    t_memset(&m, 0, sizeof m);
    long r = msg1(&m, RCV_MSG | RCV_TIMEOUT | TRAILER_AUDIT, 0, sizeof m, boot, 10000);
    t_printf("received: %ld, id %d, %s, %u descriptors\n", r, m.h.id, (m.h.bits & COMPLEX) ? "complex" : "simple", m.body[0]);
    t_printf("its reply right: %s, disposition %u\n", m.h.remote ? "a name" : "none", m.h.bits & 0x1f);
    struct port_desc *d = (struct port_desc *)(m.body + 1);
    struct ool_desc *o = (struct ool_desc *)(m.body + 4);
    t_printf("a port: disposition %u, %s\n", d->disposition, d->name ? "a name" : "none");
    t_printf("out of line: %u bytes, %s\n", o->size,
             o->address && !t_memcmp((void *)o->address, said, sizeof said) ? "what the child said" : "something else");
    t_printf("the word: %u\n", m.body[8]);
    // The trailer: after the message, its type and size, a sequence
    // number, the sender's security token, and its audit token, whose
    // sixth word is its pid.
    u32 *trailer = (u32 *)((char *)&m + ((m.h.size + 3) & ~3u));
    t_printf("trailer: %u bytes, sent by %s\n", trailer[1], trailer[5 + 5] == (u32)pid ? "the child" : "someone else");
    u32 theirs = d->name, reply = m.h.remote;
    t_printf("their port: type %x\n", type_of(theirs));

    // To be told when it dies, and when a second port of ours has no
    // senders (once it has had one).
    u32 second = allocate(RIGHT_RECEIVE);
    t_printf("ask for a dead name: %ld\n", ask_notice(theirs, 72, 0, note));
    t_printf("ask for no senders: %ld\n", ask_notice(second, 70, 1, note));

    // The reply, by the send-once right: a send right to the second port.
    t_memset(&m, 0, sizeof m);
    m.h.bits = COMPLEX | MOVE_SEND_ONCE, m.h.remote = reply, m.h.id = 200;
    m.body[0] = 1;
    *(struct port_desc *)(m.body + 1) = (struct port_desc){second, 0, 0, MAKE_SEND, 0};
    m.body[4] = 7;
    t_printf("reply: %ld\n", msg2(&m, SEND_MSG | MQ_CALL, sizeof m.h + 20, 0, 0, 0));

    // The child sends on the right it was given.
    t_memset(&m, 0, sizeof m);
    r = msg2(&m, RCV_MSG | RCV_TIMEOUT, 0, sizeof m, second, 10000);
    t_printf("on the second port: %ld, id %d\n", r, m.h.id);

    int status = -1;
    long got = SYS(SYS_wait4, pid, &status, 0, 0);
    t_printf("the child: %s, status %x\n", got == pid ? "waited for" : "not waited for", status);

    // What the child's end tells: its port is dead, and the second port
    // has no senders. (In whichever order they come.)
    int dead = 0, none = 0;
    for (int i = 0; i < 2; i++) {
        t_memset(&m, 0, sizeof m);
        if (msg2(&m, RCV_MSG | RCV_TIMEOUT, 0, sizeof m, note, 5000)) break;
        if (m.h.id == 72 && m.body[2] == theirs) dead++;
        if (m.h.id == 70 && m.body[2] == 1) none++;
    }
    t_printf("told: %d dead name, %d no senders\n", dead, none);
    u32 t = type_of(theirs);
    t_printf("their port now: %s\n", (t & TYPE_DEAD_NAME) && !(t & TYPE_SEND) ? "a dead name" : "something else");
    // Sending to a dead name.
    t_memset(&m, 0, sizeof m);
    m.h.bits = COPY_SEND, m.h.remote = theirs, m.h.id = 400;
    t_printf("send to it: %lx\n", msg2(&m, SEND_MSG | MQ_CALL, sizeof m.h, 0, 0, 0));

    // A guard on a receive right is its context from then on (what
    // libxpc finds an async reply's handler by), until it is taken off.
    u32 g = allocate(RIGHT_RECEIVE);
    t_printf("guard: %ld\n", TRAP(TRAP_port_guard, task, g, 0x6e78, 0));
    t_printf("its context: %s\n", get_context(g) == 0x6e78 ? "the guard" : "something else");
    t_printf("unguard: %ld\n", TRAP(TRAP_port_unguard, task, g, 0x6e78));
    t_printf("its context now: %s\n", get_context(g) == 0 ? "none" : "something");
    return 0;
}
