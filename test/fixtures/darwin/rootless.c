// What System Integrity Protection keeps, as libsystem_sandbox asks it
// (rootless_check_trusted, gpu_bundle_is_path_trusted): a sandbox check
// (__mac_syscall "Sandbox", call 2) of whether launchd, pid 1, may write a
// file by its path. A file the system keeps nobody may write: denied (1),
// and so trusted. Elsewhere allowed (0). And the same check of this
// process, which no sandbox holds: the same, as nobody may write what the
// system keeps.
#include "t.h"

enum { SYS___mac_syscall = 381 };

static void check(long pid, const char *path, const char *class) {
    long result = -1;
    long args[8] = {(long)&result, pid, (long)"file-write-data", 1, (long)path, 0x20000015, 0, (long)class};
    long r = SYS(SYS___mac_syscall, "Sandbox", 2, args);
    t_printf("%s %s%s%s: %ld, denied %ld\n", pid == 1 ? "launchd" : "this process", path, class ? " as " : "", class ? class : "", r, r == 0 ? result : -1);
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    static const char *paths[] = {"/System/Library/CoreServices/SystemVersion.plist", "/bin/sh", "/usr/bin/true", "/sbin/mount", "/usr/local", "/private/tmp", "/Library/Preferences"};
    for (unsigned i = 0; i < sizeof paths / sizeof *paths; i++) check(1, paths[i], 0);
    check(1, "/System/Library/CoreServices/SystemVersion.plist", "KernelExtensionManagement");
    check(1, "/private/tmp", "KernelExtensionManagement");
    long me = SYS(SYS_getpid);
    check(me, "/System/Library/CoreServices/SystemVersion.plist", 0);
    check(me, "/private/tmp", 0);
    return 0;
}
