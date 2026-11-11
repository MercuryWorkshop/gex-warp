// The size stat gives a directory in /tmp as entries come and go: on a
// Mac's APFS 64, and 32 more for each entry whatever its name's length
// (gex's tmpfs said 4096 throughout; pyrus's gex-gaps/access_x_ok.c).
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static char dir[] = "/tmp/gexdirsize.XXXXXX";

static long long size(const char *p) {
    struct stat st;
    return stat(p, &st) ? -1 : (long long)st.st_size;
}

static void make(int i) {
    char p[96];
    snprintf(p, sizeof p, "%s/f%d", dir, i);
    close(open(p, O_CREAT | O_WRONLY, 0644));
}

int main(void) {
    if (!mkdtemp(dir)) return perror("mkdtemp"), 1;
    printf("empty: %lld\n", size(dir));
    int at[] = {1, 2, 10, 100};
    for (int k = 0, i = 0; k < 4; k++) {
        while (i < at[k]) make(i++);
        printf("%d entries: %lld\n", at[k], size(dir));
    }
    char p[160];
    for (int i = 0; i < 50; i++) snprintf(p, sizeof p, "%s/f%d", dir, i), unlink(p);
    printf("50 taken away: %lld\n", size(dir));
    snprintf(p, sizeof p, "%s/sub", dir);
    mkdir(p, 0755);
    printf("and a directory: %lld, which is %lld\n", size(dir), size(p));
    char q[200];
    snprintf(q, sizeof q, "%s/a_name_of_some_length_to_see_whether_the_length_counts_at_all", dir);
    close(open(q, O_CREAT | O_WRONLY, 0644));
    printf("and a long name: %lld\n", size(dir));
    unlink(q), rmdir(p);
    for (int i = 50; i < 100; i++) snprintf(p, sizeof p, "%s/f%d", dir, i), unlink(p);
    printf("emptied: %lld\n", size(dir));
    rmdir(dir);
    return 0;
}
