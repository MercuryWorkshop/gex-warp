/* The Linux personality's share of setuid.c: a program with the set-user-ID
 * or set-group-ID bit run by an ordinary user (gexc only), what the ids are
 * and AT_SECURE says, and giving up root as su does.
 *
 *   setuid-linux exec PATH [ARGS...]   execve PATH with ARGS, or "show"
 *   setuid-linux show LABEL
 *   setuid-linux drop LABEL SECRET PLAIN
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <stdio.h>
#include <string.h>
#include <sys/auxv.h>
#include <sys/stat.h>
#include <unistd.h>

static void show(const char *name, long r) {
	if (r < 0) printf("%s = -%d\n", name, errno);
	else printf("%s = %ld\n", name, r);
}

static void ids(const char *label, const char *when) {
	uid_t r, e, s;
	gid_t rg, eg, sg;
	getresuid(&r, &e, &s);
	getresgid(&rg, &eg, &sg);
	printf("%s %s: ruid %d euid %d suid %d rgid %d egid %d sgid %d AT_SECURE %lu\n", label, when, r, e, s, rg, eg, sg, getauxval(AT_SECURE));
	gid_t list[32];
	int n = getgroups(32, list);
	printf("%s %s: getgroups = %d:", label, when, n);
	for (int i = 0; i < n; i++) printf(" %u", list[i]);
	printf("\n");
}

int main(int argc, char **argv) {
	setvbuf(stdout, 0, _IONBF, 0);
	if (argc < 3) return 2;
	const char *how = argv[1], *path = argv[2];
	if (!strcmp(how, "exec")) {
		char *args[16] = {(char *)path, "show", "exec", 0};
		if (argc > 3) {
			int n = 0;
			for (int i = 2; i < argc && n < 15; i++) args[n++] = argv[i];
			args[n] = 0;
		}
		execv(path, args);
		show("execve", -1);
		return 1;
	}
	if (!strcmp(how, "show") || !strcmp(how, "drop")) {
		const char *label = path;
		ids(label, "at the start");
		struct stat st;
		int r = stat(argv[0], &st);
		printf("%s: its file is owned by %u:%u, mode %o (%d)\n", label, st.st_uid, st.st_gid, st.st_mode & 07777, r);
		if (!strcmp(how, "show")) return 0;
		const char *secret = argv[3], *plain = argv[4];
		show("open the secret", open(secret, O_RDONLY));
		gid_t groups[2] = {20, 80};
		show("setgroups(2)", setgroups(2, groups));
		ids(label, "after setgroups");
		show("setgid(20)", setgid(20));
		show("setgroups(1)", setgroups(1, groups));
		show("setuid(502)", setuid(502));
		ids(label, "after giving up root");
		show("setuid(0)", setuid(0));
		show("setgid(0)", setgid(0));
		show("setgroups(1)", setgroups(1, groups));
		show("open the secret", open(secret, O_RDONLY));
		char *args[] = {(char *)plain, "show", "after the drop", 0};
		execv(plain, args);
		show("execve", -1);
		return 1;
	}
	return 2;
}
