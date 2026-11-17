/* Window procedure calls nested deep in one thread: each level sends the
 * window a message from inside its procedure, so every level is a user
 * callback from native win32u into the i386 program, a call-out on the page.
 * Partway down, and again on the way back up, code the program writes is
 * run, rewritten and run again, and freed: an install and a retirement of
 * translated code while the call-outs above are open. Prints
 * "cbnest ok N SUM" and exits 0 when every level returned its own value in
 * order and the written code gave what it was rewritten to.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

static int depth, bad;
static unsigned char *code;

/* mov eax, v; ret */
static int run_code(unsigned v)
{
    code[0] = 0xb8;
    memcpy(code + 1, &v, 4);
    code[5] = 0xc3;
    FlushInstructionCache(GetCurrentProcess(), code, 6);
    return ((int (*)(void))code)();
}

static void churn(int level)
{
    if (run_code(level * 3 + 1) != level * 3 + 1) bad++;
    if (run_code(level * 7 + 2) != level * 7 + 2) bad++;
    /* Code freed and made again elsewhere: the first page's translations go. */
    unsigned char *old = code;
    code = VirtualAlloc(NULL, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE);
    VirtualFree(old, 0, MEM_RELEASE);
    if (run_code(level) != level) bad++;
}

static LRESULT CALLBACK wp(HWND h, UINT m, WPARAM w, LPARAM l)
{
    if (m != WM_USER) return DefWindowProcA(h, m, w, l);
    int level = (int)w;
    if (level == depth / 2) churn(level);
    LRESULT r = level < depth ? SendMessageA(h, WM_USER, level + 1, 0) : 0;
    if (level < depth && r != (level + 1) * 2 + 1) bad++;
    if (level == depth / 2) churn(level + 1);
    return level * 2 + 1;
}

int main(int argc, char **argv)
{
    WNDCLASSA wc = {0};
    int rounds = argc > 2 ? atoi(argv[2]) : 3, sum = 0;
    depth = argc > 1 ? atoi(argv[1]) : 40;
    code = VirtualAlloc(NULL, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE);
    wc.lpfnWndProc = wp;
    wc.lpszClassName = "cbnest";
    RegisterClassA(&wc);
    HWND h = CreateWindowExA(0, "cbnest", "cbnest", WS_POPUP, 0, 0, 10, 10, NULL, NULL, NULL, NULL);
    for (int i = 0; i < rounds; i++) sum += (int)SendMessageA(h, WM_USER, 0, 0);
    DestroyWindow(h);
    if (bad || sum != rounds) { printf("cbnest FAIL bad %d sum %d\n", bad, sum); return 1; }
    printf("cbnest ok %d %d\n", depth, sum);
    return 0;
}
