// gexloginwindow: what loginwindow does for WindowServer, in its place.
//
// A Mac's loginwindow is the program that owns the console's session: the
// one WindowServer draws, until somebody logs in and after. (Apple's is a
// protected binary, which gex does not run.) WindowServer starts with a
// session of its own making that nobody owns. What it waits for is narrow,
// and this is that part:
//
// - an audit session for the login (`setaudit_addr`, with a new id): its
//   audit user the console's user, and the flags that say it reaches the
//   window server, a terminal and the console, and that its user has
//   authenticated. loginwindow has launchd make its session; here the
//   program makes its own, as root;
// - the connection to WindowServer taken as loginwindow's
//   (`SLSSetLoginwindowConnection`): WindowServer gives its session the
//   caller's audit session, and the caller is its owner. It asks for the
//   entitlement com.apple.private.SkyLight.loginwindow, which this
//   program's signature carries (build.sh);
// - the console given to the user: its device's owner, and the login name;
// - and something to show: a window the size of the display, filled, with
//   a square in it, which WindowServer composites to the framebuffer; and
//   what WindowServer says the display then shows (its image of it).
//
// - and then, given a program, the user's session itself: the program run
//   as the user, in the login's audit session (which a child inherits),
//   with the user's groups, home and name, as loginwindow starts the
//   user's applications once they have logged in. (Its windows are
//   ordered in above this one's: the desktop's level is WindowServer's
//   own to draw, and shows nothing of a window put there.)
//
//   gexloginwindow [-u UID[:GID]] [-n NAME] [-- PROGRAM [ARG...]]
//
// It says what it does on stdout, a line each, and then stays, as
// loginwindow does: the session ends with it.
#include <ApplicationServices/ApplicationServices.h>
#include <bsm/audit.h>
#include <errno.h>
#include <grp.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

// SkyLight's own interface (not in the SDK's headers).
int SLSMainConnectionID(void);
CGError SLSSetLoginwindowConnection(int cid);
CGError SLSNewWindow(int cid, int backing, float x, float y, CFTypeRef region, uint32_t *wid);
CGError SLSSetWindowOpacity(int cid, uint32_t wid, bool opaque);
CGError SLSSetWindowLevel(int cid, uint32_t wid, int level);
CGError SLSOrderWindow(int cid, uint32_t wid, int place, uint32_t relative);
CGContextRef SLWindowContextCreate(int cid, uint32_t wid, CFDictionaryRef options);
CGError CGSNewRegionWithRect(const CGRect *rect, CFTypeRef *region);

enum {
    kBackingBuffered = 2,
    kOrderAbove = 1,
};

static void say_session(const char *when) {
    auditinfo_addr_t ai;
    memset(&ai, 0, sizeof ai);
    int r = getaudit_addr(&ai, sizeof ai);
    printf("gexloginwindow: %s: uid %d, audit session %d (%s), audit user %d, flags 0x%llx\n", when, (int)geteuid(), (int)ai.ai_asid,
           r ? strerror(errno) : "ok", (int)ai.ai_auid, (unsigned long long)ai.ai_flags);
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    uid_t user = 501;
    gid_t group = 20;
    const char *name = "gex";
    char **program = NULL;
    for (int i = 1; i < argc; i += 2) {
        if (!strcmp(argv[i], "--")) {
            if (i + 1 < argc) program = argv + i + 1;
            break;
        }
        if (i + 1 >= argc) break;
        if (!strcmp(argv[i], "-u")) {
            char *end;
            user = (uid_t)strtoul(argv[i + 1], &end, 10);
            if (*end == ':') group = (gid_t)strtoul(end + 1, NULL, 10);
        } else if (!strcmp(argv[i], "-n")) name = argv[i + 1];
    }
    say_session("at the start");

    // The login's audit session: a new one, the console user's.
    auditinfo_addr_t ai;
    memset(&ai, 0, sizeof ai);
    ai.ai_auid = user;
    ai.ai_asid = AU_ASSIGN_ASID;
    ai.ai_termid.at_type = AU_IPv4;
    ai.ai_flags = AU_SESSION_FLAG_HAS_GRAPHIC_ACCESS | AU_SESSION_FLAG_HAS_TTY | AU_SESSION_FLAG_HAS_CONSOLE_ACCESS | AU_SESSION_FLAG_HAS_AUTHENTICATED;
    if (setaudit_addr(&ai, sizeof ai)) printf("gexloginwindow: setaudit_addr: %s\n", strerror(errno));
    say_session("in the login's session");

    // The console is the user's.
    if (chown("/dev/console", user, group)) printf("gexloginwindow: chown /dev/console: %s\n", strerror(errno));
    if (setlogin(name)) printf("gexloginwindow: setlogin: %s\n", strerror(errno));

    // WindowServer's session is this one's from here.
    int cid = SLSMainConnectionID();
    printf("gexloginwindow: connection %d\n", cid);
    CGError e = SLSSetLoginwindowConnection(cid);
    printf("gexloginwindow: SLSSetLoginwindowConnection -> %d\n", (int)e);

    CGDirectDisplayID display = CGMainDisplayID();
    CGRect bounds = CGDisplayBounds(display);
    printf("gexloginwindow: main display %u, %.0fx%.0f at %.0f,%.0f, %zux%zu pixels\n", display, bounds.size.width, bounds.size.height,
           bounds.origin.x, bounds.origin.y, CGDisplayPixelsWide(display), CGDisplayPixelsHigh(display));

    // A window over the whole display, as loginwindow's backdrop is.
    CGRect frame = CGRectMake(0, 0, bounds.size.width, bounds.size.height);
    CFTypeRef region = NULL;
    e = CGSNewRegionWithRect(&frame, &region);
    printf("gexloginwindow: region -> %d\n", (int)e);
    uint32_t wid = 0;
    e = SLSNewWindow(cid, kBackingBuffered, (float)bounds.origin.x, (float)bounds.origin.y, region, &wid);
    printf("gexloginwindow: SLSNewWindow -> %d, window %u\n", (int)e, wid);
    e = SLSSetWindowOpacity(cid, wid, true);
    printf("gexloginwindow: SLSSetWindowOpacity -> %d\n", (int)e);
    CGContextRef c = SLWindowContextCreate(cid, wid, NULL);
    printf("gexloginwindow: context %p\n", (void *)c);
    if (c) {
        CGContextSetRGBFillColor(c, 0.20, 0.45, 0.75, 1);
        CGContextFillRect(c, frame);
        CGContextSetRGBFillColor(c, 1, 1, 1, 1);
        CGContextFillRect(c, CGRectMake(100, 100, 200, 200));
        CGContextFlush(c);
        printf("gexloginwindow: drawn\n");
    }
    e = SLSOrderWindow(cid, wid, kOrderAbove, 0);
    printf("gexloginwindow: SLSOrderWindow -> %d\n", (int)e);
    if (c) CGContextFlush(c);
    // What WindowServer shows of it: the display as it composites it.
    sleep(2);
    printf("gexloginwindow: display online %d, active %d, asleep %d\n", (int)CGDisplayIsOnline(display), (int)CGDisplayIsActive(display),
           (int)CGDisplayIsAsleep(display));
    CGImageRef shown = CGDisplayCreateImage(display);
    CFDataRef pixels = shown ? CGDataProviderCopyData(CGImageGetDataProvider(shown)) : NULL;
    if (pixels) {
        const uint8_t *p = CFDataGetBytePtr(pixels);
        size_t row = CGImageGetBytesPerRow(shown), w = CGImageGetWidth(shown), h = CGImageGetHeight(shown);
        const uint8_t *in = p + 50 * row + 50 * 4, *square = p + 200 * row + 200 * 4;
        printf("gexloginwindow: shown %zux%zu: at 50,50 %02x%02x%02x, at 200,200 %02x%02x%02x\n", w, h, in[2], in[1], in[0], square[2], square[1], square[0]);
    } else {
        printf("gexloginwindow: shown: no image (%p)\n", (void *)shown);
    }
    printf("gexloginwindow: up\n");
    if (!program) for (;;) pause();

    // The user's session: the program, as the user.
    struct passwd *pw = getpwuid(user);
    const char *home = pw && pw->pw_dir ? pw->pw_dir : "/";
    pid_t pid = fork();
    if (pid == 0) {
        if (setgid(group) || initgroups(name, (int)group) || setuid(user)) {
            printf("gexloginwindow: becoming %d: %s\n", (int)user, strerror(errno));
            _exit(126);
        }
        setenv("HOME", home, 1);
        setenv("USER", name, 1);
        setenv("LOGNAME", name, 1);
        setenv("SHELL", pw && pw->pw_shell ? pw->pw_shell : "/bin/sh", 1);
        if (chdir(home)) (void)!chdir("/");
        execv(program[0], program);
        printf("gexloginwindow: %s: %s\n", program[0], strerror(errno));
        _exit(127);
    }
    printf("gexloginwindow: started %s as %d, pid %d\n", program[0], (int)user, (int)pid);
    int status;
    while (pid > 0 && waitpid(pid, &status, 0) < 0 && errno == EINTR) {}
    if (pid > 0) {
        if (WIFSIGNALED(status)) printf("gexloginwindow: %s: signal %d\n", program[0], WTERMSIG(status));
        else printf("gexloginwindow: %s: exit %d\n", program[0], WEXITSTATUS(status));
    }
    for (;;) pause();
}
