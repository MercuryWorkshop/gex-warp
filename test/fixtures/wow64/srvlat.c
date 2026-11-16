/* Wineserver round trips as an i386 program sees them (test/wow64/srvlat):
 * a bare request, a message sent to another thread's window, a hidden
 * window created and destroyed, a shown one created and destroyed (what an
 * installer's progress dialog does for every block it reads) and a shown
 * window's text set. Each in microseconds per operation, best of ROUNDS.
 * Prints "srvlat req R send S hidden H shown W text T" (µs per op).
 * usage: srvlat.exe [ROUNDS] [SCALE] */
#include <windows.h>
#include <winternl.h>
#include <stdio.h>
#include <stdlib.h>

typedef NTSTATUS (WINAPI *qit_fn)(HANDLE, THREADINFOCLASS, void *, ULONG, ULONG *);

static HWND peer;

static LRESULT CALLBACK wndproc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp)
{
    if (msg == WM_USER) return wp + 1;
    return DefWindowProcA(hwnd, msg, wp, lp);
}

static DWORD WINAPI peer_thread(void *ev)
{
    MSG m;
    peer = CreateWindowExA(0, "srvlat", "peer", WS_OVERLAPPED, 0, 0, 100, 100, NULL, NULL, NULL, NULL);
    SetEvent(ev);
    while (GetMessageA(&m, NULL, 0, 0) > 0) DispatchMessageA(&m);
    return 0;
}

static double now_us(void)
{
    LARGE_INTEGER c, f;
    QueryPerformanceCounter(&c); QueryPerformanceFrequency(&f);
    return (double)c.QuadPart * 1e6 / (double)f.QuadPart;
}

static void pump(void)
{
    MSG m;
    while (PeekMessageA(&m, NULL, 0, 0, PM_REMOVE)) DispatchMessageA(&m);
}

int main(int argc, char **argv)
{
    int rounds = argc > 1 ? atoi(argv[1]) : 3, scale = argc > 2 ? atoi(argv[2]) : 1, r, i;
    qit_fn qit = (qit_fn)GetProcAddress(GetModuleHandleA("ntdll.dll"), "NtQueryInformationThread");
    double best[5] = { 1e30, 1e30, 1e30, 1e30, 1e30 };
    int n[5] = { 2000 * scale, 500 * scale, 100 * scale, 50 * scale, 500 * scale };
    WNDCLASSA wc = { 0 };
    HANDLE ev = CreateEventA(NULL, TRUE, FALSE, NULL);
    HWND label;
    char tbi[64], text[32];

    wc.lpfnWndProc = wndproc;
    wc.lpszClassName = "srvlat";
    wc.hbrBackground = (HBRUSH)(COLOR_BTNFACE + 1);
    if (!RegisterClassA(&wc)) { printf("srvlat: RegisterClass failed\n"); return 1; }
    CloseHandle(CreateThread(NULL, 0, peer_thread, ev, 0, NULL));
    WaitForSingleObject(ev, INFINITE);
    label = CreateWindowExA(0, "srvlat", "label", WS_OVERLAPPEDWINDOW | WS_VISIBLE, 50, 50, 300, 120, NULL, NULL, NULL, NULL);
    if (!peer || !label) { printf("srvlat: windows failed\n"); return 1; }
    pump();

    for (r = 0; r < rounds; r++)
    {
        double t;

        t = now_us();
        for (i = 0; i < n[0]; i++) qit(GetCurrentThread(), 0, tbi, 28, NULL);
        t = (now_us() - t) / n[0]; if (t < best[0]) best[0] = t;

        t = now_us();
        for (i = 0; i < n[1]; i++)
            if (SendMessageA(peer, WM_USER, i, 0) != i + 1) { printf("srvlat: SendMessage %d wrong\n", i); return 1; }
        t = (now_us() - t) / n[1]; if (t < best[1]) best[1] = t;

        t = now_us();
        for (i = 0; i < n[2]; i++)
        {
            HWND h = CreateWindowExA(0, "srvlat", "hidden", WS_POPUP, 0, 0, 200, 100, NULL, NULL, NULL, NULL);
            if (!h) { printf("srvlat: hidden window %d failed\n", i); return 1; }
            DestroyWindow(h);
        }
        t = (now_us() - t) / n[2]; if (t < best[2]) best[2] = t;

        t = now_us();
        for (i = 0; i < n[3]; i++)
        {
            HWND h = CreateWindowExA(WS_EX_DLGMODALFRAME, "srvlat", "shown", WS_POPUP | WS_CAPTION | WS_VISIBLE, 100, 100, 260, 80, NULL, NULL, NULL, NULL);
            if (!h) { printf("srvlat: shown window %d failed\n", i); return 1; }
            pump();
            DestroyWindow(h);
            pump();
        }
        t = (now_us() - t) / n[3]; if (t < best[3]) best[3] = t;

        t = now_us();
        for (i = 0; i < n[4]; i++)
        {
            sprintf(text, "block %d", i);
            SetWindowTextA(label, text);
        }
        pump();
        t = (now_us() - t) / n[4]; if (t < best[4]) best[4] = t;
    }
    PostMessageA(peer, WM_QUIT, 0, 0);
    printf("srvlat req %.1f send %.1f hidden %.1f shown %.1f text %.1f\n", best[0], best[1], best[2], best[3], best[4]);
    fflush(stdout);
    return 0;
}
