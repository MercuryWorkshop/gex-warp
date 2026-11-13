// IOKit's registry through its MIG interface, as IOKit.framework speaks it
// (the `iokit` subsystem, routines 2800 on): the master port from the host
// (host_get_io_master); the kernel's version, which says a matching
// dictionary may be sent serialized in the binary form; the services a
// dictionary matches (IOResources, and a class there is none of), each a
// port, and what the registry says of the one found: its class, what it
// conforms to, its superclass, its name, its path, its id, a property;
// the entry by path, and nothing for a path to nothing; opening a service
// that has no user client; a routine there is none of.
//
// Only what any Mac's registry has is asked: IOResources is at the root of
// the service plane everywhere, and says IOKit = IOService.
#include "t.h"

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

enum { TRAP_port_mod_refs = 19, TRAP_mach_msg = 31, TRAP_mach_msg2 = 47 };
enum { RIGHT_RECEIVE = 1 };
enum { COPY_SEND = 19, MAKE_SEND_ONCE = 21 };
enum { SEND_MSG = 1, RCV_MSG = 2 };
#define COMPLEX 0x80000000u
#define KOBJECT_CALL 0x200000000ull
#define RCV_INTERRUPTED 0x10004005

struct header {
    u32 bits, size, remote, local, voucher;
    i32 id;
};
struct message {
    struct header h;
    u32 body[1100];
};

static u32 task;

__attribute__((unused)) static long msg2(struct message *m, u64 option, u32 send_size, u32 rcv_size, u32 rcv_name) {
    u32 descs = (option & SEND_MSG) && (m->h.bits & COMPLEX) ? m->body[0] : 0;
    long r;
    do {
        r = t_trap8(TRAP_mach_msg2, (long)m, (long)option, (long)(m->h.bits | (u64)send_size << 32),
                    (long)(m->h.remote | (u64)m->h.local << 32), (long)(m->h.voucher | (u64)(u32)m->h.id << 32),
                    (long)(descs | (u64)rcv_name << 32), (long)rcv_size, 0);
        option &= ~(u64)SEND_MSG;
    } while (r == RCV_INTERRUPTED);
    return r;
}

// A MIG request to `to`, a port of the kernel's, and its reply in place:
// with mach_msg_trap under gex (as 10.14's libSystem sends), mach_msg2 on
// the Mac (which ends a program that makes the older trap).
static struct message m;
static long rpc(u32 to, i32 id, u32 size) {
    u32 reply = (u32)TRAP(TRAP_mach_reply_port);
    m.h.bits = (m.h.bits & COMPLEX) | COPY_SEND | MAKE_SEND_ONCE << 8;
    m.h.remote = to, m.h.local = reply, m.h.voucher = 0, m.h.id = id;
    long r;
#ifdef T_STATIC
    do {
        r = TRAP(TRAP_mach_msg, &m, SEND_MSG | RCV_MSG, size, sizeof m, reply, 0);
    } while (r == RCV_INTERRUPTED);
#else
    r = msg2(&m, SEND_MSG | RCV_MSG | KOBJECT_CALL, size, sizeof m, reply);
#endif
    TRAP(TRAP_port_mod_refs, task, reply, RIGHT_RECEIVE, -1);
    return r;
}

// A request's arguments, packed as MIG packs them, after the NDR record.
static u32 *args;
static u32 *at;
static void begin(void) {
    t_memset(&m, 0, sizeof m);
    m.body[0] = 0, m.body[1] = 1;  // NDR
    at = args = m.body + 2;
}
static u32 size_now(void) {
    return (u32)((u8 *)at - (u8 *)&m);
}
static void put_bytes(const void *p, u32 n) {
    *at++ = n;
    t_memcpy(at, p, n);
    at += (n + 3) / 4;
}
// A C string: an offset nothing uses, then the bytes with the terminator.
static void put_cstr(const char *s) {
    *at++ = 0;
    put_bytes(s, (u32)t_strlen(s) + 1);
}

// The right a reply carries in its one port descriptor (0 for none), or
// the error a simple reply says.
static u32 port_of(long r, long *code) {
    *code = r;
    if (r) return 0;
    if (!(m.h.bits & COMPLEX)) {
        *code = (i32)m.body[2];
        return 0;
    }
    return m.body[1];
}

// A simple reply's code, and where what follows it begins.
static i32 code_of(long r) {
    return r ? (i32)r : (i32)m.body[2];
}
static const char *cstr_of(void) {
    return (const char *)(m.body + 5);
}

static u32 matching(u32 master, const char *class) {
    // { IOProviderClass: class } in the binary form: the signature, a
    // dictionary of one, its key a symbol, its value the string ending it.
    static u32 b[64];
    u32 *w = b;
    *w++ = 0xd3;
    *w++ = 0x81000001;
    *w++ = 0x08000010;
    t_memcpy(w, "IOProviderClass", 16);
    w += 4;
    u32 n = (u32)t_strlen(class);
    *w++ = 0x89000000 | n;
    t_memset(w, 0, (n + 3) & ~3u);
    t_memcpy(w, class, n);
    w += (n + 3) / 4;
    begin();
    put_bytes(b, (u32)((u8 *)w - (u8 *)b));
    long code;
    u32 it = port_of(rpc(master, 2881, size_now()), &code);
    if (code) t_printf("matching %s = %x\n", class, (u32)code);
    return it;
}

static u32 next(u32 it, long *code) {
    begin();
    return port_of(rpc(it, 2802, sizeof m.h), code);
}

static u64 entry_id(u32 entry) {
    begin();
    long r = rpc(entry, 2871, sizeof m.h);
    return code_of(r) ? 0 : *(u64 *)(m.body + 3);
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    task = (u32)TRAP(TRAP_task_self);
    u32 host = (u32)TRAP(TRAP_host_self);
    begin();
    long code;
    u32 master = port_of(rpc(host, 205, sizeof m.h), &code);
    t_printf("host_get_io_master = %d, a port: %s\n", (int)code, master ? "yes" : "no");
    if (!master) return 1;

    begin();
    long r = rpc(master, 2877, sizeof m.h);
    t_printf("io_server_version = %d, binary matching: %s\n", code_of(r), *(u64 *)(m.body + 3) >= 20150715 ? "yes" : "no");

    u32 it = matching(master, "IOResources");
    int count = 0;
    u32 resources = 0, e;
    while ((e = next(it, &code))) {
        if (!resources) resources = e;
        count++;
    }
    t_printf("IOResources: %d found, then %x\n", count, (u32)code);
    it = matching(master, "GexNoSuchClass");
    next(it, &code);
    t_printf("GexNoSuchClass: none found: %x\n", (u32)code);
    if (!resources) return 1;

    begin();
    r = rpc(resources, 2800, sizeof m.h);
    t_printf("class = %d %s\n", code_of(r), cstr_of());
    static const char *classes[] = {"IOService", "IORegistryEntry", "IOHIDSystem"};
    for (int i = 0; i < 3; i++) {
        begin();
        put_cstr(classes[i]);
        r = rpc(resources, 2801, size_now());
        t_printf("conforms to %s = %d %d\n", classes[i], code_of(r), (int)m.body[3]);
    }
    begin();
    put_cstr("IOResources");
    r = rpc(master, 2860, size_now());
    t_printf("superclass of IOResources = %d %s\n", code_of(r), cstr_of());

    begin();
    r = rpc(resources, 2810, sizeof m.h);
    t_printf("name = %d %s\n", code_of(r), cstr_of());
    begin();
    put_cstr("IOService");
    r = rpc(resources, 2826, size_now());
    t_printf("path = %d %s\n", code_of(r), cstr_of());
    u64 id = entry_id(resources);
    t_printf("entry id: %s\n", id ? "yes" : "none");

    begin();
    put_cstr("IOService:/IOResources");
    u32 again = port_of(rpc(master, 2809, size_now()), &code);
    t_printf("by path = %d, the same entry: %s\n", (int)code, again && entry_id(again) == id ? "yes" : "no");
    begin();
    put_cstr("IOService:/GexNoSuchEntry");
    u32 none = port_of(rpc(master, 2809, size_now()), &code);
    t_printf("a path to nothing = %d, a port: %s\n", (int)code, none ? "yes" : "no");

    // io_registry_entry_get_property_bin(entry, plane, name, options): the
    // value, serialized, out of line.
    begin();
    put_cstr("IOService");
    put_cstr("IOKit");
    *at++ = 0;
    r = rpc(resources, 2879, size_now());
    if (r || !(m.h.bits & COMPLEX)) {
        t_printf("IOKit = %x\n", r ? (u32)r : m.body[2]);
    } else {
        const u32 *v = *(const u32 **)(m.body + 1);
        u32 key = v[1], n = key & 0xffffff, kind = key & 0x7f000000;
        // A string, or a symbol (whose length counts its terminator).
        if (kind == 0x08000000 && n) n--;
        char s[64];
        t_memset(s, 0, sizeof s);
        t_memcpy(s, v + 2, n < 63 ? n : 63);
        t_printf("IOKit = %s (signature %x)\n", s, v[0]);
    }

    // io_service_open_extended(service, owning task, type, NDR, properties):
    // IOResources makes no user client.
    begin();
    m.h.bits = COMPLEX;
    m.body[0] = 2;
    m.body[1] = task, m.body[2] = 0, m.body[3] = COPY_SEND << 16;
    m.body[4] = 0, m.body[5] = 0, m.body[6] = 0x01000000, m.body[7] = 0;
    m.body[8] = 0, m.body[9] = 1;
    m.body[10] = 0;
    m.body[11] = 0, m.body[12] = 1;
    m.body[13] = 0;
    r = rpc(resources, 2862, sizeof m.h + 14 * 4);
    u32 connect = port_of(r, &code);
    t_printf("open IOResources = %d, result %x, a connection: %s\n", (int)code, code ? 0 : m.body[6], connect ? "yes" : "no");

    begin();
    r = rpc(master, 2899, sizeof m.h);
    t_printf("routine 2899 = %d\n", code_of(r));
    return 0;
}
