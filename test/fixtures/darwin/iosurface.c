// IOSurface's public API (IOSurface.framework), as a program uses it: a
// surface made from a dictionary of its properties, what it says of its
// shape, locked and written and read back, its seed moving with a write,
// a value set and copied, its use count, looked up again by its id and by
// a Mach port made of it; and looked up from another process (a global
// surface, by id), which writes into it for this one to see.
//
// Built against the Mac's SDK and run on the Mac as the reference; under
// gex it runs on Mojave's own IOSurface.framework (test/darwin/iosurface),
// whose kernel half is gex's (crates/gex-kernel/src/darwin/iokit/surface.rs).
// Only what both kernels must agree on is printed: no addresses, no sizes
// a driver rounds as it likes.
#include <CoreFoundation/CoreFoundation.h>
#include <IOSurface/IOSurface.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>

extern char **environ;

static CFNumberRef num(int v) { return CFNumberCreate(NULL, kCFNumberIntType, &v); }

static void put(CFMutableDictionaryRef d, CFStringRef k, int v) {
    CFNumberRef n = num(v);
    CFDictionarySetValue(d, k, n);
    CFRelease(n);
}

static int failures;
static void check(const char *what, int ok) {
    printf("%s %s\n", ok ? "ok" : "FAIL", what);
    if (!ok) failures++;
}

enum { W = 64, H = 32 };

// The sum of the surface's pixels, row by row, read under a read lock.
static unsigned long sum(IOSurfaceRef s) {
    IOSurfaceLock(s, kIOSurfaceLockReadOnly, NULL);
    unsigned char *base = IOSurfaceGetBaseAddress(s);
    size_t bpr = IOSurfaceGetBytesPerRow(s);
    unsigned long total = 0;
    for (int y = 0; y < H; y++) {
        unsigned int *row = (unsigned int *)(base + y * bpr);
        for (int x = 0; x < W; x++) total += row[x];
    }
    IOSurfaceUnlock(s, kIOSurfaceLockReadOnly, NULL);
    return total;
}

static void fill(IOSurfaceRef s, unsigned int seed) {
    IOSurfaceLock(s, 0, NULL);
    unsigned char *base = IOSurfaceGetBaseAddress(s);
    size_t bpr = IOSurfaceGetBytesPerRow(s);
    for (int y = 0; y < H; y++) {
        unsigned int *row = (unsigned int *)(base + y * bpr);
        for (int x = 0; x < W; x++) row[x] = seed + (unsigned)(y * W + x);
    }
    IOSurfaceUnlock(s, 0, NULL);
}

// The other process: the surface by its id, its pixels, and new ones.
static int child(unsigned int id) {
    IOSurfaceRef s = IOSurfaceLookup(id);
    check("the other process looks the surface up by its id", s != NULL);
    if (!s) return 1;
    check("and finds it the same shape", IOSurfaceGetWidth(s) == W && IOSurfaceGetHeight(s) == H);
    printf("the other process reads %lu\n", sum(s));
    fill(s, 1000);
    CFRelease(s);
    return failures != 0;
}

int main(int argc, char **argv) {
    if (argc == 3 && strcmp(argv[1], "child") == 0) return child((unsigned)strtoul(argv[2], NULL, 10));
    setvbuf(stdout, NULL, _IONBF, 0);

    CFMutableDictionaryRef props = CFDictionaryCreateMutable(NULL, 0, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    put(props, kIOSurfaceWidth, W);
    put(props, kIOSurfaceHeight, H);
    put(props, kIOSurfaceBytesPerElement, 4);
    put(props, kIOSurfacePixelFormat, 'BGRA');
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    CFDictionarySetValue(props, kIOSurfaceIsGlobal, kCFBooleanTrue);
#pragma clang diagnostic pop
    IOSurfaceRef s = IOSurfaceCreate(props);
    check("a surface is made", s != NULL);
    if (!s) return 1;
    printf("%zux%zu, %zu bytes an element, format %c%c%c%c, %zu planes\n", IOSurfaceGetWidth(s), IOSurfaceGetHeight(s), IOSurfaceGetBytesPerElement(s),
           (int)(IOSurfaceGetPixelFormat(s) >> 24), (int)(IOSurfaceGetPixelFormat(s) >> 16 & 0xff), (int)(IOSurfaceGetPixelFormat(s) >> 8 & 0xff),
           (int)(IOSurfaceGetPixelFormat(s) & 0xff), IOSurfaceGetPlaneCount(s));
    check("a row holds the width", IOSurfaceGetBytesPerRow(s) >= W * 4);
    check("the allocation holds the rows", IOSurfaceGetAllocSize(s) >= IOSurfaceGetBytesPerRow(s) * H);
    check("the surface has an id", IOSurfaceGetID(s) != 0);

    // A write moves the seed on; a read does not.
    uint32_t seed0 = IOSurfaceGetSeed(s);
    fill(s, 7);
    uint32_t seed1 = IOSurfaceGetSeed(s);
    check("a write's unlock moves the seed", seed1 != seed0);
    unsigned long want = 0;
    for (int i = 0; i < W * H; i++) want += 7u + (unsigned)i;
    check("what was written is read back", sum(s) == want);
    check("a read leaves the seed", IOSurfaceGetSeed(s) == seed1);

    // Values.
    CFNumberRef v = num(42);
    IOSurfaceSetValue(s, CFSTR("gex"), v);
    CFRelease(v);
    CFTypeRef got = IOSurfaceCopyValue(s, CFSTR("gex"));
    int n = 0;
    if (got && CFGetTypeID(got) == CFNumberGetTypeID()) CFNumberGetValue(got, kCFNumberIntType, &n);
    printf("value gex = %d\n", n);
    if (got) CFRelease(got);
    IOSurfaceRemoveValue(s, CFSTR("gex"));
    got = IOSurfaceCopyValue(s, CFSTR("gex"));
    check("a removed value is gone", got == NULL);
    if (got) CFRelease(got);
    check("no value of a name never set", IOSurfaceCopyValue(s, CFSTR("never")) == NULL);

    // Use counts.
    check("not in use at first", !IOSurfaceIsInUse(s));
    IOSurfaceIncrementUseCount(s);
    check("in use once counted", IOSurfaceIsInUse(s) && IOSurfaceGetUseCount(s) == 1);
    IOSurfaceDecrementUseCount(s);
    check("not in use when counted down", !IOSurfaceIsInUse(s) && IOSurfaceGetUseCount(s) == 0);

    // Looked up again: the same surface.
    IOSurfaceID id = IOSurfaceGetID(s);
    IOSurfaceRef again = IOSurfaceLookup(id);
    check("looked up by its id", again != NULL && IOSurfaceGetID(again) == id && sum(again) == want);
    if (again) CFRelease(again);
    mach_port_t port = IOSurfaceCreateMachPort(s);
    check("a Mach port is made of it", port != MACH_PORT_NULL);
    IOSurfaceRef by_port = IOSurfaceLookupFromMachPort(port);
    check("looked up by the port", by_port != NULL && IOSurfaceGetID(by_port) == id && sum(by_port) == want);
    if (by_port) CFRelease(by_port);
    mach_port_deallocate(mach_task_self(), port);
    check("no surface of an id never made", IOSurfaceLookup(0x7fffff) == NULL);

    // From another process: it reads what this one wrote, and writes.
    char ids[16];
    snprintf(ids, sizeof ids, "%u", id);
    char *args[] = { argv[0], "child", ids, NULL };
    pid_t pid;
    int st = 0;
    check("the other process starts", posix_spawn(&pid, argv[0], NULL, NULL, args, environ) == 0 && waitpid(pid, &st, 0) == pid);
    check("and succeeds", WIFEXITED(st) && WEXITSTATUS(st) == 0);
    unsigned long theirs = 0;
    for (int i = 0; i < W * H; i++) theirs += 1000u + (unsigned)i;
    check("this process sees what the other wrote", sum(s) == theirs);

    CFRelease(s);
    CFRelease(props);
    return failures != 0;
}
