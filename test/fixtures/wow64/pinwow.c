/* wow64win's sdwhwin32, the table WoW64 makes an i386 program's win32u
 * calls through, read by an x86_64 program that loads wow64win: its
 * service limit and its first entry. The prefix's dll-bases pins wow64win
 * and the other DLLs of gex's core PE module below 4 GiB (pinwow.test.js).
 * Prints the DLL's base, then "pinwow ok". */
#include <windows.h>
#include <stdio.h>

typedef struct
{
    ULONG_PTR *ServiceTable;
    ULONG_PTR *CounterTable;
    ULONG_PTR ServiceLimit;
    BYTE *ArgumentTable;
} SERVICE_TABLE;

int main(void)
{
    HMODULE m = LoadLibraryA("wow64win.dll");
    SERVICE_TABLE *t = m ? (void *)GetProcAddress(m, "sdwhwin32") : NULL;

    setvbuf(stdout, NULL, _IONBF, 0);
    printf("pinwow base %p, sdwhwin32 %p\n", m, t);
    if (!t) { printf("pinwow FAIL no sdwhwin32\n"); return 1; }
    printf("pinwow: %lu services, the first at %p\n", (unsigned long)t->ServiceLimit, t->ServiceTable ? (void *)t->ServiceTable[0] : NULL);
    if (t->ServiceLimit < 100 || !t->ServiceTable || !t->ServiceTable[0]) { printf("pinwow FAIL\n"); return 1; }
    printf("pinwow ok\n");
    return 0;
}
