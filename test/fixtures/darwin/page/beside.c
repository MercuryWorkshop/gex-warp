// A Linux program beside a Mac one, in one system: this one (Linux's) runs
// a Mac's shell from the Mac root at /opt/mac as its child, and the two
// talk through what the one kernel under them has -- FIFOs in the /tmp they
// share, POSIX shared memory by name (which the Mac program reads as the
// file of /dev/shm it is), a wait status, and the signals that stop a job
// and set it going.
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <unistd.h>

static const char script[] =
    "read line < /tmp/to-mac;"
    "{ echo \"a $(uname -s) program heard: $line\";"
    "  echo \"it reads the shared memory: $(head -c 16 /dev/shm/beside)\"; } > /tmp/to-linux;"
    "sleep 1;"
    "exit 7";

int main(void) {
    setvbuf(stdout, 0, _IONBF, 0);
    struct utsname u;
    uname(&u);
    printf("beside: this is a %s program\n", u.sysname);
    mkfifo("/tmp/to-mac", 0600);
    mkfifo("/tmp/to-linux", 0600);
    int shm = shm_open("/beside", O_CREAT | O_RDWR, 0600);
    ftruncate(shm, 16384);
    char *mem = mmap(0, 16384, PROT_READ | PROT_WRITE, MAP_SHARED, shm, 0);
    if (mem == MAP_FAILED) {
        perror("beside: mmap");
        return 1;
    }
    strcpy(mem, "written by Linux");
    pid_t pid = fork();
    if (pid == 0) {
        execl("/opt/mac/bin/bash", "bash", "-c", script, (char *)0);
        perror("beside: exec");
        _exit(127);
    }
    int to = open("/tmp/to-mac", O_WRONLY);
    write(to, "hello from Linux\n", 17);
    close(to);
    FILE *from = fopen("/tmp/to-linux", "r");
    char line[256];
    while (from && fgets(line, sizeof line, from)) printf("beside: %s", line);
    if (from) fclose(from);
    // The Mac program is this one's job: stopped, seen to be, set going.
    int st = 0;
    kill(pid, SIGSTOP);
    pid_t r = waitpid(pid, &st, WUNTRACED);
    printf("beside: the Mac program %s\n", r == pid && WIFSTOPPED(st) ? "stopped" : "did not stop");
    kill(pid, SIGCONT);
    r = waitpid(pid, &st, 0);
    printf("beside: the Mac program ended, status %d\n", r == pid && WIFEXITED(st) ? WEXITSTATUS(st) : -1);
    unlink("/tmp/to-mac"), unlink("/tmp/to-linux"), shm_unlink("/beside");
    return 3;
}
