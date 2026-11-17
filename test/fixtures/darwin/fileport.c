// File ports: fileport_makeport makes a send right that stands for an open
// file, and fileport_makefd a descriptor for that file again, closed on
// exec, sharing the file's offset with the first (how libxpc sends a
// descriptor in a message). What is refused: a descriptor that is not
// open, a name that is no file port, a port whose right is gone.
#include "t.h"

enum { SYS_fileport_makeport = 430, SYS_fileport_makefd = 431 };
enum { TRAP_port_deallocate = 18, TRAP_port_type = 76 };

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    long task = TRAP(TRAP_task_self);
    long fd = SYS(SYS_open, "f", O_RDWR | O_CREAT | O_TRUNC, 0644);
    SYS(SYS_write, fd, "abcdef", 6);
    SYS(SYS_lseek, fd, 2, 0);
    unsigned port = 0;
    t_show("makeport", SYS(SYS_fileport_makeport, fd, &port));
    t_check("a name", port != 0);
    unsigned type = 0;
    t_show("its type", TRAP(TRAP_port_type, task, port, &type));
    t_printf("a send right: %d\n", (type & 0x10000) != 0);
    long nfd = SYS(SYS_fileport_makefd, port);
    t_check("a new descriptor", nfd >= 0 && nfd != fd);
    t_show("closed on exec", SYS(SYS_fcntl, nfd, F_GETFD));
    char b[8] = {0};
    t_show("read from it", SYS(SYS_read, nfd, b, 2));
    t_printf("read %s, the first's offset now %ld\n", b, SYS(SYS_lseek, fd, 0, 1));
    SYS(SYS_close, fd);
    t_show("the file outlives the first descriptor", SYS(SYS_read, nfd, b, 2));
    t_printf("read %s\n", b);
    long again = SYS(SYS_fileport_makefd, port);
    t_check("and the port, another descriptor", again >= 0);
    unsigned none = 0;
    t_show("makeport of a descriptor not open", SYS(SYS_fileport_makeport, 900, &none));
    t_show("makefd of a name that is no file port", SYS(SYS_fileport_makefd, 0x12345));
    t_show("deallocate the port", TRAP(TRAP_port_deallocate, task, port));
    t_show("makefd after", SYS(SYS_fileport_makefd, port));
    return 0;
}
