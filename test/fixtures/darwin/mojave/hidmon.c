// hidmon: a client of the HID event system (Mojave's hidd), saying each
// event it is given -- what reaches WindowServer the same way. Its
// signature carries the entitlement the event system asks of a monitor.
// With an argument, the type of client it is (1, a monitor, by default).
#include <CoreFoundation/CoreFoundation.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct __IOHIDEventSystemClient *IOHIDEventSystemClientRef;
typedef struct __IOHIDEvent *IOHIDEventRef;
typedef struct __IOHIDService *IOHIDServiceRef;
typedef void (*IOHIDEventSystemClientEventCallback)(void *target, void *refcon, IOHIDServiceRef sender, IOHIDEventRef event);
IOHIDEventSystemClientRef IOHIDEventSystemClientCreateWithType(CFAllocatorRef, int, CFDictionaryRef);
void IOHIDEventSystemClientRegisterEventCallback(IOHIDEventSystemClientRef, IOHIDEventSystemClientEventCallback, void *, void *);
void IOHIDEventSystemClientScheduleWithRunLoop(IOHIDEventSystemClientRef, CFRunLoopRef, CFStringRef);
CFArrayRef IOHIDEventSystemClientCopyServices(IOHIDEventSystemClientRef);
uint32_t IOHIDEventGetType(IOHIDEventRef);
long IOHIDEventGetIntegerValue(IOHIDEventRef, uint32_t);
void IOHIDEventGetVendorDefinedData(IOHIDEventRef, uint8_t **, CFIndex *);

// An event: its type, and a vendor-defined one's usage page, usage and
// length, and (an NX event) the NX type and where the pointer is.
static void event(void *target, void *refcon, IOHIDServiceRef sender, IOHIDEventRef e) {
    (void)target, (void)refcon, (void)sender;
    uint32_t type = IOHIDEventGetType(e);
    printf("hidmon: event type %u", type);
    if (type == 1) {
        uint8_t *d = NULL;
        CFIndex n = 0;
        IOHIDEventGetVendorDefinedData(e, &d, &n);
        printf(" page %#lx usage %ld length %ld", IOHIDEventGetIntegerValue(e, 0x10000), IOHIDEventGetIntegerValue(e, 0x10001), (long)n);
        if (d && n >= 12) {
            int32_t w[3];
            memcpy(w, d, sizeof w);
            printf(" nx %d at %d,%d", w[0], w[1], w[2]);
        }
    }
    printf("\n");
    fflush(stdout);
}

int main(int argc, char **argv) {
    int type = argc > 1 ? atoi(argv[1]) : 1;
    IOHIDEventSystemClientRef c = IOHIDEventSystemClientCreateWithType(kCFAllocatorDefault, type, NULL);
    if (!c) {
        printf("hidmon: no client\n");
        return 1;
    }
    CFArrayRef services = IOHIDEventSystemClientCopyServices(c);
    printf("hidmon: %ld services\n", services ? (long)CFArrayGetCount(services) : -1L);
    IOHIDEventSystemClientRegisterEventCallback(c, event, NULL, NULL);
    IOHIDEventSystemClientScheduleWithRunLoop(c, CFRunLoopGetMain(), kCFRunLoopDefaultMode);
    printf("hidmon: up\n");
    fflush(stdout);
    CFRunLoopRun();
    return 0;
}
