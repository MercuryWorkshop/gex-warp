// ioctl: the descriptor requests every file has, the terminal ones a file
// that is no terminal refuses, and a pseudo-terminal's -- its attributes in
// BSD's struct termios, its window, and a line through it either way.
#include "t.h"

enum {
    TIOCGETA = 0x40487413, TIOCSETA = 0x80487414, TIOCGWINSZ = 0x40087468, TIOCSWINSZ = 0x80087467,
    TIOCPTYGRANT = 0x20007454, TIOCPTYGNAME = 0x40807453, TIOCPTYUNLK = 0x20007452,
    FIONREAD = 0x4004667f, FIONBIO = 0x8004667e, FIOCLEX = 0x20006601, FIONCLEX = 0x20006602,
};
enum { O_NOCTTY = 0x20000 };
// c_iflag, c_oflag, c_cflag, c_lflag and c_cc's indices.
enum { ICRNL = 0x100, IXON = 0x200, OPOST = 1, ONLCR = 2, CS8 = 0x300, CREAD = 0x800, HUPCL = 0x4000 };
enum { ECHOE = 2, ECHO = 8, ISIG = 0x80, ICANON = 0x100, IEXTEN = 0x400 };
enum { VEOF = 0, VERASE = 3, VKILL = 5, VINTR = 8, VQUIT = 9, VSUSP = 10, VMIN = 16, VTIME = 17 };

struct t_termios {
    u64 c_iflag, c_oflag, c_cflag, c_lflag;
    u8 c_cc[20];
    u64 c_ispeed, c_ospeed;
};
struct t_winsize {
    u16 ws_row, ws_col, ws_xpixel, ws_ypixel;
};

static void show_termios(const char *what, const struct t_termios *t) {
    t_printf("%s: iflag %lx oflag %lx cflag %lx lflag %lx, speeds %lu %lu\n", what, t->c_iflag, t->c_oflag, t->c_cflag, t->c_lflag,
             t->c_ispeed, t->c_ospeed);
    t_printf("  intr %x quit %x erase %x kill %x eof %x susp %x min %x time %x\n", t->c_cc[VINTR], t->c_cc[VQUIT], t->c_cc[VERASE],
             t->c_cc[VKILL], t->c_cc[VEOF], t->c_cc[VSUSP], t->c_cc[VMIN], t->c_cc[VTIME]);
}

// Read what is there within a second, as text with its control characters
// spelled out.
static void show_read(const char *what, long fd) {
    struct {
        int fd;
        short events, revents;
    } pfd = {(int)fd, 1, 0};
    char buf[64];
    long n = SYS(SYS_poll, &pfd, 1, 1000) == 1 ? SYS(SYS_read, fd, buf, sizeof buf) : -999;
    t_printf("%s: %ld \"", what, n);
    for (long i = 0; i < n; i++) {
        if (buf[i] == '\r') t_printf("\\r");
        else if (buf[i] == '\n') t_printf("\\n");
        else t_printf("%c", buf[i]);
    }
    t_printf("\"\n");
}

int main(int argc, char **argv, char **envp, char **apple) {
    (void)argc, (void)argv, (void)envp, (void)apple;
    static struct t_termios t;
    struct t_winsize ws = {0, 0, 0, 0};
    long wr = -1;
    long rd = t_syscall2(&wr, SYS_pipe, 0, 0, 0);
    t_show("TIOCGETA on a pipe", SYS(SYS_ioctl, rd, TIOCGETA, &t));
    t_show("TIOCGWINSZ on a pipe", SYS(SYS_ioctl, rd, TIOCGWINSZ, &ws));
    SYS(SYS_write, wr, "12345", 5);
    int n = -1;
    t_show("FIONREAD", SYS(SYS_ioctl, rd, FIONREAD, &n));
    t_printf("  %d waiting\n", n);
    n = 1;
    t_show("FIONBIO on", SYS(SYS_ioctl, rd, FIONBIO, &n));
    t_show("  F_GETFL", SYS(SYS_fcntl, rd, F_GETFL, 0) & O_NONBLOCK);
    n = 0;
    t_show("FIONBIO off", SYS(SYS_ioctl, rd, FIONBIO, &n));
    t_show("  F_GETFL", SYS(SYS_fcntl, rd, F_GETFL, 0) & O_NONBLOCK);
    t_show("FIOCLEX", SYS(SYS_ioctl, rd, FIOCLEX, 0));
    t_show("  F_GETFD", SYS(SYS_fcntl, rd, F_GETFD, 0));
    t_show("FIONCLEX", SYS(SYS_ioctl, rd, FIONCLEX, 0));
    t_show("  F_GETFD", SYS(SYS_fcntl, rd, F_GETFD, 0));
    t_show("a request nothing has", SYS(SYS_ioctl, rd, 0x20007aff, 0));
    t_show("ioctl of no descriptor", SYS(SYS_ioctl, 99, FIONREAD, &n));
    long null = SYS(SYS_open, "/dev/null", O_RDWR);
    t_show("TIOCGETA on /dev/null", SYS(SYS_ioctl, null, TIOCGETA, &t));

    // A pseudo-terminal: the master, then grantpt, unlockpt and ptsname as
    // the libc asks them of it.
    long master = SYS(SYS_open, "/dev/ptmx", O_RDWR | O_NOCTTY);
    t_check("open /dev/ptmx", master >= 0);
    t_show("TIOCPTYGRANT", SYS(SYS_ioctl, master, TIOCPTYGRANT, 0));
    t_show("TIOCPTYUNLK", SYS(SYS_ioctl, master, TIOCPTYUNLK, 0));
    static char name[128];
    t_show("TIOCPTYGNAME", SYS(SYS_ioctl, master, TIOCPTYGNAME, name));
    t_check("a device's name", !t_strncmp(name, "/dev/", 5) && t_strlen(name) > 6);
    long slave = SYS(SYS_open, name, O_RDWR | O_NOCTTY);
    t_check("open it", slave >= 0);
    t_show("TIOCGETA", SYS(SYS_ioctl, slave, TIOCGETA, &t));
    t_check("it echoes, a line at a time", (t.c_lflag & (ECHO | ICANON | ISIG)) == (ECHO | ICANON | ISIG));
    t_check("and ends lines as a terminal does", (t.c_oflag & (OPOST | ONLCR)) == (OPOST | ONLCR) && t.c_iflag & ICRNL);
    t_check("eight bits", (t.c_cflag & CS8) == CS8);
    t_check("^C and ^D", t.c_cc[VINTR] == 3 && t.c_cc[VEOF] == 4);

    // Set every field, and read them back.
    t_memset(&t, 0, sizeof t);
    t.c_iflag = ICRNL | IXON;
    t.c_oflag = OPOST | ONLCR;
    t.c_cflag = CS8 | CREAD | HUPCL;
    t.c_lflag = ISIG | ICANON | ECHO | ECHOE | IEXTEN;
    t_memset(t.c_cc, 0xff, sizeof t.c_cc);
    t.c_cc[VINTR] = 3, t.c_cc[VQUIT] = 0x1c, t.c_cc[VERASE] = 0x7f, t.c_cc[VKILL] = 0x15, t.c_cc[VEOF] = 4;
    t.c_cc[VMIN] = 1, t.c_cc[VTIME] = 0;
    t.c_ispeed = t.c_ospeed = 38400;
    t_show("TIOCSETA", SYS(SYS_ioctl, slave, TIOCSETA, &t));
    t_check("the caller's structure is as it was", t.c_iflag == (ICRNL | IXON) && t.c_ospeed == 38400 && t.c_cc[VSUSP] == 0xff);
    static struct t_termios got;
    t_show("TIOCGETA", SYS(SYS_ioctl, slave, TIOCGETA, &got));
    show_termios("as set", &got);

    ws.ws_row = 24, ws.ws_col = 80;
    t_show("TIOCSWINSZ", SYS(SYS_ioctl, slave, TIOCSWINSZ, &ws));
    t_memset(&ws, 0, sizeof ws);
    t_show("TIOCGWINSZ", SYS(SYS_ioctl, master, TIOCGWINSZ, &ws));
    t_printf("  %d by %d\n", ws.ws_col, ws.ws_row);

    // Out of the program: a newline leaves as CR LF. Into it: a line, which
    // is echoed back to the keyboard's side.
    SYS(SYS_write, slave, "out\n", 4);
    show_read("the screen sees", master);
    SYS(SYS_write, master, "in\r", 3);
    show_read("the program reads", slave);
    show_read("and the screen its echo", master);

    // Raw: no echo, no lines, a byte at a time.
    t.c_iflag = 0, t.c_oflag = 0, t.c_lflag = 0;
    t_show("TIOCSETA raw", SYS(SYS_ioctl, slave, TIOCSETA, &t));
    t_show("TIOCGETA", SYS(SYS_ioctl, slave, TIOCGETA, &got));
    show_termios("raw", &got);
    SYS(SYS_write, master, "k", 1);
    show_read("a key, unechoed", slave);
    SYS(SYS_write, slave, "a\nb", 3);
    show_read("output, untouched", master);
    return 0;
}
