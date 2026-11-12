/* An executable bigger than the space gex's fixed layout has for one below
 * the interpreter (0x0100_0000 to 0x04a0_0000): BIG_MIB MiB of .bss, every
 * page of it written and read back, its last byte too; the break grown by
 * 1 MiB and written; and malloc's memory, a large block and a small one.
 * Then one line of what it saw, and where its image and its interpreter
 * are: in gex's fixed low layout, or in the arena above it (0x0700_0000).
 * With the argument `exec` it then execs itself, by argv[0], to do it all
 * again in the exec'd image. */
#define _DEFAULT_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/auxv.h>
#include <sys/syscall.h>

#ifndef BIG_MIB
#define BIG_MIB 96
#endif
#define ARENA 0x07000000UL

static char big[(size_t)BIG_MIB << 20];

static const char *where(unsigned long a)
{
	return a == 0 ? "none" : a >= ARENA ? "arena" : "fixed";
}

int main(int argc, char **argv)
{
	memset(big, 1, sizeof big);
	big[sizeof big - 1] = 7;
	long pages = 0;
	for (size_t i = 0; i < sizeof big; i += 4096)
		pages += big[i];

	/* (musl's sbrk takes no increment: the break by its system call) */
	char *b0 = (char *)syscall(SYS_brk, 0);
	char *b1 = (char *)syscall(SYS_brk, b0 + (1 << 20));
	int brk_ok = b1 == b0 + (1 << 20) && b0 >= big + sizeof big;
	if (brk_ok) {
		memset(b0, 3, 1 << 20);
		brk_ok = b0[0] == 3 && b0[(1 << 20) - 1] == 3;
	}

	char *m = malloc(4 << 20);
	char *s = malloc(100);
	int malloc_ok = m && s;
	if (malloc_ok) {
		memset(m, 5, 4 << 20);
		strcpy(s, "small");
		malloc_ok = m[(4 << 20) - 1] == 5 && !strcmp(s, "small");
	}

	printf("pages %ld, last %d, brk %s, malloc %s, image %s, interpreter %s\n", pages, big[sizeof big - 1],
	       brk_ok ? "ok" : "FAIL", malloc_ok ? "ok" : "FAIL", where((unsigned long)big),
	       where(getauxval(AT_BASE)));
	fprintf(stderr, "big %p, break %p, malloc %p %p, main %p\n", (void *)big, (void *)b0, (void *)m, (void *)s,
		(void *)main);
	if (argc > 1 && !strcmp(argv[1], "exec")) {
		fflush(stdout);
		execl(argv[0], argv[0], (char *)0);
		perror("execl");
		return 127;
	}
	return 0;
}
