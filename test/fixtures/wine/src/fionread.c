/* A response read the way Qt reads a socket (test/wine/fionread on the page,
 * test/wow64/fionread natively): on each FD_READ (WSAEventSelect here; Qt's
 * message window hears the same events), ask FIONREAD how much is there and
 * read that much. fionread HOST PORT N: N requests, each answered in three
 * parts 30 ms apart, read so ("fionread"), then N more read 64 KiB at a time
 * ("fixed"). Prints each one's ms, then
 *   fionread worst MS fixed worst MS largest-fionread BYTES
 * A FIONREAD that says 1 while more is waiting costs a wakeup a byte.
 * gex-nt-arch: x64
 * gex-nt-libs: -lws2_32 */
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static LARGE_INTEGER freq;
static SOCKET sock;
static char buf[65536];
static int have, want, done, sized;
static u_long largest;

static double now_ms( void )
{
    LARGE_INTEGER c;
    QueryPerformanceCounter( &c );
    return c.QuadPart * 1000.0 / freq.QuadPart;
}

static void events( WSAEVENT ev )
{
    WSANETWORKEVENTS ne;
    if (WSAEnumNetworkEvents( sock, ev, &ne )) return;
    if (ne.lNetworkEvents & FD_READ)
    {
        u_long n = sizeof(buf) - 1 - have;
        int r;
        if (sized)
        {
            u_long av = 0;
            ioctlsocket( sock, FIONREAD, &av );
            if (av > largest) largest = av;
            if (!av) return;
            if (av < n) n = av;
        }
        r = recv( sock, buf + have, n, 0 );
        if (r > 0)
        {
            have += r;
            buf[have] = 0;
            if (want < 0)
            {
                char *end = strstr( buf, "\r\n\r\n" ), *cl = strstr( buf, "Content-Length: " );
                if (end) want = (end - buf + 4) + (cl ? atoi( cl + 16 ) : 0);
            }
            if (want >= 0 && have >= want) done = 1;
        }
    }
    if ((ne.lNetworkEvents & FD_CLOSE) && !done) done = want >= 0 && have >= want ? 1 : -1;
}

int main( int argc, char **argv )
{
    WSADATA wd;
    struct addrinfo hints = {0}, *ai;
    double worst[2] = {0, 0};
    int n, i, pass;

    setvbuf( stdout, NULL, _IONBF, 0 );
    if (argc < 4) { printf( "fionread HOST PORT N\n" ); return 2; }
    n = atoi( argv[3] );
    QueryPerformanceFrequency( &freq );
    WSAStartup( MAKEWORD(2, 2), &wd );
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    if (getaddrinfo( argv[1], argv[2], &hints, &ai )) { printf( "fionread: no address for %s\n", argv[1] ); return 1; }
    for (pass = 0; pass < 2; pass++)
    {
        sized = !pass;
        for (i = 0; i < n; i++)
        {
            double t0 = now_ms(), ms;
            WSAEVENT ev = WSACreateEvent();
            sock = socket( AF_INET, SOCK_STREAM, 0 );
            have = 0; want = -1; done = 0;
            /* connected first, blocking: the reads are what is measured */
            if (connect( sock, ai->ai_addr, ai->ai_addrlen )) { printf( "fionread: connect %d\n", WSAGetLastError() ); return 1; }
            WSAEventSelect( sock, ev, FD_READ | FD_CLOSE );
            send( sock, "GET / HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n", 45, 0 );
            while (!done && now_ms() - t0 < 60000)
                if (WSAWaitForMultipleEvents( 1, &ev, FALSE, 1000, FALSE ) == WSA_WAIT_EVENT_0) events( ev );
            ms = now_ms() - t0;
            closesocket( sock );
            WSACloseEvent( ev );
            printf( "%s %d %s %.1f ms (%d bytes)\n", sized ? "fionread" : "fixed", i, done == 1 ? "ok" : "FAIL", ms, have );
            if (done != 1) return 1;
            if (ms > worst[pass]) worst[pass] = ms;
        }
    }
    printf( "fionread worst %.1f fixed worst %.1f largest-fionread %lu\n", worst[0], worst[1], largest );
    return 0;
}
