// An XPC client whose service ends with its request unanswered, as fontd's
// FontWorker does when it has been idle ten seconds (and under gex a
// request can take longer than that to reach it). Each request is sent
// with xpc_connection_send_message_with_reply_sync; a Mac answers each
// with XPC_ERROR_CONNECTION_INTERRUPTED, and the connection's handler is
// told the same, once the service has ended. A second request starts the
// service again (its check-in waits on the service's port). Ends itself,
// saying so, if a reply has not come in 60 s.
//
// `xpcdie N`: N requests, one after the other.
#include <xpc/xpc.h>
#include <dispatch/dispatch.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

static const char *what(xpc_object_t r) {
    if (r == XPC_ERROR_CONNECTION_INTERRUPTED) return "interrupted";
    if (r == XPC_ERROR_CONNECTION_INVALID) return "invalid";
    if (xpc_get_type(r) == XPC_TYPE_DICTIONARY) return "a reply";
    return "something else";
}

int main(int argc, char **argv) {
    int rounds = argc > 1 ? atoi(argv[1]) : 1;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 60 * NSEC_PER_SEC), dispatch_get_global_queue(0, 0), ^{
        printf("xpcdie: no reply in 60 s\n");
        fflush(stdout);
        _exit(3);
    });
    xpc_connection_t c = xpc_connection_create_mach_service("com.gex.xpcdie", NULL, 0);
    xpc_connection_set_event_handler(c, ^(xpc_object_t e) {
        printf("xpcdie: told %s\n", what(e));
        fflush(stdout);
    });
    xpc_connection_resume(c);
    for (int i = 1; i <= rounds; i++) {
        xpc_object_t m = xpc_dictionary_create(NULL, NULL, 0);
        xpc_dictionary_set_int64(m, "n", i);
        xpc_object_t r = xpc_connection_send_message_with_reply_sync(c, m);
        printf("xpcdie: reply %d: %s\n", i, what(r));
        fflush(stdout);
        xpc_release(m);
    }
    return 0;
}
