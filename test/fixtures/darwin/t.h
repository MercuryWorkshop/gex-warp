// A freestanding C library for gex's Darwin tests: raw XNU system calls and
// just enough formatting to say what they answered. The same source is
// built twice. With -DT_STATIC it is a static Mach-O executable (its own
// entry point, no dyld, no libSystem), which is what gex runs before any
// Darwin userland exists in a guest. Without it, it is an ordinary dynamic
// executable whose main dyld calls, and it still asks nothing of libSystem:
// that one runs on the Mac the tests are built on, and what it prints there
// is what gex must print.
//
// A call answers its value, or the negative of Darwin's error number: the
// kernel reports failure with the carry flag set and the error number in
// the result register, which the stubs fold into one value.
#ifndef T_H
#define T_H

typedef unsigned long size_t;
typedef long ssize_t;
typedef unsigned char u8;
typedef unsigned short u16;
typedef unsigned int u32;
typedef unsigned long long u64;
typedef int i32;
typedef long long i64;

// A BSD system call with up to eight arguments.
long t_syscall(long n, long a, long b, long c, long d, long e, long f, long g, long h);
// The same, also storing the second result register (pipe's second
// descriptor, fork's child flag) at `second`.
long t_syscall2(long *second, long n, long a, long b, long c);
// A Mach trap (its positive number) or machine-dependent call: no carry flag.
long t_trap(long n, long a, long b, long c, long d, long e, long f);

#define SYS(n, ...) T_SYS_(n, ##__VA_ARGS__, 0, 0, 0, 0, 0, 0, 0, 0)
#define T_SYS_(n, a, b, c, d, e, f, g, h, ...) \
    t_syscall(n, (long)(a), (long)(b), (long)(c), (long)(d), (long)(e), (long)(f), (long)(g), (long)(h))
#define TRAP(n, ...) T_TRAP_(n, ##__VA_ARGS__, 0, 0, 0, 0, 0, 0)
#define T_TRAP_(n, a, b, c, d, e, f, ...) t_trap(n, (long)(a), (long)(b), (long)(c), (long)(d), (long)(e), (long)(f))

// printf's %d %i %u %x %o %c %s %p with l, ll, z and a zero-padded width, to
// a descriptor, unbuffered: what is printed is in order with the calls
// around it.
void t_printf(const char *fmt, ...);
void t_dprintf(int fd, const char *fmt, ...);
// "name = value" for a call's answer, an error by its number: `open = -2`.
void t_show(const char *name, long r);
// "ok name" or "FAIL name".
void t_check(const char *name, int cond);
void t_exit(int code) __attribute__((noreturn));

// sigaction as the libc makes the call: with a trampoline the kernel enters
// instead of the handler, which calls it and then sigreturn. `handler` is
// 0 (SIG_DFL), 1 (SIG_IGN) or a function; `mask` and the answer's are
// Darwin's 32-bit sets. The old action's handler and flags go to `old`.
struct t_sigaction {
    void *handler;
    u32 mask;
    int flags;
};
long t_sigaction(int sig, void *handler, int flags, u32 mask, struct t_sigaction *old);
// What a handler installed with SA_SIGINFO is called with.
struct t_siginfo {
    int si_signo, si_errno, si_code, si_pid;
    u32 si_uid;
    int si_status;
    void *si_addr;
    long si_value, si_band;
    u64 pad[7];
};
struct t_ucontext {
    int uc_onstack;
    u32 uc_sigmask;
    void *ss_sp;
    u64 ss_size;
    int ss_flags;
    void *uc_link;
    u64 uc_mcsize;
    struct t_mcontext *uc_mcontext;
};
#if defined(__arm64__)
struct t_mcontext {
    u64 far;
    u32 esr, exception;
    u64 x[29], fp, lr, sp, pc;
    u32 cpsr, flags;
    __uint128_t v[32];
    u32 fpsr, fpcr;
};
#endif
enum { SA_ONSTACK = 1, SA_RESTART = 2, SA_RESETHAND = 4, SA_NODEFER = 0x10, SA_SIGINFO = 0x40 };
enum { SIG_BLOCK = 1, SIG_UNBLOCK = 2, SIG_SETMASK = 3 };
#define T_SIGBIT(sig) (1u << ((sig) - 1))

// A thread running fn(arg) on a stack of its own: its handle, or a negative
// error. The static build asks the kernel directly (bsdthread_register once,
// then bsdthread_create, as libpthread does); the Mac's build goes through
// the libpthread that is already registered there. t_thread_join waits for
// its end, t_thread_port is its Mach port's name (for __pthread_kill).
int t_thread_start(void *(*fn)(void *), void *arg);
void t_thread_join(int handle);
u32 t_thread_port(int handle);
// A lock on a word of memory, waited for with ulock_wait: what
// os_unfair_lock is, less the owner's name.
void t_lock(u32 *word);
void t_unlock(u32 *word);
enum { UL_COMPARE_AND_WAIT = 1, UL_UNFAIR_LOCK = 2, ULF_WAKE_ALL = 0x100, ULF_NO_ERRNO = 0x1000000 };

size_t t_strlen(const char *s);
int t_strcmp(const char *a, const char *b);
int t_strncmp(const char *a, const char *b, size_t n);
void *t_memset(void *p, int c, size_t n);
void *t_memcpy(void *d, const void *s, size_t n);
int t_memcmp(const void *a, const void *b, size_t n);
// The value of NAME in a vector of NAME=value strings (envp, apple), or 0.
const char *t_getenv(char **vec, const char *name);

// XNU's system call numbers (bsd/kern/syscalls.master).
enum {
    SYS_exit = 1, SYS_fork = 2, SYS_read = 3, SYS_write = 4, SYS_open = 5, SYS_close = 6, SYS_wait4 = 7,
    SYS_link = 9, SYS_unlink = 10, SYS_chdir = 12, SYS_fchdir = 13, SYS_chmod = 15, SYS_getpid = 20,
    SYS_getuid = 24, SYS_geteuid = 25, SYS_access = 33, SYS_kill = 37, SYS_getppid = 39, SYS_dup = 41,
    SYS_pipe = 42, SYS_getegid = 43, SYS_sigaction = 46, SYS_getgid = 47, SYS_sigprocmask = 48,
    SYS_sigpending = 52, SYS_sigaltstack = 53, SYS_ioctl = 54, SYS_symlink = 57, SYS_readlink = 58,
    SYS_execve = 59, SYS_umask = 60, SYS_munmap = 73, SYS_mprotect = 74, SYS_madvise = 75,
    SYS_getpgrp = 81, SYS_setpgid = 82, SYS_setitimer = 83, SYS_getitimer = 86, SYS_getdtablesize = 89,
    SYS_dup2 = 90, SYS_fcntl = 92, SYS_select = 93, SYS_fsync = 95, SYS_gettimeofday = 116,
    SYS_getrusage = 117, SYS_readv = 120, SYS_writev = 121, SYS_fchmod = 124, SYS_rename = 128,
    SYS_flock = 131, SYS_mkdir = 136, SYS_rmdir = 137, SYS_utimes = 138, SYS_setsid = 147,
    SYS_sigsuspend = 111, SYS_getpgid = 151, SYS_pread = 153, SYS_pwrite = 154, SYS_sigreturn = 184, SYS_getrlimit = 194,
    SYS_setrlimit = 195, SYS_mmap = 197, SYS_lseek = 199, SYS_truncate = 200, SYS_ftruncate = 201,
    SYS_sysctl = 202, SYS_poll = 230, SYS_sysctlbyname = 274, SYS_issetugid = 327, SYS___pthread_kill = 328,
    SYS___semwait_signal = 334, SYS_stat64 = 338, SYS_fstat64 = 339, SYS_lstat64 = 340,
    SYS_getdirentries64 = 344, SYS_bsdthread_create = 360, SYS_bsdthread_terminate = 361,
    SYS_bsdthread_register = 366, SYS_thread_selfid = 372, SYS_openat = 463, SYS_renameat = 465,
    SYS_faccessat = 466, SYS_fstatat64 = 470, SYS_unlinkat = 472, SYS_readlinkat = 473, SYS_mkdirat = 475,
    SYS_getentropy = 500, SYS_ulock_wait = 515, SYS_ulock_wake = 516,
};

// Mach traps (osfmk/kern/syscall_sw.c), by their positive numbers.
enum {
    TRAP_mach_absolute_time = 3, TRAP_mach_vm_allocate = 10, TRAP_mach_vm_deallocate = 12,
    TRAP_mach_vm_protect = 14, TRAP_mach_vm_map = 15, TRAP_mach_reply_port = 26, TRAP_thread_self = 27,
    TRAP_task_self = 28, TRAP_host_self = 29, TRAP_swtch_pri = 59, TRAP_mach_timebase_info = 89,
};

// Darwin's own spellings of what the tests pass.
enum {
    O_RDONLY = 0, O_WRONLY = 1, O_RDWR = 2, O_NONBLOCK = 4, O_APPEND = 8, O_NOFOLLOW = 0x100,
    O_CREAT = 0x200, O_TRUNC = 0x400, O_EXCL = 0x800, O_DIRECTORY = 0x100000, O_CLOEXEC = 0x1000000,
};
enum { AT_FDCWD = -2, AT_SYMLINK_NOFOLLOW = 0x20, AT_REMOVEDIR = 0x80 };
enum { PROT_NONE = 0, PROT_READ = 1, PROT_WRITE = 2, PROT_EXEC = 4 };
enum { MAP_SHARED = 1, MAP_PRIVATE = 2, MAP_FIXED = 0x10, MAP_ANON = 0x1000 };
enum { F_DUPFD = 0, F_GETFD = 1, F_SETFD = 2, F_GETFL = 3, F_SETFL = 4, F_GETPATH = 50, F_DUPFD_CLOEXEC = 67 };
enum {
    SIGHUP = 1, SIGINT = 2, SIGQUIT = 3, SIGILL = 4, SIGTRAP = 5, SIGABRT = 6, SIGFPE = 8, SIGKILL = 9,
    SIGBUS = 10, SIGSEGV = 11, SIGSYS = 12, SIGPIPE = 13, SIGALRM = 14, SIGTERM = 15, SIGURG = 16,
    SIGSTOP = 17, SIGTSTP = 18, SIGCONT = 19, SIGCHLD = 20, SIGWINCH = 28, SIGUSR1 = 30, SIGUSR2 = 31,
};

// `struct stat` (64-bit inodes: the only one arm64 has).
struct t_timespec {
    long tv_sec;
    long tv_nsec;
};
struct t_stat {
    i32 st_dev;
    u16 st_mode;
    u16 st_nlink;
    u64 st_ino;
    u32 st_uid;
    u32 st_gid;
    i32 st_rdev;
    struct t_timespec st_atim, st_mtim, st_ctim, st_birthtim;
    i64 st_size;
    i64 st_blocks;
    i32 st_blksize;
    u32 st_flags;
    u32 st_gen;
    i32 st_lspare;
    i64 st_qspare[2];
};
struct t_timeval {
    long tv_sec;
    int tv_usec;
};

int main(int argc, char **argv, char **envp, char **apple);

#endif
