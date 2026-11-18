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
// - checked in as an application (the Process Manager's GetCurrentProcess:
//   LaunchServices and WindowServer's process list): the first root
//   process named loginwindow to do so gives the process list its session,
//   which WindowServer's front process, and so every application's window
//   being ordered in, needs;
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
//   gexloginwindow [-u UID[:GID]] [-n NAME] [-s PROGRAM]... [-t SECS] [-w SECS] [-o SECS] [-- PROGRAM [ARG...]]
//
// -s starts a program of the session's own first, as the user, and does
// not wait for it: the Dock, SystemUIServer and the Finder, as loginwindow
// has the session's launchd start them (their jobs offered by gexlaunchd
// -o, for them to check in to).
//
// -w SECS looks at WindowServer's windows every SECS and says them, each
// time they have changed: whose they are, where, their level, whether they
// are on the screen (whether an application's window was made and ordered
// in).
//
// -o SECS sends PROGRAM the open-application Apple Event ('oapp') every
// SECS until it takes one, as loginwindow's launch of an application
// through LaunchServices hands it one: AppKit finishes launching an
// application (applicationDidFinishLaunching, where Calculator orders its
// window in) on its first open event. (AE.framework makes one up for an
// application nobody launched with one, unless appleeventsd says one is to
// come; here none came.)
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
CGError SLSSetSystemDefinedCursor(int cid, int cursor);
CGError SLSShowCursor(int cid);
bool SLCursorIsVisible(void);
CGError SLSGetCurrentCursorLocation(int cid, CGPoint *at);
CGError SLSWindowIsOrderedIn(int cid, uint32_t wid, bool *in);
CFArrayRef SLSCopySpacesForWindows(int cid, int mask, CFArrayRef windows);
uint64_t SLSGetActiveSpace(int cid);
CGError SLPSGetFrontProcess(ProcessSerialNumber *psn);
CGError SLSGetConnectionIDForPSN(int cid, const ProcessSerialNumber *psn, int *owner);
CGError SLSConnectionGetPID(int cid, pid_t *pid);

enum {
    kBackingBuffered = 2,
    kOrderAbove = 1,
};

// The windows WindowServer has, a line each (`CGWindowListCopyWindowInfo`
// of them all), said when they are not as they were the last time: and
// which is ordered in, the spaces each is on, the space shown, and the
// front process (whose windows an application's ordered "conditionally"
// go in front of).
static char *windows_said;

static void say_windows(void) {
    int cid = SLSMainConnectionID();
    CFArrayRef list = CGWindowListCopyWindowInfo(kCGWindowListOptionAll, kCGNullWindowID);
    size_t cap = 256, len = 0;
    char *out = malloc(cap);
    out[0] = 0;
    CFIndex n = list ? CFArrayGetCount(list) : 0;
    for (CFIndex i = 0; i < n; i++) {
        CFDictionaryRef w = CFArrayGetValueAtIndex(list, i);
        int number = 0, pid = 0, layer = 0, onscreen = 0;
        double alpha = 0;
        CFNumberRef v;
        if ((v = CFDictionaryGetValue(w, kCGWindowNumber))) CFNumberGetValue(v, kCFNumberIntType, &number);
        if ((v = CFDictionaryGetValue(w, kCGWindowOwnerPID))) CFNumberGetValue(v, kCFNumberIntType, &pid);
        if ((v = CFDictionaryGetValue(w, kCGWindowLayer))) CFNumberGetValue(v, kCFNumberIntType, &layer);
        if ((v = CFDictionaryGetValue(w, kCGWindowAlpha))) CFNumberGetValue(v, kCFNumberDoubleType, &alpha);
        CFBooleanRef on = CFDictionaryGetValue(w, kCGWindowIsOnscreen);
        onscreen = on && CFBooleanGetValue(on);
        CGRect r = CGRectZero;
        CFDictionaryRef b = CFDictionaryGetValue(w, kCGWindowBounds);
        if (b) CGRectMakeWithDictionaryRepresentation(b, &r);
        char owner[64] = "", title[64] = "";
        CFStringRef s = CFDictionaryGetValue(w, kCGWindowOwnerName);
        if (s) CFStringGetCString(s, owner, sizeof owner, kCFStringEncodingUTF8);
        s = CFDictionaryGetValue(w, kCGWindowName);
        if (s) CFStringGetCString(s, title, sizeof title, kCFStringEncodingUTF8);
        bool in = false;
        SLSWindowIsOrderedIn(cid, (uint32_t)number, &in);
        char spaces[64] = "";
        CFNumberRef one = CFNumberCreate(NULL, kCFNumberIntType, &number);
        CFArrayRef ids = CFArrayCreate(NULL, (const void **)&one, 1, &kCFTypeArrayCallBacks);
        CFArrayRef where = ids ? SLSCopySpacesForWindows(cid, 7, ids) : NULL;
        for (CFIndex k = 0; where && k < CFArrayGetCount(where) && strlen(spaces) < sizeof spaces - 24; k++) {
            long long space = 0;
            CFNumberGetValue(CFArrayGetValueAtIndex(where, k), kCFNumberLongLongType, &space);
            snprintf(spaces + strlen(spaces), sizeof spaces - strlen(spaces), "%s%lld", k ? "," : "", space);
        }
        if (where) CFRelease(where);
        if (ids) CFRelease(ids);
        CFRelease(one);
        char line[320];
        int k = snprintf(line, sizeof line, "gexloginwindow: window %d of %d (%s) \"%s\": %.0fx%.0f at %.0f,%.0f, layer %d, alpha %.2f, %s, %s, spaces [%s]\n", number, pid,
                         owner, title, r.size.width, r.size.height, r.origin.x, r.origin.y, layer, alpha, onscreen ? "on screen" : "off screen",
                         in ? "ordered in" : "not ordered in", spaces);
        if (k < 0) continue;
        if (len + (size_t)k + 1 > cap) out = realloc(out, cap = (len + k + 1) * 2);
        memcpy(out + len, line, (size_t)k + 1);
        len += (size_t)k;
    }
    if (list) CFRelease(list);
    ProcessSerialNumber front = {0, 0};
    int front_cid = 0;
    pid_t front_pid = -1;
    if (!SLPSGetFrontProcess(&front) && !SLSGetConnectionIDForPSN(cid, &front, &front_cid)) SLSConnectionGetPID(front_cid, &front_pid);
    char tail[160];
    int k = snprintf(tail, sizeof tail, "gexloginwindow: space %llu shown; front process %u/%u (pid %d)\n", (unsigned long long)SLSGetActiveSpace(cid),
                     (unsigned)front.highLongOfPSN, (unsigned)front.lowLongOfPSN, (int)front_pid);
    if (k > 0) {
        if (len + (size_t)k + 1 > cap) out = realloc(out, cap = (len + k + 1) * 2);
        memcpy(out + len, tail, (size_t)k + 1);
        len += (size_t)k;
    }
    if (windows_said && !strcmp(windows_said, out)) {
        free(out);
        return;
    }
    printf("gexloginwindow: %ld windows\n%s", (long)n, out);
    free(windows_said);
    windows_said = out;
}

static void say_session(const char *when) {
    auditinfo_addr_t ai;
    memset(&ai, 0, sizeof ai);
    int r = getaudit_addr(&ai, sizeof ai);
    printf("gexloginwindow: %s: uid %d, audit session %d (%s), audit user %d, flags 0x%llx\n", when, (int)geteuid(), (int)ai.ai_asid,
           r ? strerror(errno) : "ok", (int)ai.ai_auid, (unsigned long long)ai.ai_flags);
}

// `argv` run as the user, in this session, from the home: its pid. (A
// program that cannot be run says so, and ends with 127.)
static pid_t start_as(char **argv, uid_t user, gid_t group, const char *name, const char *home, struct passwd *pw) {
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
        execv(argv[0], argv);
        printf("gexloginwindow: %s: %s\n", argv[0], strerror(errno));
        _exit(127);
    }
    return pid;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    uid_t user = 501;
    gid_t group = 20;
    const char *name = "gex";
    char **program = NULL;
    const char *session[8];
    int nsession = 0;
    int ticks = 0, looks = 0, opens = 0;
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
        else if (!strcmp(argv[i], "-s") && nsession < 8) session[nsession++] = argv[i + 1];
        else if (!strcmp(argv[i], "-t")) ticks = atoi(argv[i + 1]);
        else if (!strcmp(argv[i], "-w")) looks = atoi(argv[i + 1]);
        else if (!strcmp(argv[i], "-o")) opens = atoi(argv[i + 1]);
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
    // And loginwindow checked in as the session's process (the Process
    // Manager's check-in: LaunchServices, then WindowServer's process
    // list, as an application does), which gives WindowServer's process
    // list the login's session (`_XCreateApplication`: the first euid-0
    // process named loginwindow sets it). Without it WindowServer knows of
    // no front process (CPXGetFrontProcess: procNotFound) and orders no
    // application's window in: Calculator's stayed off screen after its
    // open event. On a thread of its own: the check-in waits on lsd.
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        ProcessSerialNumber me = {0, 0};
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
        OSErr pe = GetCurrentProcess(&me);
#pragma clang diagnostic pop
        printf("gexloginwindow: checked in as the session's process: %d, psn %u/%u\n", (int)pe, (unsigned)me.highLongOfPSN, (unsigned)me.lowLongOfPSN);
    });

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
    // The pointer: WindowServer starts with it hidden and no cursor of
    // its own, and loginwindow gives it the arrow (the system's cursor 0)
    // and shows it.
    e = SLSSetSystemDefinedCursor(cid, 0);
    CGError shown_e = SLSShowCursor(cid);
    CGPoint at = {0, 0};
    SLSGetCurrentCursorLocation(cid, &at);
    printf("gexloginwindow: the arrow -> %d, shown -> %d, visible %d, at %.0f,%.0f\n", (int)e, (int)shown_e, (int)SLCursorIsVisible(), at.x, at.y);
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

    // The user's session: the session's own programs, then the program,
    // each as the user.
    struct passwd *pw = getpwuid(user);
    const char *home = pw && pw->pw_dir ? pw->pw_dir : "/";
    for (int i = 0; i < nsession; i++) {
        char *one[] = {(char *)session[i], NULL};
        pid_t p = start_as(one, user, group, name, home, pw);
        printf("gexloginwindow: started %s as %d, pid %d (the session's)\n", session[i], (int)user, (int)p);
    }
    // -t SECS: the square drawn again every SECS, in turn white and
    // black, for as long as this runs: whether what WindowServer shows
    // follows a window's drawing after the first.
    if (ticks > 0 && c) {
        dispatch_source_t timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_main_queue());
        dispatch_source_set_timer(timer, dispatch_time(DISPATCH_TIME_NOW, (int64_t)ticks * NSEC_PER_SEC), (uint64_t)ticks * NSEC_PER_SEC, NSEC_PER_SEC / 10);
        __block int n = 0;
        dispatch_source_set_event_handler(timer, ^{
            n++;
            CGFloat v = n % 2 ? 0 : 1;
            CGContextSetRGBFillColor(c, v, v, v, 1);
            CGContextFillRect(c, CGRectMake(100, 100, 200, 200));
            CGContextFlush(c);
            printf("gexloginwindow: tick %d, the square %s\n", n, n % 2 ? "black" : "white");
        });
        dispatch_resume(timer);
    }
    if (looks > 0) {
        say_windows();
        dispatch_source_t look = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_main_queue());
        dispatch_source_set_timer(look, dispatch_time(DISPATCH_TIME_NOW, (int64_t)looks * NSEC_PER_SEC), (uint64_t)looks * NSEC_PER_SEC, NSEC_PER_SEC / 10);
        dispatch_source_set_event_handler(look, ^{
            say_windows();
        });
        dispatch_resume(look);
    }
    if (!program) dispatch_main();
    pid_t pid = start_as(program, user, group, name, home, pw);
    printf("gexloginwindow: started %s as %d, pid %d\n", program[0], (int)user, (int)pid);
    if (opens > 0 && pid > 0) {
        // Its open event, sent until it is taken (an application not yet
        // up to receive one: procNotFound, or the send times out).
        dispatch_source_t open = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_global_queue(QOS_CLASS_UTILITY, 0));
        dispatch_source_set_timer(open, dispatch_time(DISPATCH_TIME_NOW, (int64_t)opens * NSEC_PER_SEC), (uint64_t)opens * NSEC_PER_SEC, NSEC_PER_SEC);
        __block int tries = 0;
        dispatch_source_set_event_handler(open, ^{
            AEAddressDesc target;
            AppleEvent event = {typeNull, NULL};
            OSStatus e = AECreateDesc(typeKernelProcessID, &pid, sizeof pid, &target);
            if (e == noErr) {
                e = AECreateAppleEvent(kCoreEventClass, kAEOpenApplication, &target, kAutoGenerateReturnID, kAnyTransactionID, &event);
                AEDisposeDesc(&target);
            }
            if (e == noErr) {
                e = AESendMessage(&event, NULL, kAENoReply, 60 * 60);
                AEDisposeDesc(&event);
            }
            tries++;
            if (e == noErr || tries % 10 == 1) printf("gexloginwindow: open event to %d: %d (try %d)\n", (int)pid, (int)e, tries);
            if (e == noErr) dispatch_source_cancel(open);
        });
        dispatch_resume(open);
    }
    // (Its end waited for on a thread of its own: the main one serves
    // the ticks.)
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        int status;
        while (pid > 0 && waitpid(pid, &status, 0) < 0 && errno == EINTR) {}
        if (pid > 0) {
            if (WIFSIGNALED(status)) printf("gexloginwindow: %s: signal %d\n", program[0], WTERMSIG(status));
            else printf("gexloginwindow: %s: exit %d\n", program[0], WEXITSTATUS(status));
        }
    });
    dispatch_main();
}
