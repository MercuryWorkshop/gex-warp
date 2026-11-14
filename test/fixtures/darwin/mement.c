// Named memory entries: mach_make_memory_entry_64 and mach_vm_map of the
// port it makes, as MIG messages to the task's port.
//
// With no argument, in one task: an entry of memory the task has and its
// mapping (the same pages: a store through either is seen through the
// other), the same mapped as a copy, an entry of memory made for it
// (MAP_MEM_NAMED_CREATE) mapped twice, a range of one (a parent entry),
// what is refused (more protection than the memory has or the entry
// allows, memory that is not there, a range past the entry's end), and
// mach_vm_read of the task's memory.
//
// With "fork", between two tasks: the child names a page of its own memory
// and memory made for it, and sends both entries to its parent (through
// the bootstrap port it was born with, as darwin/machipc does); the parent
// maps them, and each sees the other's stores while both run, and the
// parent's mappings keep the memory once the child is gone.
#include "t.h"

long t_trap8(long n, long a, long b, long c, long d, long e, long f, long g, long h);
#if defined(__arm64__)
__asm__(".text\n.globl _t_trap8\n.p2align 2\n_t_trap8:\n"
        "  neg x16, x0\n  mov x0, x1\n  mov x1, x2\n  mov x2, x3\n  mov x3, x4\n"
        "  mov x4, x5\n  mov x5, x6\n  mov x6, x7\n  ldr x7, [sp]\n  svc #0x80\n  ret\n");
#define PAGE 0x4000ul
#elif defined(__x86_64__)
__asm__(".text\n.globl _t_trap8\n_t_trap8:\n"
        "  movq %rdi, %rax\n  orq $0x1000000, %rax\n  movq %rsi, %rdi\n  movq %rdx, %rsi\n"
        "  movq %rcx, %rdx\n  movq %r8, %r10\n  movq %r9, %r8\n  movq 8(%rsp), %r9\n"
        "  movq 16(%rsp), %r11\n  movq %r11, 8(%rsp)\n  movq 24(%rsp), %r11\n  movq %r11, 16(%rsp)\n"
        "  syscall\n  ret\n");
#define PAGE 0x1000ul
#endif

enum { TRAP_port_allocate = 16, TRAP_port_mod_refs = 19, TRAP_mach_msg2 = 47 };
enum { RIGHT_RECEIVE = 1 };
enum { MOVE_SEND = 17, COPY_SEND = 19, MAKE_SEND = 20, MAKE_SEND_ONCE = 21 };
enum { SEND_MSG = 1, RCV_MSG = 2, RCV_TIMEOUT = 0x100 };
#define COMPLEX 0x80000000u
#define MQ_CALL 0x400000000ull
#define KOBJECT_CALL 0x200000000ull
#define RCV_INTERRUPTED 0x10004005
#define NAMED_CREATE 0x20000u
#define VM_COPY 0x200000u
#define R 1u
#define RW 3u

struct header {
    u32 bits, size, remote, local, voucher;
    i32 id;
};
struct port_desc {
    u32 name, pad1;
    u16 pad2;
    u8 disposition, type;
};
struct message {
    struct header h;
    u32 body[64];
};

static u32 task;
static const u32 ndr[2] = {0, 1};

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

static long rpc(struct message *m, u32 size) {
    u32 reply = (u32)TRAP(TRAP_mach_reply_port);
    m->h.bits = (m->h.bits & COMPLEX) | COPY_SEND | MAKE_SEND_ONCE << 8;
    m->h.remote = task, m->h.local = reply, m->h.voucher = 0;
    long r = msg2(m, SEND_MSG | RCV_MSG | KOBJECT_CALL, size, sizeof *m, reply, 0);
    TRAP(TRAP_port_mod_refs, task, reply, RIGHT_RECEIVE, -1);
    return r;
}

static void put64(u32 *at, u64 v) { t_memcpy(at, &v, 8); }
static u64 get64(const u32 *at) {
    u64 v;
    t_memcpy(&v, at, 8);
    return v;
}

// mach_make_memory_entry_64(task, &size, offset, permission, &entry, parent).
static long make_entry(u64 *size, u64 offset, u32 permission, u32 *entry, u32 parent) {
    static struct message m;
    t_memset(&m, 0, sizeof m);
    m.h.id = 4817, m.h.bits = COMPLEX;
    m.body[0] = 1;
    *(struct port_desc *)(m.body + 1) = (struct port_desc){parent, 0, 0, COPY_SEND, 0};
    m.body[4] = ndr[0], m.body[5] = ndr[1];
    put64(m.body + 6, *size);
    put64(m.body + 8, offset);
    m.body[10] = permission;
    long r = rpc(&m, sizeof m.h + 44);
    if (r) return r;
    if (!(m.h.bits & COMPLEX)) return (long)m.body[2];
    *entry = ((struct port_desc *)(m.body + 1))->name;
    *size = get64(m.body + 6);
    return 0;
}

// mach_vm_map(task, &address, size, 0, VM_FLAGS_ANYWHERE, entry, offset,
// copy, cur, cur, VM_INHERIT_DEFAULT).
static long map_entry(u64 *address, u64 size, u32 entry, u64 offset, u32 copy, u32 cur) {
    static struct message m;
    t_memset(&m, 0, sizeof m);
    m.h.id = 4811, m.h.bits = COMPLEX;
    m.body[0] = 1;
    *(struct port_desc *)(m.body + 1) = (struct port_desc){entry, 0, 0, COPY_SEND, 0};
    m.body[4] = ndr[0], m.body[5] = ndr[1];
    put64(m.body + 6, 0);
    put64(m.body + 8, size);
    put64(m.body + 10, 0);
    m.body[12] = 1;
    put64(m.body + 13, offset);
    m.body[15] = copy, m.body[16] = cur, m.body[17] = cur, m.body[18] = 1;
    long r = rpc(&m, sizeof m.h + 76);
    if (r) return r;
    if (m.body[2]) return (long)m.body[2];
    *address = get64(m.body + 3);
    return 0;
}

// mach_vm_read(task, address, size, &data, &count).
static long vm_read(u64 address, u64 size, u64 *data, u32 *count) {
    static struct message m;
    t_memset(&m, 0, sizeof m);
    m.h.id = 4804;
    m.body[0] = ndr[0], m.body[1] = ndr[1];
    put64(m.body + 2, address);
    put64(m.body + 4, size);
    long r = rpc(&m, sizeof m.h + 24);
    if (r) return r;
    if (!(m.h.bits & COMPLEX)) return (long)m.body[2];
    *data = get64(m.body + 1);
    *count = m.body[7];
    return 0;
}

static u8 *allocate_pages(u64 n) {
    u64 at = 0;
    return TRAP(TRAP_mach_vm_allocate, task, &at, n * PAGE, 1) ? 0 : (u8 *)at;
}

static const char *yes(int c) { return c ? "yes" : "no"; }

static void one_task(void) {
    // Memory the task has: three pages, the middle one named.
    u8 *base = allocate_pages(3);
    for (int i = 0; i < 3; i++) t_memset(base + i * PAGE, 'a' + i, PAGE);
    u64 size = PAGE;
    u32 e = 0;
    long r = make_entry(&size, (u64)base + PAGE, RW, &e, 0);
    t_printf("an entry of the task's memory: %ld, a page: %s\n", r, yes(size == PAGE));
    u64 at = 0;
    r = map_entry(&at, PAGE, e, 0, 0, RW);
    u8 *p = (u8 *)at;
    t_printf("mapped: %ld, holds the page: %s\n", r, yes(!r && p[0] == 'b' && p[PAGE - 1] == 'b'));
    if (!r) {
        p[10] = 'X';
        base[PAGE + 20] = 'Y';
        t_printf("a store through the mapping seen in the memory: %s; the other way: %s\n", yes(base[PAGE + 10] == 'X'), yes(p[20] == 'Y'));
    }
    u64 cp = 0;
    r = map_entry(&cp, PAGE, e, 0, 1, RW);
    if (!r) {
        u8 *c = (u8 *)cp;
        int had = c[10] == 'X' && c[20] == 'Y';
        c[30] = 'Z';
        t_printf("mapped as a copy: %ld, holds what was stored: %s, its own store kept to itself: %s\n", r, yes(had), yes(base[PAGE + 30] == 'b'));
    } else {
        t_printf("mapped as a copy: %ld\n", r);
    }
    t_printf("a range past the entry's end: %ld\n", map_entry(&cp, PAGE, e, PAGE, 0, RW));
    t_printf("more than the whole entry: %ld\n", map_entry(&cp, 2 * PAGE, e, 0, 0, RW));

    // Memory made for the entry, of a size that is not whole pages.
    u32 made = 0;
    size = 4 * PAGE - 100;
    r = make_entry(&size, 0, NAMED_CREATE | RW, &made, 0);
    t_printf("made: %ld, rounded to whole pages: %s\n", r, yes(size == 4 * PAGE));
    u64 a1 = 0, a2 = 0;
    long r1 = map_entry(&a1, 4 * PAGE, made, 0, 0, RW), r2 = map_entry(&a2, 4 * PAGE, made, 0, 0, RW);
    if (!r1 && !r2) {
        u8 *x = (u8 *)a1, *y = (u8 *)a2;
        int zero = 1;
        for (u64 i = 0; i < 4 * PAGE; i++) zero &= x[i] == 0;
        x[5] = 'm', y[3 * PAGE + 7] = 'n';
        t_printf("mapped twice: zeroes: %s, each sees the other's store: %s\n", yes(zero), yes(y[5] == 'm' && x[3 * PAGE + 7] == 'n'));
    } else {
        t_printf("mapped twice: %ld %ld\n", r1, r2);
    }
    // Its first page as an entry of its own. (Four pages made, since under
    // Rosetta a range of an entry is measured in the Mac's own 16 KiB
    // pages, which a smaller one is not.)
    u32 sub = 0;
    size = PAGE;
    r = make_entry(&size, 0, RW, &sub, made);
    u64 a3 = 0;
    long r3 = r ? -1 : map_entry(&a3, PAGE, sub, 0, 0, RW);
    t_printf("a range of it: %ld, mapped: %ld, its page: %s\n", r, r3, yes(!r3 && ((u8 *)a3)[5] == 'm'));
    size = 4 * PAGE;
    t_printf("a range past its end: %ld\n", make_entry(&size, PAGE, RW, &sub, made));

    // What is refused.
    u8 *ro = allocate_pages(1);
    ro[0] = 'r';
    TRAP(TRAP_mach_vm_protect, task, ro, PAGE, 0, R);
    u32 re = 0;
    size = PAGE;
    t_printf("to write memory that is read-only: %ld\n", make_entry(&size, (u64)ro, RW, &re, 0));
    size = PAGE;
    r = make_entry(&size, (u64)ro, R, &re, 0);
    t_printf("to read it: %ld\n", r);
    if (!r) {
        u64 ra = 0;
        t_printf("mapped to write: %ld\n", map_entry(&ra, PAGE, re, 0, 0, RW));
        r = map_entry(&ra, PAGE, re, 0, 0, R);
        t_printf("mapped to read: %ld, holds it: %s\n", r, yes(!r && ((u8 *)ra)[0] == 'r'));
    }
    u8 *gone = allocate_pages(1);
    TRAP(TRAP_mach_vm_deallocate, task, gone, PAGE);
    size = PAGE;
    t_printf("of memory that is not there: %ld\n", make_entry(&size, (u64)gone, RW, &re, 0));
    u64 data = 0;
    u32 count = 0;
    t_printf("read of memory that is not there: %ld\n", vm_read((u64)gone, PAGE, &data, &count));

    // mach_vm_read: a copy of the three pages.
    r = vm_read((u64)base, 3 * PAGE, &data, &count);
    t_printf("read: %ld, %s bytes, the same: %s\n", r, count == 3 * PAGE ? "all the" : "other", yes(!r && !t_memcmp((void *)data, base, 3 * PAGE)));
}

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

static u32 get_special(u32 which) {
    static struct message m;
    t_memset(&m, 0, sizeof m);
    m.h.id = 3409;
    m.body[0] = ndr[0], m.body[1] = ndr[1], m.body[2] = which;
    if (rpc(&m, sizeof m.h + 12) || !(m.h.bits & COMPLEX)) return 0;
    return ((struct port_desc *)(m.body + 1))->name;
}

// Wait up to five seconds for a byte to become `want`.
static int await(volatile u8 *at, u8 want) {
    u64 until = t_nanos() + 5000000000ull;
    while (*at != want)
        if (t_nanos() > until) return 0;
    return 1;
}

static int child(void) {
    task = (u32)TRAP(TRAP_task_self);
    u32 boot = get_special(4);
    if (!boot) return 1;
    int status = 0;
    u8 *mine = allocate_pages(3);
    t_memset(mine, 'c', 3 * PAGE);
    u64 size = PAGE;
    u32 e1 = 0, e2 = 0;
    if (make_entry(&size, (u64)mine + PAGE, RW, &e1, 0)) return 2;
    size = 2 * PAGE;
    if (make_entry(&size, 0, NAMED_CREATE | RW, &e2, 0)) return 4;
    u64 at = 0;
    if (map_entry(&at, 2 * PAGE, e2, 0, 0, RW)) return 8;
    volatile u8 *made = (u8 *)at;
    t_memcpy((void *)made, "hello", 5);
    static struct message m;
    t_memset(&m, 0, sizeof m);
    m.h.bits = COMPLEX | COPY_SEND, m.h.remote = boot, m.h.id = 100;
    m.body[0] = 2;
    *(struct port_desc *)(m.body + 1) = (struct port_desc){e1, 0, 0, MOVE_SEND, 0};
    *(struct port_desc *)(m.body + 4) = (struct port_desc){e2, 0, 0, MOVE_SEND, 0};
    if (msg2(&m, SEND_MSG | MQ_CALL, sizeof m.h + 28, 0, 0, 0)) return 16;
    // The parent's stores, seen here; then ours, for it to see.
    volatile u8 *page = mine + PAGE;
    if (!await(page + 100, 'P')) status |= 32;
    if (!await(made + PAGE + 200, 'Q')) status |= 64;
    page[101] = 'A';
    made[PAGE + 201] = 'B';
    // Until the parent has seen them.
    if (!await(page + 102, 'D')) status |= 128;
    return status;
}

static void two_tasks(void) {
    u32 boot = 0;
    TRAP(TRAP_port_allocate, task, RIGHT_RECEIVE, &boot);
    t_printf("set the bootstrap port: %ld\n", set_special(4, boot));
    long pid = 0, child_flag = -1;
    pid = t_syscall2(&child_flag, SYS_fork, 0, 0, 0);
    if (pid < 0) {
        t_printf("fork failed: %ld\n", pid);
        return;
    }
    if (child_flag) t_exit(child());
    static struct message m;
    t_memset(&m, 0, sizeof m);
    long r = msg2(&m, RCV_MSG | RCV_TIMEOUT, 0, sizeof m, boot, 10000);
    t_printf("received: %ld, %u entries\n", r, (m.h.bits & COMPLEX) ? m.body[0] : 0);
    u32 e1 = ((struct port_desc *)(m.body + 1))->name, e2 = ((struct port_desc *)(m.body + 4))->name;
    u64 a1 = 0, a2 = 0;
    long r1 = map_entry(&a1, PAGE, e1, 0, 0, RW), r2 = map_entry(&a2, 2 * PAGE, e2, 0, 0, RW);
    volatile u8 *page = (u8 *)a1, *made = (u8 *)a2;
    t_printf("its page mapped: %ld, holds it: %s\n", r1, yes(!r1 && page[0] == 'c' && page[PAGE - 1] == 'c'));
    t_printf("its made memory mapped: %ld, holds what it wrote: %s\n", r2, yes(!r2 && !t_memcmp((void *)made, "hello", 5)));
    if (r1 || r2) return;
    page[100] = 'P';
    made[PAGE + 200] = 'Q';
    int a = await(page + 101, 'A'), b = await(made + PAGE + 201, 'B');
    t_printf("its stores seen while it runs: %s, %s\n", yes(a), yes(b));
    page[102] = 'D';
    int status = -1;
    long got = SYS(SYS_wait4, pid, &status, 0, 0);
    t_printf("the child: %s, status %x\n", got == pid ? "waited for" : "not waited for", status);
    t_printf("the memory, the child gone: %s\n", yes(page[101] == 'A' && made[PAGE + 201] == 'B' && made[0] == 'h'));
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)envp, (void)apple;
    task = (u32)TRAP(TRAP_task_self);
    if (argc > 1 && !t_strcmp(argv[1], "fork"))
        two_tasks();
    else
        one_task();
    return 0;
}
