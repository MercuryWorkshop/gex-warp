// The service of fixtures/darwin/mojave/xpcdie.c: it checks in for
// com.gex.xpcdie, takes its client's connection (libxpc's check-in message
// on the service's port, which carries the connection's receive right),
// and ends three seconds later without receiving from it, the client's
// request still queued there -- what fontworker's idle exit does to
// fontd's. `take`: it receives the request first, and ends without
// answering it.
#include <mach/mach.h>
#include <servers/bootstrap.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv) {
    mach_port_t sp = MACH_PORT_NULL;
    kern_return_t kr = bootstrap_check_in(bootstrap_port, "com.gex.xpcdie", &sp);
    fprintf(stderr, "xpcdie-service: check-in %d\n", kr);
    if (kr) return 1;
    union {
        mach_msg_header_t h;
        char b[4096];
    } m;
    memset(&m, 0, sizeof m);
    kr = mach_msg(&m.h, MACH_RCV_MSG, 0, sizeof m, sp, 0, 0);
    mach_port_t conn = MACH_PORT_NULL;
    if (!kr && (m.h.msgh_bits & MACH_MSGH_BITS_COMPLEX)) {
        mach_msg_body_t *body = (mach_msg_body_t *)(&m.h + 1);
        mach_msg_port_descriptor_t *d = (mach_msg_port_descriptor_t *)(body + 1);
        for (unsigned i = 0; i < body->msgh_descriptor_count; i++, d++)
            if (d->disposition == MACH_MSG_TYPE_MOVE_RECEIVE) conn = d->name;
    }
    fprintf(stderr, "xpcdie-service: a client: %s\n", conn ? "yes" : "no");
    if (argc > 1 && !strcmp(argv[1], "take") && conn) {
        memset(&m, 0, sizeof m);
        kr = mach_msg(&m.h, MACH_RCV_MSG, 0, sizeof m, conn, 0, 0);
        fprintf(stderr, "xpcdie-service: took its request: %x\n", kr);
    }
    sleep(3);
    fprintf(stderr, "xpcdie-service: ends\n");
    return 0;
}
