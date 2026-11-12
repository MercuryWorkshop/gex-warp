// What a Mac program is told of the machine and the system it is on, by
// each way it can ask: sysctl, uname, Foundation, gethostuuid; and what its
// bottle says of itself (gex.bottle.*). With arguments, it first does what
// they say, so that one run can show what a bottle lets a program change:
//   write PATH TEXT   make the file (or say why not)
//   read PATH         print it
#import <Foundation/Foundation.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/sysctl.h>
#include <sys/utsname.h>
#include <unistd.h>
#include <uuid/uuid.h>

static void text(const char *name) {
    char buf[512];
    size_t n = sizeof buf;
    if (sysctlbyname(name, buf, &n, NULL, 0) != 0) printf("%s: error %d\n", name, errno);
    else printf("%s: %s\n", name, buf);
}

static void number(const char *name, int hex) {
    int v = 0;
    size_t n = sizeof v;
    if (sysctlbyname(name, &v, &n, NULL, 0) != 0) printf("%s: error %d\n", name, errno);
    else if (hex) printf("%s: 0x%08x\n", name, (unsigned)v);
    else printf("%s: %d\n", name, v);
}

int main(int argc, char **argv) {
    @autoreleasepool {
        for (int i = 1; i < argc; i++) {
            if (!strcmp(argv[i], "write") && i + 2 < argc) {
                FILE *f = fopen(argv[i + 1], "w");
                if (f) {
                    fprintf(f, "%s\n", argv[i + 2]);
                    fclose(f);
                    printf("write %s: done\n", argv[i + 1]);
                } else {
                    printf("write %s: error %d\n", argv[i + 1], errno);
                }
                i += 2;
            } else if (!strcmp(argv[i], "read") && i + 1 < argc) {
                char line[256] = "";
                FILE *f = fopen(argv[i + 1], "r");
                if (f) {
                    if (!fgets(line, sizeof line, f)) line[0] = 0;
                    fclose(f);
                    line[strcspn(line, "\n")] = 0;
                    printf("read %s: %s\n", argv[i + 1], line);
                } else {
                    printf("read %s: error %d\n", argv[i + 1], errno);
                }
                i += 1;
            }
        }
        text("hw.model");
        text("hw.target");
        text("hw.targettype");
        text("machdep.cpu.brand_string");
        number("hw.cpufamily", 1);
        number("hw.cpusubfamily", 0);
        number("hw.ncpu", 0);
        number("hw.physicalcpu", 0);
        number("hw.nperflevels", 0);
        number("hw.perflevel0.physicalcpu", 0);
        number("hw.perflevel1.physicalcpu", 0);
        number("sysctl.proc_translated", 0);
        text("kern.osproductversion");
        text("kern.osversion");
        text("kern.osrelease");
        text("kern.version");
        struct utsname u;
        if (uname(&u) == 0) printf("uname: %s %s %s %s\n", u.sysname, u.release, u.version, u.machine);
        NSOperatingSystemVersion v = [NSProcessInfo processInfo].operatingSystemVersion;
        printf("foundation: %ld.%ld.%ld\n", (long)v.majorVersion, (long)v.minorVersion, (long)v.patchVersion);
        printf("foundation: %s\n", [NSProcessInfo processInfo].operatingSystemVersionString.UTF8String);
        printf("foundation: %lu processors\n", (unsigned long)[NSProcessInfo processInfo].processorCount);
        uuid_t id;
        struct timespec wait = {1, 0};
        if (gethostuuid(id, &wait) == 0) {
            uuid_string_t s;
            uuid_unparse_upper(id, s);
            printf("hostuuid: %s\n", s);
        } else {
            printf("hostuuid: error %d\n", errno);
        }
        text("gex.bottle.name");
        text("gex.bottle.graphics");
        text("gex.bottle.serial");
    }
    return 0;
}
