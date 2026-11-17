// Kernel control sockets: socket(PF_SYSTEM, SOCK_DGRAM, SYSPROTO_CONTROL),
// a control's id by its name (CTLIOCGINFO), and a connection to it by id
// (struct sockaddr_ctl). The network statistics' control
// (com.apple.network.statistics) is there; a name nobody registered is not,
// nor an id; a kernel event socket (SYSPROTO_EVENT) is another matter.
#include "t.h"

enum { SYS_socket = 97, SYS_connect = 98, PF_SYSTEM = 32, SOCK_DGRAM = 2, SYSPROTO_CONTROL = 2 };
#define CTLIOCGINFO 0xc0644e03ul

struct ctl_info {
    u32 id;
    char name[96];
};

static long info(long fd, const char *name, u32 *id) {
    struct ctl_info ci = {0, {0}};
    for (int i = 0; name[i] && i < 95; i++) ci.name[i] = name[i];
    long r = SYS(SYS_ioctl, fd, CTLIOCGINFO, &ci);
    *id = ci.id;
    return r;
}

static long to(long fd, u32 id) {
    unsigned char sc[32] = {32, PF_SYSTEM, 2, 0};
    for (int i = 0; i < 4; i++) sc[4 + i] = (unsigned char)(id >> (8 * i));
    return SYS(SYS_connect, fd, sc, sizeof sc);
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    long fd = SYS(SYS_socket, PF_SYSTEM, SOCK_DGRAM, SYSPROTO_CONTROL);
    t_check("a control socket", fd >= 0);
    u32 id = 0, none = 0;
    t_show("the statistics' id", info(fd, "com.apple.network.statistics", &id));
    t_check("is one", id != 0);
    t_show("a name nobody has", info(fd, "com.apple.gex.nothing", &none));
    t_show("connect to no such id", to(fd, 0x7fffff00));
    t_show("connect to the statistics", to(fd, id));
    long plain = SYS(SYS_open, "/dev/null", O_RDONLY);
    t_show("CTLIOCGINFO of a file", info(plain, "com.apple.network.statistics", &none));
    return 0;
}
