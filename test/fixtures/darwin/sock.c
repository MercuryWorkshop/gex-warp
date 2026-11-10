// Sockets: BSD's sockaddr (which begins with its own length), its msghdr
// and its 12-byte cmsghdr, a descriptor passed over a socketpair, a named
// unix socket across a fork, datagrams, the options and flags that are
// numbered differently from Linux's, and TCP over the loopback.
#include "t.h"

enum {
    SYS_recvmsg = 27, SYS_sendmsg = 28, SYS_recvfrom = 29, SYS_accept = 30, SYS_getpeername = 31,
    SYS_getsockname = 32, SYS_socket = 97, SYS_connect = 98, SYS_bind = 104, SYS_setsockopt = 105,
    SYS_listen = 106, SYS_getsockopt = 118, SYS_sendto = 133, SYS_shutdown = 134, SYS_socketpair = 135,
};
enum { AF_UNIX = 1, AF_INET = 2, SOCK_STREAM = 1, SOCK_DGRAM = 2 };
enum { SOL_SOCKET = 0xffff, SO_REUSEADDR = 4, SO_TYPE = 0x1008, SO_ERROR = 0x1007, SO_NOSIGPIPE = 0x1022, SCM_RIGHTS = 1 };
enum { MSG_PEEK = 2, MSG_TRUNC = 0x10, MSG_CTRUNC = 0x20, MSG_DONTWAIT = 0x80 };

struct t_sockaddr_un {
    u8 sun_len, sun_family;
    char sun_path[104];
};
struct t_sockaddr_in {
    u8 sin_len, sin_family;
    u16 sin_port;
    u8 sin_addr[4];
    char sin_zero[8];
};
struct t_iovec {
    void *iov_base;
    size_t iov_len;
};
struct t_msghdr {
    void *msg_name;
    u32 msg_namelen;
    struct t_iovec *msg_iov;
    int msg_iovlen;
    void *msg_control;
    u32 msg_controllen;
    int msg_flags;
};
struct t_cmsghdr {
    u32 cmsg_len;
    int cmsg_level, cmsg_type;
};

static long fork_(void) {
    long child_flag = -1;
    long pid = t_syscall2(&child_flag, SYS_fork, 0, 0, 0);
    if (pid < 0) return pid;
    return child_flag ? 0 : pid;
}

static void show_text(const char *what, long n, const char *buf) {
    t_printf("%s: %ld \"", what, n);
    for (long i = 0; i < n; i++) t_printf("%c", buf[i]);
    t_printf("\"\n");
}

static void pair(void) {
    int sv[2] = {-1, -1};
    t_show("socketpair", SYS(SYS_socketpair, AF_UNIX, SOCK_STREAM, 0, sv));
    t_check("two descriptors", sv[0] >= 0 && sv[1] >= 0 && sv[0] != sv[1]);
    char buf[64];
    t_show("write one end", SYS(SYS_write, sv[0], "hello", 5));
    t_show("peek the other", SYS(SYS_recvfrom, sv[1], buf, sizeof buf, MSG_PEEK, 0, 0));
    show_text("read it", SYS(SYS_read, sv[1], buf, sizeof buf), buf);
    t_show("receive with nothing there, not waiting", SYS(SYS_recvfrom, sv[1], buf, sizeof buf, MSG_DONTWAIT, 0, 0));

    int v = -1;
    u32 len = sizeof v;
    t_show("getsockopt SO_TYPE", SYS(SYS_getsockopt, sv[0], SOL_SOCKET, SO_TYPE, &v, &len));
    t_printf("  %d, %u bytes\n", v, len);
    v = -1, len = sizeof v;
    t_show("getsockopt SO_ERROR", SYS(SYS_getsockopt, sv[0], SOL_SOCKET, SO_ERROR, &v, &len));
    t_printf("  %d\n", v);
    v = 1;
    t_show("setsockopt SO_NOSIGPIPE", SYS(SYS_setsockopt, sv[0], SOL_SOCKET, SO_NOSIGPIPE, &v, sizeof v));
    t_show("setsockopt of no option", SYS(SYS_setsockopt, sv[0], SOL_SOCKET, 0x7777, &v, sizeof v));
    long wr = -1;
    long rd = t_syscall2(&wr, SYS_pipe, 0, 0, 0);
    len = sizeof v;
    t_show("getsockopt of a pipe", SYS(SYS_getsockopt, rd, SOL_SOCKET, SO_TYPE, &v, &len));

    static struct t_sockaddr_un sa;
    t_memset(&sa, 0x55, sizeof sa);
    len = sizeof sa;
    t_show("getsockname of an unnamed socket", SYS(SYS_getsockname, sv[0], &sa, &len));
    t_printf("  %u bytes: length %d, family %d\n", len, sa.sun_len, sa.sun_family);

    // A descriptor in a control message: the pipe's write end.
    struct {
        struct t_cmsghdr h;
        int fd;
    } ctl = {{sizeof ctl, SOL_SOCKET, SCM_RIGHTS}, (int)wr};
    struct t_iovec iov = {"fd", 2};
    struct t_msghdr msg;
    t_memset(&msg, 0, sizeof msg);
    msg.msg_iov = &iov, msg.msg_iovlen = 1, msg.msg_control = &ctl, msg.msg_controllen = sizeof ctl;
    t_show("sendmsg with a descriptor", SYS(SYS_sendmsg, sv[0], &msg, 0));
    struct {
        struct t_cmsghdr h;
        int fd[3];
    } got;
    t_memset(&got, 0, sizeof got);
    struct t_iovec riov = {buf, sizeof buf};
    t_memset(&msg, 0x55, sizeof msg);
    msg.msg_name = 0, msg.msg_namelen = 0;
    msg.msg_iov = &riov, msg.msg_iovlen = 1, msg.msg_control = &got, msg.msg_controllen = sizeof got;
    show_text("recvmsg", SYS(SYS_recvmsg, sv[1], &msg, 0), buf);
    t_printf("  control: %u bytes, a message of %u at level %x of type %d; flags %x\n", msg.msg_controllen, got.h.cmsg_len,
             got.h.cmsg_level, got.h.cmsg_type, msg.msg_flags);
    t_check("a new descriptor", got.fd[0] > 2 && got.fd[0] != wr);
    SYS(SYS_write, got.fd[0], "through", 7);
    show_text("what it writes, the pipe reads", SYS(SYS_read, rd, buf, sizeof buf), buf);

    t_show("shutdown writing", SYS(SYS_shutdown, sv[0], 1));
    t_show("the other end reads the end", SYS(SYS_read, sv[1], buf, sizeof buf));
    t_show("shutdown of a pipe", SYS(SYS_shutdown, rd, 1));
    SYS(SYS_close, sv[0]), SYS(SYS_close, sv[1]);
    t_show("socket of no family", SYS(SYS_socket, 99, SOCK_STREAM, 0));
}

static void datagrams(void) {
    int sv[2];
    t_show("socketpair of datagrams", SYS(SYS_socketpair, AF_UNIX, SOCK_DGRAM, 0, sv));
    SYS(SYS_sendto, sv[0], "first", 5, 0, 0, 0);
    SYS(SYS_sendto, sv[0], "second!", 7, 0, 0, 0);
    char buf[64];
    show_text("one", SYS(SYS_recvfrom, sv[1], buf, sizeof buf, 0, 0, 0), buf);
    // Into three bytes: the rest of the datagram is gone, and said to be.
    struct t_iovec iov = {buf, 3};
    struct t_msghdr msg;
    t_memset(&msg, 0, sizeof msg);
    msg.msg_iov = &iov, msg.msg_iovlen = 1;
    show_text("the other, into three bytes", SYS(SYS_recvmsg, sv[1], &msg, 0), buf);
    t_printf("  flags %x\n", msg.msg_flags);
    SYS(SYS_close, sv[0]), SYS(SYS_close, sv[1]);
}

static void named(void) {
    static struct t_sockaddr_un sa, peer;
    long ls = SYS(SYS_socket, AF_UNIX, SOCK_STREAM, 0);
    t_check("socket", ls >= 0);
    t_memset(&sa, 0, sizeof sa);
    sa.sun_family = AF_UNIX;
    // A name of this run's own, in /tmp.
    const char *stem = "/tmp/gex-t-sock.";
    size_t at = t_strlen(stem);
    t_memcpy(sa.sun_path, stem, at);
    for (long pid = SYS(SYS_getpid); pid; pid /= 10) sa.sun_path[at++] = (char)('0' + pid % 10);
    // The length the name has with its NUL, as SUN_LEN + 1.
    u32 salen = 2 + (u32)at + 1;
    sa.sun_len = (u8)salen;
    SYS(SYS_unlink, sa.sun_path);
    t_show("bind", SYS(SYS_bind, ls, &sa, salen));
    t_show("bind again", SYS(SYS_bind, ls, &sa, salen));
    t_show("listen", SYS(SYS_listen, ls, 4));
    struct t_stat st;
    t_show("stat the name", SYS(SYS_stat64, sa.sun_path, &st));
    t_printf("  type %o\n", st.st_mode & 0170000);
    u32 len = sizeof peer;
    t_memset(&peer, 0x55, sizeof peer);
    t_show("getsockname", SYS(SYS_getsockname, ls, &peer, &len));
    t_printf("  family %d, %s\n", peer.sun_family,
             len == salen && peer.sun_len == salen && !t_strcmp(peer.sun_path, sa.sun_path) ? "the name and length it was bound with" : "something else");

    long pid = fork_();
    if (pid == 0) {
        long c = SYS(SYS_socket, AF_UNIX, SOCK_STREAM, 0);
        long r = SYS(SYS_connect, c, &sa, salen);
        char buf[16];
        SYS(SYS_write, c, "ping", 4);
        long n = SYS(SYS_read, c, buf, sizeof buf);
        len = sizeof peer;
        t_memset(&peer, 0, sizeof peer);
        long pn = SYS(SYS_getpeername, c, &peer, &len);
        // One line, so the two processes' output cannot interleave.
        t_printf("child: connect %ld, read %ld \"%c%c%c%c\", getpeername %ld family %d, %s\n", r, n, buf[0], buf[1], buf[2], buf[3], pn,
                 peer.sun_family, !t_strcmp(peer.sun_path, sa.sun_path) ? "the server's name" : "something else");
        t_exit(0);
    }
    len = sizeof peer;
    t_memset(&peer, 0x55, sizeof peer);
    long c = SYS(SYS_accept, ls, &peer, &len);
    t_check("accept", c >= 0);
    t_printf("  from %u bytes: length %d, family %d\n", len, peer.sun_len, peer.sun_family);
    char buf[16];
    show_text("the server reads", SYS(SYS_read, c, buf, sizeof buf), buf);
    SYS(SYS_write, c, "pong", 4);
    int status = -1;
    SYS(SYS_wait4, pid, &status, 0, 0);
    t_printf("the client: status %x\n", status);
    SYS(SYS_close, c), SYS(SYS_close, ls);
    long s = SYS(SYS_socket, AF_UNIX, SOCK_STREAM, 0);
    t_show("connect to a name nobody listens on", SYS(SYS_connect, s, &sa, salen));
    SYS(SYS_unlink, sa.sun_path);
    t_show("and to no name at all", SYS(SYS_connect, s, &sa, salen));
}

static void tcp(int refusal) {
    static struct t_sockaddr_in sa, got;
    long ls = SYS(SYS_socket, AF_INET, SOCK_STREAM, 0);
    t_check("a TCP socket", ls >= 0);
    int one = 1;
    t_show("setsockopt SO_REUSEADDR", SYS(SYS_setsockopt, ls, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one));
    t_memset(&sa, 0, sizeof sa);
    sa.sin_len = sizeof sa, sa.sin_family = AF_INET;
    sa.sin_addr[0] = 127, sa.sin_addr[3] = 1;
    t_show("bind 127.0.0.1, any port", SYS(SYS_bind, ls, &sa, sizeof sa));
    t_show("listen", SYS(SYS_listen, ls, 4));
    u32 len = sizeof got;
    t_memset(&got, 0x55, sizeof got);
    t_show("getsockname", SYS(SYS_getsockname, ls, &got, &len));
    t_printf("  %u bytes: length %d, family %d, %d.%d.%d.%d, %s\n", len, got.sin_len, got.sin_family, got.sin_addr[0], got.sin_addr[1],
             got.sin_addr[2], got.sin_addr[3], got.sin_port ? "a port" : "no port");
    sa.sin_port = got.sin_port;
    long pid = fork_();
    if (pid == 0) {
        long c = SYS(SYS_socket, AF_INET, SOCK_STREAM, 0);
        long r = SYS(SYS_connect, c, &sa, sizeof sa);
        SYS(SYS_write, c, "over tcp", 8);
        char buf[16];
        long n = SYS(SYS_read, c, buf, sizeof buf);
        t_exit(r == 0 && n == 2 ? 0 : 1);
    }
    len = sizeof got;
    t_memset(&got, 0x55, sizeof got);
    long c = SYS(SYS_accept, ls, &got, &len);
    t_check("accept", c >= 0);
    t_printf("  from %u bytes: length %d, family %d, %d.%d.%d.%d\n", len, got.sin_len, got.sin_family, got.sin_addr[0], got.sin_addr[1],
             got.sin_addr[2], got.sin_addr[3]);
    char buf[16];
    show_text("the server reads", SYS(SYS_read, c, buf, sizeof buf), buf);
    len = sizeof got;
    t_memset(&got, 0x55, sizeof got);
    t_show("getpeername", SYS(SYS_getpeername, c, &got, &len));
    t_printf("  length %d, family %d, %d.%d.%d.%d\n", got.sin_len, got.sin_family, got.sin_addr[0], got.sin_addr[1], got.sin_addr[2],
             got.sin_addr[3]);
    SYS(SYS_write, c, "ok", 2);
    int status = -1;
    SYS(SYS_wait4, pid, &status, 0, 0);
    t_printf("the client: status %x\n", status);
    SYS(SYS_close, c), SYS(SYS_close, ls);
    // Nobody listens there now. (Asked for by an argument: where the
    // loopback is another machine's -- a page's, through its proxy -- the
    // answer is that machine's to give.)
    if (refusal) {
        long s = SYS(SYS_socket, AF_INET, SOCK_STREAM, 0);
        t_show("connect to a closed port", SYS(SYS_connect, s, &sa, sizeof sa));
    }
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)envp, (void)apple;
    pair();
    datagrams();
    named();
    tcp(argc > 1 && !t_strcmp(argv[1], "refusal"));
    return 0;
}
