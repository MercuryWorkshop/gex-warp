// See t.h.
#include "t.h"

#if defined(__arm64__)
// The call's number in x16, `svc #0x80`; carry set means x0 is an error.
__asm__(
    ".text\n.globl _t_syscall\n.p2align 2\n_t_syscall:\n"
    "  mov x16, x0\n  mov x0, x1\n  mov x1, x2\n  mov x2, x3\n  mov x3, x4\n"
    "  mov x4, x5\n  mov x5, x6\n  mov x6, x7\n  ldr x7, [sp]\n"
    "  svc #0x80\n  b.cc 1f\n  neg x0, x0\n1: ret\n"
    ".globl _t_syscall2\n.p2align 2\n_t_syscall2:\n"
    "  mov x9, x0\n  mov x16, x1\n  mov x0, x2\n  mov x1, x3\n  mov x2, x4\n"
    "  svc #0x80\n  str x1, [x9]\n  b.cc 1f\n  neg x0, x0\n1: ret\n"
    ".globl _t_trap\n.p2align 2\n_t_trap:\n"
    "  neg x16, x0\n  mov x0, x1\n  mov x1, x2\n  mov x2, x3\n  mov x3, x4\n"
    "  mov x4, x5\n  mov x5, x6\n  svc #0x80\n  ret\n");
#elif defined(__x86_64__)
// The class in the top byte of rax (2: BSD, 1: Mach), `syscall`; carry set
// means rax is an error.
__asm__(
    ".text\n.globl _t_syscall\n_t_syscall:\n"
    "  movq %rdi, %rax\n  orq $0x2000000, %rax\n  movq %rsi, %rdi\n  movq %rdx, %rsi\n"
    "  movq %rcx, %rdx\n  movq %r8, %r10\n  movq %r9, %r8\n  movq 8(%rsp), %r9\n"
    // The seventh and eighth are read from the stack, past the return
    // address: where this was passed its seventh and eighth, one further
    // up. (Moved down in place, so that the call is made as libsyscall's
    // stubs make it, the return address on top.)
    "  movq 16(%rsp), %r11\n  movq %r11, 8(%rsp)\n  movq 24(%rsp), %r11\n  movq %r11, 16(%rsp)\n"
    "  syscall\n  jnc 1f\n  negq %rax\n1: ret\n"
    ".globl _t_syscall2\n_t_syscall2:\n"
    "  movq %rdi, %r11\n  pushq %r11\n  movq %rsi, %rax\n  orq $0x2000000, %rax\n  movq %rdx, %rdi\n"
    "  movq %rcx, %rsi\n  movq %r8, %rdx\n  syscall\n  popq %r11\n  movq %rdx, (%r11)\n"
    "  jnc 1f\n  negq %rax\n1: ret\n"
    ".globl _t_trap\n_t_trap:\n"
    "  movq %rdi, %rax\n  orq $0x1000000, %rax\n  movq %rsi, %rdi\n  movq %rdx, %rsi\n"
    "  movq %rcx, %rdx\n  movq %r8, %r10\n  movq %r9, %r8\n  movq 8(%rsp), %r9\n"
    "  syscall\n  ret\n");
#endif

// The trampoline: (handler, style, signal, siginfo, context, token). The
// handler is called as handler(signal, siginfo, context), then
// sigreturn(context, UC_FLAVOR, token) puts the interrupted thread back.
#if defined(__arm64__)
__asm__(
    ".text\n.p2align 2\n_t_sigtramp:\n"
    "  mov x19, x4\n  mov x20, x5\n  mov x8, x0\n"
    "  mov x0, x2\n  mov x1, x3\n  mov x2, x4\n  blr x8\n"
    "  mov x0, x19\n  mov x1, #30\n  mov x2, x20\n  mov x16, #184\n  svc #0x80\n  brk #1\n");
#elif defined(__x86_64__)
__asm__(
    ".text\n_t_sigtramp:\n"
    "  movq %r8, %r12\n  movq %r9, %r13\n  movq %rdi, %rax\n"
    "  movq %rdx, %rdi\n  movq %rcx, %rsi\n  movq %r8, %rdx\n  callq *%rax\n"
    "  movq %r12, %rdi\n  movl $30, %esi\n  movq %r13, %rdx\n  movl $0x20000b8, %eax\n  syscall\n  ud2\n");
#endif
extern void t_sigtramp(void) __asm__("_t_sigtramp");

long t_sigaction(int sig, void *handler, int flags, u32 mask, struct t_sigaction *old) {
    // struct __sigaction: the handler, the trampoline, the mask, the flags.
    struct {
        void *handler;
        void (*tramp)(void);
        u32 mask;
        int flags;
    } act = {handler, t_sigtramp, mask, flags};
    return SYS(SYS_sigaction, sig, &act, old);
}

void t_lock(u32 *word) {
    // 0 free, 1 held, 2 held and waited for.
    u32 c = 0;
    if (__atomic_compare_exchange_n(word, &c, 1, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) return;
    if (c != 2) c = __atomic_exchange_n(word, 2, __ATOMIC_ACQUIRE);
    while (c != 0) {
        SYS(SYS_ulock_wait, UL_COMPARE_AND_WAIT, word, 2, 0);
        c = __atomic_exchange_n(word, 2, __ATOMIC_ACQUIRE);
    }
}

void t_unlock(u32 *word) {
    if (__atomic_exchange_n(word, 0, __ATOMIC_RELEASE) == 2) SYS(SYS_ulock_wake, UL_COMPARE_AND_WAIT, word, 0);
}

// Mach's own clock; but for x86-64, whose reference is Rosetta: there the
// trap answers in the host's ticks and the timebase says one to one, so the
// wall clock measures instead.
u64 t_nanos(void) {
#if defined(__x86_64__)
    struct t_timeval tv = {0, 0};
    SYS(SYS_gettimeofday, &tv, 0, 0);
    return (u64)tv.tv_sec * 1000000000ull + (u64)tv.tv_usec * 1000;
#else
    struct {
        u32 numer, denom;
    } tb = {1, 1};
    TRAP(TRAP_mach_timebase_info, &tb);
    u64 t = (u64)TRAP(TRAP_mach_absolute_time);
    return t / tb.denom * tb.numer + t % tb.denom * tb.numer / tb.denom;
#endif
}

#define T_THREADS 16
#ifdef T_STATIC
// What stands for a pthread_t: the kernel is told where in it the thread's
// own data is (`tsd`), and points the thread pointer there.
struct t_pthread {
    void *(*fn)(void *);
    void *arg;
    u32 done, port;
    u64 pad;
    u64 tsd[8];
};
static struct t_pthread *threads[T_THREADS];
static int nthreads;

void t_thread_main(struct t_pthread *self, u32 port) {
    u64 tp = 0;
#if defined(__arm64__)
    __asm__("mrs %0, tpidrro_el0" : "=r"(tp));
#else
    // The base of %gs is not read directly: its first word is, where the
    // thread's maker put the address, as libpthread does.
    __asm__("movq %%gs:0, %0" : "=r"(tp));
#endif
    if (tp != (u64)self->tsd) t_printf("FAIL the new thread's thread pointer\n");
    if (port != (u32)TRAP(TRAP_thread_self)) t_printf("FAIL the new thread's port\n");
    self->port = port;
    self->fn(self->arg);
    __atomic_store_n(&self->done, 1, __ATOMIC_RELEASE);
    SYS(SYS_ulock_wake, UL_COMPARE_AND_WAIT | ULF_WAKE_ALL, &self->done, 0);
    // (no stack to free: the joiner still reads this thread's record)
    for (;;) SYS(SYS_bsdthread_terminate, 0, 0, 0, 0);
}

#if defined(__arm64__)
// (pthread, port, fn, arg, stack, flags), on the new stack.
__asm__(".text\n.p2align 2\n_t_thread_entry:\n  bl _t_thread_main\n  brk #1\n");
#elif defined(__x86_64__)
__asm__(".text\n_t_thread_entry:\n  andq $-16, %rsp\n  callq _t_thread_main\n  ud2\n");
#endif
extern void t_thread_entry(void) __asm__("_t_thread_entry");

int t_thread_start(void *(*fn)(void *), void *arg) {
    static int registered;
    if (!registered) {
        // struct _pthread_registration_data, packed.
        struct __attribute__((packed)) {
            u64 version, dispatch_queue_offset, main_qos;
            u32 tsd_offset, return_to_kernel_offset, mach_thread_self_offset;
            u64 stack_addr_hint;
            u32 mutex_default_policy, joinable_offset_bits, wq_quantum_expiry_offset;
        } data;
        t_memset(&data, 0, sizeof data);
        data.version = sizeof data;
        data.tsd_offset = __builtin_offsetof(struct t_pthread, tsd);
        long r = SYS(SYS_bsdthread_register, t_thread_entry, 0, sizeof(struct t_pthread), &data, sizeof data, 0);
        if (r < 0) return (int)r;
        registered = 1;
    }
    if (nthreads == T_THREADS) return -1;
    // The stack, with the thread's record at its top.
    long size = 256 * 1024;
    long base = SYS(SYS_mmap, 0, size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    if (base < 0) return (int)base;
    struct t_pthread *t = (struct t_pthread *)(base + size - 4096);
    t->fn = fn, t->arg = arg;
    t->tsd[0] = (u64)t->tsd;
    threads[nthreads] = t;
    // PTHREAD_START_CUSTOM: the stack and the record are the caller's.
    long r = SYS(SYS_bsdthread_create, fn, arg, t, t, 0x01000000);
    if (r < 0) return (int)r;
    return nthreads++;
}

void t_thread_join(int handle) {
    struct t_pthread *t = threads[handle];
    while (!__atomic_load_n(&t->done, __ATOMIC_ACQUIRE)) SYS(SYS_ulock_wait, UL_COMPARE_AND_WAIT, &t->done, 0, 0);
}

u32 t_thread_port(int handle) {
    // Set by the thread itself as it starts.
    while (!__atomic_load_n(&threads[handle]->port, __ATOMIC_ACQUIRE)) TRAP(TRAP_swtch_pri, 0);
    return threads[handle]->port;
}
#else
int pthread_create(void **thread, const void *attr, void *(*fn)(void *), void *arg);
int pthread_join(void *thread, void **value);
u32 pthread_mach_thread_np(void *thread);
static void *threads[T_THREADS];
static int nthreads;

int t_thread_start(void *(*fn)(void *), void *arg) {
    if (nthreads == T_THREADS) return -1;
    int r = pthread_create(&threads[nthreads], 0, fn, arg);
    return r ? -r : nthreads++;
}

void t_thread_join(int handle) {
    pthread_join(threads[handle], 0);
}

u32 t_thread_port(int handle) {
    return pthread_mach_thread_np(threads[handle]);
}
#endif

size_t t_strlen(const char *s) {
    size_t n = 0;
    while (s[n]) n++;
    return n;
}

int t_strcmp(const char *a, const char *b) {
    while (*a && *a == *b) a++, b++;
    return (u8)*a - (u8)*b;
}

int t_strncmp(const char *a, const char *b, size_t n) {
    for (; n; n--, a++, b++) {
        if (*a != *b) return (u8)*a - (u8)*b;
        if (!*a) break;
    }
    return 0;
}

void *t_memset(void *p, int c, size_t n) {
    volatile u8 *d = p;
    while (n--) *d++ = (u8)c;
    return p;
}

void *t_memcpy(void *d, const void *s, size_t n) {
    volatile u8 *dd = d;
    const u8 *ss = s;
    while (n--) *dd++ = *ss++;
    return d;
}

int t_memcmp(const void *a, const void *b, size_t n) {
    const u8 *x = a, *y = b;
    for (; n; n--, x++, y++)
        if (*x != *y) return *x - *y;
    return 0;
}

const char *t_getenv(char **vec, const char *name) {
    size_t n = t_strlen(name);
    for (; *vec; vec++)
        if (!t_strncmp(*vec, name, n) && (*vec)[n] == '=') return *vec + n + 1;
    return 0;
}

struct out {
    int fd;
    size_t n;
    char buf[512];
};

static void flush(struct out *o) {
    size_t at = 0;
    while (at < o->n) {
        long w = SYS(SYS_write, o->fd, o->buf + at, o->n - at);
        if (w <= 0) break;
        at += (size_t)w;
    }
    o->n = 0;
}

static void put(struct out *o, char c) {
    if (o->n == sizeof o->buf) flush(o);
    o->buf[o->n++] = c;
}

static void number(struct out *o, u64 v, unsigned base, int neg, int width, char pad) {
    char tmp[32];
    int n = 0;
    do {
        unsigned d = (unsigned)(v % base);
        tmp[n++] = (char)(d < 10 ? '0' + d : 'a' + d - 10);
        v /= base;
    } while (v);
    if (neg && pad == '0') put(o, '-');
    for (int i = n + neg; i < width; i++) put(o, pad);
    if (neg && pad != '0') put(o, '-');
    while (n) put(o, tmp[--n]);
}

static void format(struct out *o, const char *fmt, __builtin_va_list ap) {
    for (; *fmt; fmt++) {
        if (*fmt != '%') {
            put(o, *fmt);
            continue;
        }
        fmt++;
        char pad = ' ';
        int width = 0, lng = 0;
        if (*fmt == '0') pad = '0', fmt++;
        while (*fmt >= '0' && *fmt <= '9') width = width * 10 + (*fmt++ - '0');
        while (*fmt == 'l' || *fmt == 'z') lng++, fmt++;
        switch (*fmt) {
        case 'd':
        case 'i': {
            i64 v = lng ? __builtin_va_arg(ap, long) : __builtin_va_arg(ap, int);
            number(o, v < 0 ? 0 - (u64)v : (u64)v, 10, v < 0, width, pad);
            break;
        }
        case 'u':
            number(o, lng ? __builtin_va_arg(ap, unsigned long) : __builtin_va_arg(ap, unsigned), 10, 0, width, pad);
            break;
        case 'x':
            number(o, lng ? __builtin_va_arg(ap, unsigned long) : __builtin_va_arg(ap, unsigned), 16, 0, width, pad);
            break;
        case 'o':
            number(o, lng ? __builtin_va_arg(ap, unsigned long) : __builtin_va_arg(ap, unsigned), 8, 0, width, pad);
            break;
        case 'p':
            put(o, '0'), put(o, 'x');
            number(o, (u64)__builtin_va_arg(ap, void *), 16, 0, 0, ' ');
            break;
        case 'c':
            put(o, (char)__builtin_va_arg(ap, int));
            break;
        case 's': {
            const char *s = __builtin_va_arg(ap, const char *);
            if (!s) s = "(null)";
            for (int i = (int)t_strlen(s); i < width; i++) put(o, ' ');
            while (*s) put(o, *s++);
            break;
        }
        case '%':
            put(o, '%');
            break;
        default:
            put(o, '%');
            if (*fmt) put(o, *fmt);
            else fmt--;
        }
    }
    flush(o);
}

void t_printf(const char *fmt, ...) {
    struct out o;
    o.fd = 1, o.n = 0;
    __builtin_va_list ap;
    __builtin_va_start(ap, fmt);
    format(&o, fmt, ap);
    __builtin_va_end(ap);
}

void t_dprintf(int fd, const char *fmt, ...) {
    struct out o;
    o.fd = fd, o.n = 0;
    __builtin_va_list ap;
    __builtin_va_start(ap, fmt);
    format(&o, fmt, ap);
    __builtin_va_end(ap);
}

void t_show(const char *name, long r) {
    t_printf("%s = %ld\n", name, r);
}

void t_check(const char *name, int cond) {
    t_printf("%s %s\n", cond ? "ok" : "FAIL", name);
}

void t_exit(int code) {
    for (;;) SYS(SYS_exit, code);
}

#ifdef T_STATIC
// The compiler may call these by name for a struct copy or a cleared array.
void *memset(void *p, int c, size_t n) {
    return t_memset(p, c, n);
}
void *memcpy(void *d, const void *s, size_t n) {
    return t_memcpy(d, s, n);
}
void bzero(void *p, size_t n) {
    t_memset(p, 0, n);
}

// The stack XNU leaves a static program: argc, argv, 0, envp, 0, apple, 0.
void t_start(long *sp) {
    int argc = (int)sp[0];
    char **argv = (char **)(sp + 1);
    char **envp = argv + argc + 1;
    char **apple = envp;
    while (*apple) apple++;
    apple++;
    t_exit(main(argc, argv, envp, apple));
}

#if defined(__arm64__)
__asm__(".text\n.globl start\n.p2align 2\nstart:\n  mov x0, sp\n  and sp, x0, #~15\n  mov x29, #0\n  mov x30, #0\n  bl _t_start\n  brk #1\n");
#elif defined(__x86_64__)
__asm__(".text\n.globl start\nstart:\n  movq %rsp, %rdi\n  andq $-16, %rsp\n  xorl %ebp, %ebp\n  callq _t_start\n  ud2\n");
#endif
#endif
