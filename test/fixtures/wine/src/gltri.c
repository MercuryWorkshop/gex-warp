/* A GL window that never sets its viewport: wglMakeCurrent, then a grey
 * clear and one yellow triangle in immediate mode each frame, and
 * SwapBuffers in a loop for the argument's number of seconds (default 30).
 * GL gives a context the size of the window it is first made current on as
 * its viewport, so the triangle stands in the middle of the 320x240 client
 * area, about 9600 pixels of it; a viewport left at the size the host's
 * drawable had before the window's own drew it as a speck in a corner, or
 * not at all. Prints "shown" after the first frames and "frames N" and "bye"
 * at the end.
 * gex-nt-cc: clang --target=ARCH-w64-mingw32 -fuse-ld=lld -lgdi32 -lopengl32 */
#include <windows.h>
#include <GL/gl.h>
#include <stdio.h>
#include <stdlib.h>

static LRESULT CALLBACK proc(HWND w, UINT m, WPARAM wp, LPARAM lp) {
    if (m == WM_DESTROY) { PostQuitMessage(0); return 0; }
    return DefWindowProcW(w, m, wp, lp);
}

int main(int argc, char **argv) {
    int secs = argc > 1 ? atoi(argv[1]) : 30;
    WNDCLASSW wc = {0};
    wc.style = CS_OWNDC;
    wc.lpfnWndProc = proc;
    wc.hInstance = GetModuleHandleW(0);
    wc.lpszClassName = L"gex_gltri";
    RegisterClassW(&wc);
    RECT r = {0, 0, 320, 240};
    AdjustWindowRect(&r, WS_OVERLAPPEDWINDOW, FALSE);
    HWND w = CreateWindowExW(0, L"gex_gltri", L"gex gl triangle", WS_OVERLAPPEDWINDOW | WS_VISIBLE, 100, 100,
                             r.right - r.left, r.bottom - r.top, 0, 0, wc.hInstance, 0);
    HDC dc = GetDC(w);
    PIXELFORMATDESCRIPTOR pfd = {sizeof pfd, 1, PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER,
                                 PFD_TYPE_RGBA, 32};
    SetPixelFormat(dc, ChoosePixelFormat(dc, &pfd), &pfd);
    HGLRC gl = wglCreateContext(dc);
    wglMakeCurrent(dc, gl);
    DWORD end = GetTickCount() + secs * 1000, last = end + 60000;
    int frames = 0, done = 0;
    while (!done) {
        MSG msg;
        while (PeekMessageW(&msg, 0, 0, 0, PM_REMOVE)) {
            if (msg.message == WM_QUIT) done = 1;
            DispatchMessageW(&msg);
        }
        if (done) break;
        glClearColor(51 / 255.f, 51 / 255.f, 51 / 255.f, 1);
        glClear(GL_COLOR_BUFFER_BIT);
        glBegin(GL_TRIANGLES);
        glColor3ub(230, 210, 40);
        glVertex2f(-0.5f, -0.5f);
        glVertex2f(0.5f, -0.5f);
        glVertex2f(0, 0.5f);
        glEnd();
        SwapBuffers(dc);
        if (++frames == 3) { printf("shown\n"); fflush(stdout); }
        /* the seconds end it no sooner than its first frames */
        DWORD now = GetTickCount();
        if ((frames >= 3 && now >= end) || now >= last) DestroyWindow(w);
        Sleep(16);
    }
    printf("frames %d\nbye\n", frames);
    wglMakeCurrent(0, 0);
    wglDeleteContext(gl);
    return 0;
}
