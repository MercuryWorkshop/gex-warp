// A child watched by a dispatch process source (EVFILT_PROC, NOTE_EXIT)
// that is sent SIGSTOP, SIGSTOP, SIGCONT and SIGTERM one after another:
// its end is told every time, and it ended by the SIGTERM. Under gex it
// was missed about one spawn in three (pyrus-gui's gex-gaps/
// proc_source_stop.c): the child of a posix_spawn from the main thread had
// the signal mask of one of dispatch's workers, which blocks SIGTERM (the
// fork took whichever thread's state was loaded in the process's kernel),
// and a stop taken by the child just before the SIGCONT stopped it after
// the SIGCONT, for good.
#include <dispatch/dispatch.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>
extern char **environ;

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    int runs = argc > 1 ? atoi(argv[1]) : 40, missed = 0, odd = 0;
    for (int i = 0; i < runs; i++) {
        char *av[] = {"/bin/sleep", "3", NULL};
        pid_t pid;
        if (posix_spawn(&pid, "/bin/sleep", NULL, NULL, av, environ)) return printf("posix_spawn failed\n"), 1;
        dispatch_semaphore_t s = dispatch_semaphore_create(0);
        __block int status = -1;
        dispatch_source_t src = dispatch_source_create(DISPATCH_SOURCE_TYPE_PROC, pid, DISPATCH_PROC_EXIT, dispatch_get_global_queue(0, 0));
        dispatch_source_set_event_handler(src, ^{
            int st;
            waitpid(pid, &st, 0);
            status = st;
            dispatch_source_cancel(src);
            dispatch_semaphore_signal(s);
        });
        dispatch_resume(src);
        usleep(i % 2 ? 200000 : 50000);
        kill(pid, SIGSTOP), kill(pid, SIGSTOP), kill(pid, SIGCONT), kill(pid, SIGTERM);
        if (dispatch_semaphore_wait(s, dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC))) {
            missed++;
            kill(pid, SIGKILL), kill(pid, SIGCONT);
            dispatch_semaphore_wait(s, dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC));
        } else if (!WIFSIGNALED(status) || WTERMSIG(status) != SIGTERM) {
            odd++;
        }
    }
    printf("of %d: the end not told %d, not by SIGTERM %d\n", runs, missed, odd);
    return 0;
}
