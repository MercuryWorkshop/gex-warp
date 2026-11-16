/* A pinned DLL's data exports resolve to its data: msvcr120's __argc and
 * _environ through GetProcAddress, against the addresses its own functions
 * return for them. The prefix's dll-bases pins msvcr120 (pindata.test.js),
 * a DLL with a module of its own, which its LoadLibrary installs. */
#include <windows.h>
#include <stdio.h>

int main(void)
{
    HMODULE m = LoadLibraryA("msvcr120.dll");
    int *(__cdecl *p_argc)(void);
    char ***(__cdecl *p_environ)(void);
    void *argc, *environ;
    int bad = 0;

    if (!m) { printf("pindata FAIL no msvcr120\n"); return 1; }
    p_argc = (void *)GetProcAddress(m, "__p___argc");
    p_environ = (void *)GetProcAddress(m, "__p__environ");
    argc = GetProcAddress(m, "__argc");
    environ = GetProcAddress(m, "_environ");
    printf("pindata base %p\n", m);
    if (!p_argc || argc != p_argc()) { printf("pindata FAIL __argc %p, __p___argc %p\n", argc, p_argc ? p_argc() : NULL); bad = 1; }
    if (!p_environ || environ != p_environ()) { printf("pindata FAIL _environ %p, __p__environ %p\n", environ, p_environ ? p_environ() : NULL); bad = 1; }
    if (!bad) printf("pindata ok\n");
    return bad;
}
