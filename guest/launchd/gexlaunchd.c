// gexlaunchd: a bootstrap server for a Mac system under gex, in place of
// launchd.
//
// macOS's launchd is the system's first process (it refuses to be anything
// else), and much more than a bootstrap server: coalitions, audit sessions,
// sandbox and code-signing policy, the job manager behind launchctl. What a
// program asks of it to get going is narrower: its bootstrap port, through
// which it looks up the Mach services it talks to (bootstrap_look_up) and a
// daemon checks in for the receive rights of the services it provides
// (bootstrap_check_in, and an XPC listener's check-in). This is that part.
//
// It reads the job descriptions (LaunchDaemons and LaunchAgents plists:
// Label, Program or ProgramArguments, MachServices), makes a port for each
// Mach service, and serves requests on the bootstrap port it hands to every
// process it starts. A job is started when one of its services is first
// looked up; it checks in and is given the receive right; when it ends, its
// services get new ports and it can be started again.
//
// The requests arrive as libxpc sends them, an XPC pipe routine
// (subsystem 5) of a dictionary, and are read and answered with the same
// library (`xpc_pipe_receive`, `xpc_pipe_routine_reply`): this program runs
// against the system's own libxpc, whichever release that is. A reply says
// whom it answers (`req_pid`, `rec_execcnt`, from the request's audit
// token), as launchd's do, and comes from a process running as root, which
// libxpc insists on of a bootstrap server that is not process 1.
//
//   gexlaunchd [-v] [-u UID[:GID]] [-j DIR]... [-x LABEL]... [--] PROGRAM [ARG...]
//
// starts PROGRAM with this server as its bootstrap port and exits when it
// does, with its status, ending the jobs it started. -j adds a directory of
// job plists (default: /System/Library/LaunchDaemons and
// /System/Library/LaunchAgents); -x leaves a job out; -v says what is asked
// and answered; -u is the user PROGRAM and the agents run as.
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <mach/mach.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#include <xpc/xpc.h>

// libxpc's own interface, which launchd uses (not in the SDK's headers).
int xpc_pipe_receive(mach_port_t port, xpc_object_t *message);
int xpc_pipe_routine_reply(xpc_object_t reply);
xpc_object_t xpc_create_from_plist(const void *data, size_t length);
void xpc_dictionary_set_mach_send(xpc_object_t d, const char *key, mach_port_t port);
void xpc_dictionary_set_mach_recv(xpc_object_t d, const char *key, mach_port_t port);
void xpc_dictionary_get_audit_token(xpc_object_t d, audit_token_t *token);
char *xpc_copy_description(xpc_object_t o);

// The requests this answers: subsystem 5 (the bootstrap's) routines, and
// the one of subsystem 7 (launch_msg, liblaunch's older interface) that a
// daemon checks in with.
enum {
    SUBSYSTEM_BOOTSTRAP = 5,
    SUBSYSTEM_LAUNCH = 7,
    // An XPC connection's lookup of the service it is made for.
    SUBSYSTEM_XPC = 3,
    ROUTINE_ENDPOINT_LOOKUP = 804,
    // vproc_swap_integer and vproc_swap_complex: a job's property.
    SUBSYSTEM_VPROC = 6,
    ROUTINE_VPROC_SWAP = 301,
    ROUTINE_LOOKUP = 207,
    ROUTINE_CHECKIN = 805,
    ROUTINE_LAUNCH_MSG = 100,
};

struct service {
    char *name;
    struct job *job;
    // The receive right while the job does not have it, and a send right
    // to give whoever looks it up.
    mach_port_t recv, send;
};

struct job {
    char *label;
    char **argv;
    pid_t pid;
    int started;
    // Asked to be started, by a lookup ([`start_wanted`]).
    int wanted;
    // An agent, the user's, run as the user.
    int agent;
};

static struct service *services;
static int nservices;
static struct job *jobs;
static int njobs;
static mach_port_t bootstrap;
static int verbose;
// Jobs not offered (-x), and by default the log daemon: a process that
// finds logd hands it a buffer of its memory to share (the firehose,
// made a memory entry with mach_make_memory_entry_64), which gex cannot
// yet share after the fact, and libdispatch stops the program when that
// fails. Without logd it logs nowhere, as it did without a bootstrap port.
// And the directory service, which ends at its start here (EX_OSERR):
// without it libinfo reads /etc's files.
static const char *skipped[64] = {"com.apple.logd", "com.apple.opendirectoryd"};
static int nskipped = 2;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;

static void say(const char *fmt, ...) {
    if (!verbose) return;
    va_list ap;
    va_start(ap, fmt);
    fprintf(stderr, "gexlaunchd: ");
    vfprintf(stderr, fmt, ap);
    fputc('\n', stderr);
    va_end(ap);
}

static char *dup_string(xpc_object_t s) {
    return s && xpc_get_type(s) == XPC_TYPE_STRING ? strdup(xpc_string_get_string_ptr(s)) : NULL;
}

// One job description: its program and its Mach services.
static void add_job(const char *path) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) return;
    struct stat st;
    if (fstat(fd, &st) || st.st_size <= 0 || st.st_size > (1 << 22)) {
        close(fd);
        return;
    }
    char *data = malloc(st.st_size);
    ssize_t got = read(fd, data, st.st_size);
    close(fd);
    xpc_object_t plist = got == st.st_size ? xpc_create_from_plist(data, st.st_size) : NULL;
    free(data);
    if (!plist || xpc_get_type(plist) != XPC_TYPE_DICTIONARY) return;
    xpc_object_t mach = xpc_dictionary_get_value(plist, "MachServices");
    char *label = dup_string(xpc_dictionary_get_value(plist, "Label"));
    if (!label || !mach || xpc_get_type(mach) != XPC_TYPE_DICTIONARY) return;
    for (int i = 0; i < nskipped; i++)
        if (!strcmp(skipped[i], label)) return;
    // Its arguments: ProgramArguments, with Program for the file to run if
    // it says one.
    xpc_object_t args = xpc_dictionary_get_value(plist, "ProgramArguments");
    char *program = dup_string(xpc_dictionary_get_value(plist, "Program"));
    size_t n = args && xpc_get_type(args) == XPC_TYPE_ARRAY ? xpc_array_get_count(args) : 0;
    // (Room for a Program alone as the arguments, the null that ends
    // them, and the Program kept past it.)
    char **argv = calloc(n + 3, sizeof *argv);
    for (size_t i = 0; i < n; i++) argv[i] = dup_string(xpc_array_get_value(args, i));
    if (!n && program) argv[0] = strdup(program), n = 1;
    if (!n) return;
    if (program) argv[n + 1] = program;  // (the file, kept past the arguments)
    jobs = realloc(jobs, (njobs + 1) * sizeof *jobs);
    jobs[njobs] = (struct job){label, argv, 0, 0, 0, strstr(path, "/LaunchAgents/") != NULL};
    // (Each job has its own place: services point to it by index below.)
    xpc_dictionary_apply(mach, ^bool(const char *name, xpc_object_t v) {
        (void)v;
        for (int i = 0; i < nservices; i++)
            if (!strcmp(services[i].name, name)) return true;
        services = realloc(services, (nservices + 1) * sizeof *services);
        services[nservices++] = (struct service){strdup(name), (struct job *)(intptr_t)njobs, MACH_PORT_NULL, MACH_PORT_NULL};
        return true;
    });
    njobs++;
}

static void add_dir(const char *dir) {
    DIR *d = opendir(dir);
    if (!d) return;
    struct dirent *e;
    while ((e = readdir(d))) {
        size_t len = strlen(e->d_name);
        if (len < 7 || strcmp(e->d_name + len - 6, ".plist")) continue;
        char path[1024];
        snprintf(path, sizeof path, "%s/%s", dir, e->d_name);
        add_job(path);
    }
    closedir(d);
}

static struct service *find(const char *name) {
    for (int i = 0; i < nservices; i++)
        if (!strcmp(services[i].name, name)) return &services[i];
    return NULL;
}

// The service's port, made if it has none: the receive right kept until
// its job checks in, and a send right for lookups.
static void make_port(struct service *s) {
    if (s->send != MACH_PORT_NULL) return;
    mach_port_allocate(mach_task_self(), MACH_PORT_RIGHT_RECEIVE, &s->recv);
    mach_port_insert_right(mach_task_self(), s->recv, s->recv, MACH_MSG_TYPE_MAKE_SEND);
    s->send = s->recv;
    // (A name for the send right of its own, so that the receive right can
    // go and the send right stay.)
    mach_port_mod_refs(mach_task_self(), s->send, MACH_PORT_RIGHT_SEND, 1);
}

// Start a process with this server as its bootstrap port.
// The user the first program and the agents (LaunchAgents' jobs) run as
// (-u); daemons run as root, as this does.
static uid_t user;
static gid_t group;

static pid_t start(char **argv, const char *file, int as_user) {
    pid_t pid = fork();
    if (pid == 0) {
        if (as_user && user) {
            if (setgid(group) || setuid(user)) _exit(126);
        }
        execv(file ? file : argv[0], argv);
        fprintf(stderr, "gexlaunchd: %s: %s\n", file ? file : argv[0], strerror(errno));
        _exit(127);
    }
    return pid;
}

// The main thread is told to start a job by a byte on this pipe: a process
// forks only from the thread it began on, which is the one a fork's child
// is sure to be able to go on from.
static int wake[2];

static void want_job(struct job *j) {
    if (j->pid > 0 || j->wanted) return;
    j->wanted = 1;
    char c = 1;
    (void)!write(wake[1], &c, 1);
}

// Called by the main thread, with the lock held.
static void start_job(struct job *j) {
    j->wanted = 0;
    if (j->pid > 0) return;
    size_t n = 0;
    while (j->argv[n]) n++;
    // (A Program kept past the arguments, after the null that ends them.)
    const char *file = j->argv[n + 1];
    j->pid = start(j->argv, file, j->agent);
    j->started++;
    say("started %s, pid %d", j->label, j->pid);
}

static struct job *job_of(struct service *s) {
    return &jobs[(intptr_t)s->job];
}

static void reply_error(xpc_object_t reply, int64_t error) {
    xpc_dictionary_set_int64(reply, "error", error);
}

// The job a process is, by its pid.
static struct job *job_by_pid(pid_t pid) {
    for (int i = 0; i < njobs; i++)
        if (jobs[i].pid == pid) return &jobs[i];
    return NULL;
}

// Whether a process is in the user's session rather than the system's.
static int aqua(struct job *j, audit_token_t token) {
    return j ? j->agent : token.val[1] != 0;
}

// launch_msg("CheckIn"): the job's description as launchd hands it back,
// with the receive right of each of its Mach services.
static void launch_checkin(xpc_object_t reply, pid_t from) {
    struct job *j = job_by_pid(from);
    if (!j) {
        reply_error(reply, ESRCH);
        return;
    }
    xpc_object_t response = xpc_dictionary_create(NULL, NULL, 0);
    xpc_object_t mach = xpc_dictionary_create(NULL, NULL, 0);
    xpc_dictionary_set_string(response, "Label", j->label);
    for (int k = 0; k < nservices; k++) {
        struct service *s = &services[k];
        if (job_of(s) != j) continue;
        make_port(s);
        if (s->recv == MACH_PORT_NULL) continue;
        xpc_dictionary_set_mach_recv(mach, s->name, s->recv);
        s->recv = MACH_PORT_NULL;
        say("check-in %s by %d (launch_msg)", s->name, from);
    }
    xpc_dictionary_set_value(response, "MachServices", mach);
    xpc_dictionary_set_value(reply, "response", response);
    xpc_release(mach);
    xpc_release(response);
}

static void handle(xpc_object_t request) {
    audit_token_t token;
    xpc_dictionary_get_audit_token(request, &token);
    pid_t from = (pid_t)token.val[5];
    uint64_t routine = xpc_dictionary_get_uint64(request, "routine");
    const char *name = xpc_dictionary_get_string(request, "name");
    if (verbose) {
        char *desc = xpc_copy_description(request);
        say("from %d: %s", from, desc);
        free(desc);
    }
    xpc_object_t reply = xpc_dictionary_create_reply(request);
    if (!reply) return;
    // Whom it answers, as libxpc checks of a server that is not process 1.
    xpc_dictionary_set_uint64(reply, "req_pid", token.val[5]);
    xpc_dictionary_set_uint64(reply, "rec_execcnt", token.val[7]);
    pthread_mutex_lock(&lock);
    struct service *s = name ? find(name) : NULL;
    uint64_t subsystem = xpc_dictionary_get_uint64(request, "subsystem");
    if (subsystem == SUBSYSTEM_XPC && routine == ROUTINE_ENDPOINT_LOOKUP) routine = ROUTINE_LOOKUP;
    if (subsystem == SUBSYSTEM_VPROC && routine == ROUTINE_VPROC_SWAP) {
        // What a job is told of itself (vproc_priv.h's keys): the manager
        // by name, its environment, and numbers: none, and managed.
        if (xpc_dictionary_get_bool(request, "get")) {
            struct job *j = job_by_pid(from);
            switch (xpc_dictionary_get_uint64(request, "outgsk")) {
            // The manager's name, and (3) the user its domain is for:
            // the user's session ("Aqua") for an agent and for a process
            // that is no daemon of this one's and is not root, the
            // system's otherwise.
            case 6:
                xpc_dictionary_set_string(reply, "out", aqua(j, token) ? "Aqua" : "System");
                break;
            case 3:
                xpc_dictionary_set_int64(reply, "out", aqua(j, token) ? token.val[1] : 0);
                break;
            case 16: {
                xpc_object_t env = xpc_dictionary_create(NULL, NULL, 0);
                xpc_dictionary_set_value(reply, "out", env);
                xpc_release(env);
                break;
            }
            case 5:
                xpc_dictionary_set_int64(reply, "out", j != NULL);
                break;
            default:
                xpc_dictionary_set_int64(reply, "out", 0);
                break;
            }
        }
        routine = 0;
    }
    if (subsystem == SUBSYSTEM_LAUNCH) {
        const char *what = xpc_dictionary_get_string(request, "request");
        if (routine == ROUTINE_LAUNCH_MSG && what && !strcmp(what, "CheckIn")) launch_checkin(reply, from);
        else reply_error(reply, ENOTSUP);
        routine = 0;
    }
    switch (routine) {
    case 0:
        break;
    case ROUTINE_LOOKUP:
        if (!s) {
            reply_error(reply, ESRCH);
            break;
        }
        make_port(s);
        if (s->recv != MACH_PORT_NULL) want_job(job_of(s));
        xpc_dictionary_set_mach_send(reply, "port", s->send);
        say("lookup %s: found", name);
        break;
    case ROUTINE_CHECKIN:
        if (!s) {
            reply_error(reply, ESRCH);
            break;
        }
        make_port(s);
        if (s->recv == MACH_PORT_NULL) {
            reply_error(reply, EEXIST);
            break;
        }
        // The receive right goes with the reply.
        xpc_dictionary_set_mach_recv(reply, "port", s->recv);
        s->recv = MACH_PORT_NULL;
        if (job_of(s)->pid <= 0) job_of(s)->pid = from;
        say("check-in %s by %d", name, from);
        break;
    default:
        say("routine %llu: not answered", routine);
        reply_error(reply, ENOTSUP);
        break;
    }
    pthread_mutex_unlock(&lock);
    xpc_pipe_routine_reply(reply);
    xpc_release(reply);
}

static void *serve(void *arg) {
    (void)arg;
    for (;;) {
        xpc_object_t request = NULL;
        int r = xpc_pipe_receive(bootstrap, &request);
        if (r || !request) {
            say("receive: %d", r);
            continue;
        }
        if (xpc_get_type(request) == XPC_TYPE_DICTIONARY) handle(request);
        xpc_release(request);
    }
    return NULL;
}

// A job ended: its services get new ports (the old receive rights died
// with it), and it may be started again.
static void ended(pid_t pid, int status) {
    pthread_mutex_lock(&lock);
    for (int i = 0; i < njobs; i++) {
        if (jobs[i].pid != pid) continue;
        say("%s ended, status %x", jobs[i].label, status);
        jobs[i].pid = 0;
        for (int k = 0; k < nservices; k++) {
            if (job_of(&services[k]) != &jobs[i]) continue;
            if (services[k].send) mach_port_deallocate(mach_task_self(), services[k].send);
            if (services[k].recv) mach_port_mod_refs(mach_task_self(), services[k].recv, MACH_PORT_RIGHT_RECEIVE, -1);
            services[k].send = services[k].recv = MACH_PORT_NULL;
        }
    }
    pthread_mutex_unlock(&lock);
}

int main(int argc, char **argv) {
    int dirs = 0, i = 1;
    for (; i < argc; i++) {
        if (!strcmp(argv[i], "-v")) verbose = 1;
        else if (!strcmp(argv[i], "-x") && i + 1 < argc && nskipped < 64) skipped[nskipped++] = argv[++i];
        else if (!strcmp(argv[i], "-u") && i + 1 < argc) {
            char *end;
            user = (uid_t)strtoul(argv[++i], &end, 10);
            group = *end == ':' ? (gid_t)strtoul(end + 1, NULL, 10) : 20;
        }
        else if (!strcmp(argv[i], "-j") && i + 1 < argc) add_dir(argv[++i]), dirs++;
        else if (!strcmp(argv[i], "--")) {
            i++;
            break;
        } else break;
    }
    if (i >= argc) {
        fprintf(stderr, "usage: gexlaunchd [-v] [-j DIR]... [--] PROGRAM [ARG...]\n");
        return 2;
    }
    if (!dirs) {
        add_dir("/System/Library/LaunchDaemons");
        add_dir("/System/Library/LaunchAgents");
    }
    say("%d jobs, %d services", njobs, nservices);
    mach_port_allocate(mach_task_self(), MACH_PORT_RIGHT_RECEIVE, &bootstrap);
    mach_port_insert_right(mach_task_self(), bootstrap, bootstrap, MACH_MSG_TYPE_MAKE_SEND);
    // Its own bootstrap port, so that each process it starts has it.
    task_set_special_port(mach_task_self(), TASK_BOOTSTRAP_PORT, bootstrap);
    if (pipe(wake)) return 1;
    pthread_t t;
    pthread_create(&t, NULL, serve, NULL);
    pid_t first = start(argv + i, NULL, 1);
    int status = 0;
    for (;;) {
        // Jobs to start, and processes that have ended.
        struct pollfd p = {wake[0], POLLIN, 0};
        if (poll(&p, 1, 100) > 0) {
            char buf[64];
            (void)!read(wake[0], buf, sizeof buf);
        }
        pthread_mutex_lock(&lock);
        for (int k = 0; k < njobs; k++)
            if (jobs[k].wanted) start_job(&jobs[k]);
        pthread_mutex_unlock(&lock);
        int st;
        pid_t pid;
        while ((pid = waitpid(-1, &st, WNOHANG)) > 0) {
            if (pid == first) {
                status = st;
                goto done;
            }
            ended(pid, st);
        }
        if (pid < 0 && errno == ECHILD) break;
    }
done:
    for (int k = 0; k < njobs; k++)
        if (jobs[k].pid > 0) kill(jobs[k].pid, SIGTERM);
    return WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
}
