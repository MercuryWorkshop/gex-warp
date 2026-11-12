/* big.c as a static position-independent executable, with no libc: the
 * musl cross toolchains' libc.a is not position-independent, so a
 * -static-pie against it keeps relocations in its read-only text, and its
 * start-up code faults writing them (on Linux too). Nothing here needs a
 * relocation: the array and the strings are reached PC-relative, and the
 * system calls are made directly. 96 MiB of .bss, every page written and
 * read back, its last byte too; the break grown by 1 MiB and written; an
 * anonymous mapping (what malloc would take for a large block) written;
 * then one line of what it saw. */

#if defined(__aarch64__)
enum { SYS_write = 64, SYS_brk = 214, SYS_mmap = 222, SYS_exit_group = 94 };
#elif defined(__x86_64__)
enum { SYS_write = 1, SYS_brk = 12, SYS_mmap = 9, SYS_exit_group = 231 };
#endif

#define BIG (96UL << 20)
#define ARENA 0x07000000UL

static char big[BIG];

static long sys6(long n, long a, long b, long c, long d, long e, long f)
{
#if defined(__aarch64__)
	register long x8 __asm__("x8") = n;
	register long x0 __asm__("x0") = a;
	register long x1 __asm__("x1") = b;
	register long x2 __asm__("x2") = c;
	register long x3 __asm__("x3") = d;
	register long x4 __asm__("x4") = e;
	register long x5 __asm__("x5") = f;
	__asm__ volatile("svc 0" : "+r"(x0) : "r"(x8), "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x5) : "memory");
	return x0;
#elif defined(__x86_64__)
	long r;
	register long r10 __asm__("r10") = d;
	register long r8 __asm__("r8") = e;
	register long r9 __asm__("r9") = f;
	__asm__ volatile("syscall" : "=a"(r) : "a"(n), "D"(a), "S"(b), "d"(c), "r"(r10), "r"(r8), "r"(r9) : "rcx", "r11", "memory");
	return r;
#endif
}

static unsigned long len(const char *s)
{
	unsigned long n = 0;
	while (s[n])
		n++;
	return n;
}

static void put(const char *s)
{
	sys6(SYS_write, 1, (long)s, (long)len(s), 0, 0, 0);
}

static void num(unsigned long v)
{
	char b[24];
	int n = sizeof b - 1;
	b[n] = 0;
	do {
		b[--n] = '0' + v % 10;
		v /= 10;
	} while (v);
	put(b + n);
}

static void fill(volatile char *p, unsigned long n, char c)
{
	for (unsigned long i = 0; i < n; i++)
		p[i] = c;
}

void cstart(void)
{
	volatile char *v = big;
	fill(v, BIG, 1);
	v[BIG - 1] = 7;
	unsigned long pages = 0;
	for (unsigned long i = 0; i < BIG; i += 4096)
		pages += v[i];

	char *b0 = (char *)sys6(SYS_brk, 0, 0, 0, 0, 0, 0);
	char *b1 = (char *)sys6(SYS_brk, (long)(b0 + (1 << 20)), 0, 0, 0, 0, 0);
	int brk_ok = b1 == b0 + (1 << 20) && b0 >= big + BIG;
	if (brk_ok) {
		fill(b0, 1 << 20, 3);
		brk_ok = ((volatile char *)b0)[0] == 3 && ((volatile char *)b0)[(1 << 20) - 1] == 3;
	}

	/* PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS */
	long m = sys6(SYS_mmap, 0, 4 << 20, 3, 0x22, -1, 0);
	int map_ok = (unsigned long)m < -4095UL;
	if (map_ok) {
		fill((char *)m, 4 << 20, 5);
		map_ok = ((volatile char *)m)[(4 << 20) - 1] == 5;
	}

	put("pages ");
	num(pages);
	put(", last ");
	num(v[BIG - 1]);
	put(brk_ok ? ", brk ok" : ", brk FAIL");
	put(map_ok ? ", mmap ok" : ", mmap FAIL");
	put((unsigned long)big >= ARENA ? ", image arena\n" : ", image fixed\n");
	sys6(SYS_exit_group, 0, 0, 0, 0, 0, 0);
	for (;;)
		;
}

#if defined(__aarch64__)
__asm__(".text\n.global _start\n_start:\n\tmov x29, #0\n\tmov x30, #0\n\tbl cstart\n");
#elif defined(__x86_64__)
__asm__(".text\n.global _start\n_start:\n\txor %ebp, %ebp\n\tand $-16, %rsp\n\tcall cstart\n\thlt\n");
#endif
