// A service made as XPC services are, for fixtures/darwin/mojave/xpcdie.c's
// `restart`: libxpc's listener for com.gex.xpcdie (which checks the service
// in and guards its port), answering each request with its pid.
#include <xpc/xpc.h>
#include <dispatch/dispatch.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    xpc_connection_t l = xpc_connection_create_mach_service("com.gex.xpcdie", NULL, XPC_CONNECTION_MACH_SERVICE_LISTENER);
    xpc_connection_set_event_handler(l, ^(xpc_object_t peer) {
        if (xpc_get_type(peer) != XPC_TYPE_CONNECTION) return;
        xpc_connection_set_event_handler(peer, ^(xpc_object_t m) {
            if (xpc_get_type(m) != XPC_TYPE_DICTIONARY) return;
            xpc_object_t r = xpc_dictionary_create_reply(m);
            xpc_dictionary_set_int64(r, "pid", getpid());
            xpc_connection_send_message(peer, r);
            xpc_release(r);
        });
        xpc_connection_resume(peer);
    });
    xpc_connection_resume(l);
    fprintf(stderr, "xpcdie-listener: up, pid %d\n", getpid());
    dispatch_main();
}
