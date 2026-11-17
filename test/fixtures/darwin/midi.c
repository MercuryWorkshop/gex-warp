// A Mac program in the way of a studio's: it asks CoreMIDI what devices there
// are, listens to one and sends to another, and hears when a device comes or
// goes. darwin/midi-page runs it in a page whose Web MIDI is a test's own
// (the real CoreMIDI of pyrus, over the page's `midi` service).
//
//   midi          the devices, the messages that arrive, what it sends, the
//                 notifications of a device that comes and one that goes
//   midi denied   a page whose browser gives no MIDI: no devices, and a
//                 virtual source and destination between this program's own
//                 ports all the same
#include <CoreFoundation/CoreFoundation.h>
#include <CoreMIDI/CoreMIDI.h>
#include <dispatch/dispatch.h>
#include <mach/mach_time.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static void cs(CFStringRef s, char *b, size_t n) {
    if (!s || !CFStringGetCString(s, b, (CFIndex)n, kCFStringEncodingUTF8)) strcpy(b, "-");
}
static void name_of(MIDIObjectRef o, CFStringRef key, char *b, size_t n) {
    CFStringRef s = NULL;
    if (MIDIObjectGetStringProperty(o, key, &s)) strcpy(b, "-");
    else {
        cs(s, b, n);
        CFRelease(s);
    }
}
static int int_of(MIDIObjectRef o, CFStringRef key) {
    SInt32 v = -1;
    MIDIObjectGetIntegerProperty(o, key, &v);
    return v;
}
static void spin(double seconds) { CFRunLoopRunInMode(kCFRunLoopDefaultMode, seconds, false); }

static void list_all(const char *what) {
    printf("%s: %lu devices\n", what, MIDIGetNumberOfDevices());
    for (ItemCount i = 0; i < MIDIGetNumberOfDevices(); i++) {
        MIDIDeviceRef d = MIDIGetDevice(i);
        char name[100], manu[100], model[100];
        name_of(d, kMIDIPropertyName, name, sizeof name);
        name_of(d, kMIDIPropertyManufacturer, manu, sizeof manu);
        name_of(d, kMIDIPropertyModel, model, sizeof model);
        printf(" device '%s' by '%s', model '%s', offline %d, %lu entities\n", name, manu, model, int_of(d, kMIDIPropertyOffline), MIDIDeviceGetNumberOfEntities(d));
        for (ItemCount j = 0; j < MIDIDeviceGetNumberOfEntities(d); j++) {
            MIDIEntityRef e = MIDIDeviceGetEntity(d, j);
            name_of(e, kMIDIPropertyName, name, sizeof name);
            printf("  entity '%s': %lu sources, %lu destinations\n", name, MIDIEntityGetNumberOfSources(e), MIDIEntityGetNumberOfDestinations(e));
            for (ItemCount k = 0; k < MIDIEntityGetNumberOfSources(e); k++) {
                MIDIEndpointRef s = MIDIEntityGetSource(e, k);
                name_of(s, kMIDIPropertyDisplayName, name, sizeof name);
                MIDIDeviceRef back = 0;
                MIDIEntityRef eback = 0;
                MIDIEndpointGetEntity(s, &eback);
                MIDIEntityGetDevice(eback, &back);
                printf("   source '%s' (back to its device %d, offline %d, protocol %d)\n", name, back == d, int_of(s, kMIDIPropertyOffline), int_of(s, kMIDIPropertyProtocolID));
            }
            for (ItemCount k = 0; k < MIDIEntityGetNumberOfDestinations(e); k++) {
                MIDIEndpointRef s = MIDIEntityGetDestination(e, k);
                name_of(s, kMIDIPropertyDisplayName, name, sizeof name);
                printf("   destination '%s'\n", name);
            }
        }
    }
    printf("%s: %lu sources, %lu destinations\n", what, MIDIGetNumberOfSources(), MIDIGetNumberOfDestinations());
}

static MIDIEndpointRef named(const char *want, int dest) {
    ItemCount n = dest ? MIDIGetNumberOfDestinations() : MIDIGetNumberOfSources();
    for (ItemCount i = 0; i < n; i++) {
        MIDIEndpointRef e = dest ? MIDIGetDestination(i) : MIDIGetSource(i);
        char b[100];
        name_of(e, kMIDIPropertyDisplayName, b, sizeof b);
        if (!strcmp(b, want)) return e;
    }
    return 0;
}

static dispatch_semaphore_t got, completed;
static volatile int nmessages;
static MIDISysexSendRequest req;
static Byte sysex[] = {0xf0, 0x7d, 0x01, 0x02, 0xf7};
static int done_complete;
static unsigned done_left;
// (Told on a thread of CoreMIDI's: the program says so when it has waited.)
static void sysex_done(MIDISysexSendRequest *r) {
    done_complete = r->complete;
    done_left = r->bytesToSend;
    dispatch_semaphore_signal(completed);
}
static void report_sysex(void) { printf("sysex sent: complete %d, %u left\n", done_complete, done_left); }

static int denied(void) {
    setvbuf(stdout, 0, _IONBF, 0);
    MIDIClientRef c;
    printf("client: %d\n", (int)MIDIClientCreate(CFSTR("denied"), 0, 0, &c));
    list_all("no browser MIDI");
    got = dispatch_semaphore_create(0);
    MIDIEndpointRef src, dst;
    MIDIPortRef in, out;
    MIDISourceCreate(c, CFSTR("Virtual Source"), &src);
    MIDIDestinationCreateWithBlock(c, CFSTR("Virtual Destination"), &dst, ^(const MIDIPacketList *l, void *r) {
        printf("destination got:");
        for (int i = 0; i < l->packet[0].length; i++) printf(" %02x", l->packet[0].data[i]);
        printf("\n");
        dispatch_semaphore_signal(got);
    });
    MIDIInputPortCreateWithBlock(c, CFSTR("in"), &in, ^(const MIDIPacketList *l, void *r) {
        printf("port got:");
        for (int i = 0; i < l->packet[0].length; i++) printf(" %02x", l->packet[0].data[i]);
        printf("\n");
        dispatch_semaphore_signal(got);
    });
    MIDIOutputPortCreate(c, CFSTR("out"), &out);
    printf("the virtual ones are listed: %d %d\n", named("Virtual Source", 0) == src, named("Virtual Destination", 1) == dst);
    MIDIPortConnectSource(in, src, NULL);
    Byte buf[64];
    MIDIPacketList *l = (MIDIPacketList *)buf;
    MIDIPacket *p = MIDIPacketListInit(l);
    Byte note[] = {0x90, 60, 100};
    MIDIPacketListAdd(l, sizeof buf, p, 0, 3, note);
    MIDISend(out, dst, l);
    dispatch_semaphore_wait(got, DISPATCH_TIME_FOREVER);
    MIDIReceived(src, l);
    dispatch_semaphore_wait(got, DISPATCH_TIME_FOREVER);
    printf("system exclusive between the program's own endpoints: ");
    {
        req.destination = dst;
        req.data = sysex;
        req.bytesToSend = sizeof sysex;
        req.complete = 0;
        req.completionProc = sysex_done;
        completed = dispatch_semaphore_create(0);
        MIDISendSysex(&req);
        dispatch_semaphore_wait(got, DISPATCH_TIME_FOREVER);
        dispatch_semaphore_wait(completed, DISPATCH_TIME_FOREVER);
        report_sysex();
    }
    return 0;
}

// A librarian: it only listens. Two ports are connected to the keyboard, and
// what they are given (system exclusive among it, when the browser's second
// permission is held) is printed once N messages are in, and a moment more.
static int cmp_lines(const void *a, const void *b) { return strcmp(*(char *const *)a, *(char *const *)b); }
static int listen_mode(int n) {
    setvbuf(stdout, 0, _IONBF, 0);
    MIDIClientRef c;
    MIDIClientCreate(CFSTR("librarian"), 0, 0, &c);
    MIDIEndpointRef keys = named("Fake Keys", 0);
    printf("found: %d\n", keys != 0);
    static char *lines[64];
    static int nlines;
    static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
    got = dispatch_semaphore_create(0);
    MIDIPortRef in[2];
    for (long k = 0; k < 2; k++) {
        MIDIInputPortCreateWithBlock(c, CFSTR("in"), &in[k], ^(const MIDIPacketList *l, void *r) {
            const MIDIPacket *p = &l->packet[0];
            for (unsigned i = 0; i < l->numPackets; i++, p = MIDIPacketNext(p)) {
                char b[200];
                int o = snprintf(b, sizeof b, "port %ld:", (long)r);
                for (int j = 0; j < p->length; j++) o += snprintf(b + o, sizeof b - (size_t)o, " %02x", p->data[j]);
                pthread_mutex_lock(&lock);
                if (nlines < 64) lines[nlines++] = strdup(b);
                pthread_mutex_unlock(&lock);
                dispatch_semaphore_signal(got);
            }
        });
        printf("connect %ld: %d\n", k + 1, (int)MIDIPortConnectSource(in[k], keys, (void *)(k + 1)));
    }
    for (int i = 0; i < n; i++)
        if (dispatch_semaphore_wait(got, dispatch_time(DISPATCH_TIME_NOW, 30 * NSEC_PER_SEC))) {
            printf("only %d came\n", i);
            break;
        }
    usleep(500000);
    pthread_mutex_lock(&lock);
    qsort(lines, (size_t)nlines, sizeof *lines, cmp_lines);
    for (int i = 0; i < nlines; i++) printf("%s\n", lines[i]);
    pthread_mutex_unlock(&lock);
    return 0;
}

int main(int argc, char **argv) {
    if (argc > 1 && !strcmp(argv[1], "denied")) return denied();
    if (argc > 2 && !strcmp(argv[1], "listen")) return listen_mode(atoi(argv[2]));
    setvbuf(stdout, 0, _IONBF, 0);
    __block int arrived = 0, left = 0;
    MIDIClientRef c;
    printf("client: %d\n", (int)MIDIClientCreateWithBlock(CFSTR("studio"), &c, ^(const MIDINotification *m) {
        if (m->messageID == kMIDIMsgObjectAdded) {
            const MIDIObjectAddRemoveNotification *n = (const void *)m;
            char b[100];
            name_of(n->child, kMIDIPropertyName, b, sizeof b);
            if (n->childType == kMIDIObjectType_Device) {
                printf("notification: a device came: '%s'\n", b);
                arrived = 1;
            } else {
                printf("notification: an endpoint came, type %d, on the run loop's thread %d\n", (int)n->childType, pthread_main_np());
            }
        } else if (m->messageID == kMIDIMsgPropertyChanged) {
            const MIDIObjectPropertyChangeNotification *n = (const void *)m;
            char b[100], k[100];
            name_of(n->object, kMIDIPropertyName, b, sizeof b);
            cs(n->propertyName, k, sizeof k);
            if (!strcmp(k, "offline") && n->objectType == kMIDIObjectType_Device) {
                printf("notification: '%s' is now offline %d\n", b, int_of(n->object, kMIDIPropertyOffline));
                if (int_of(n->object, kMIDIPropertyOffline)) left = 1;
            }
        } else if (m->messageID == kMIDIMsgSetupChanged) {
            printf("notification: the setup changed\n");
        }
    }));
    list_all("at the start");
    MIDIEndpointRef keys = named("Fake Keys", 0), synth = named("Fake Synth", 1);
    printf("found: %d %d\n", keys != 0, synth != 0);
    char ident[100];
    name_of(keys, kMIDIPropertyDisplayName, ident, sizeof ident);
    printf("the source's display name: '%s', manufacturer '", ident);
    name_of(keys, kMIDIPropertyManufacturer, ident, sizeof ident);
    printf("%s'\n", ident);
    // Listen.
    got = dispatch_semaphore_create(0);
    __block UInt64 t0 = mach_absolute_time();
    MIDIPortRef in, out;
    MIDIInputPortCreateWithBlock(c, CFSTR("in"), &in, ^(const MIDIPacketList *l, void *r) {
        UInt64 now = mach_absolute_time();
        const MIDIPacket *p = &l->packet[0];
        for (unsigned i = 0; i < l->numPackets; i++, p = MIDIPacketNext(p)) {
            printf("message:");
            for (int j = 0; j < p->length; j++) printf(" %02x", p->data[j]);
            // (Its time: after this program began, and not after now.)
            printf(", time in range %d, connection %ld\n", p->timeStamp >= t0 && p->timeStamp <= now, (long)r);
            nmessages++;
        }
        dispatch_semaphore_signal(got);
    });
    MIDIOutputPortCreate(c, CFSTR("out"), &out);
    printf("connect: %d\n", (int)MIDIPortConnectSource(in, keys, (void *)7));
    for (int i = 0; i < 3; i++)
        if (dispatch_semaphore_wait(got, dispatch_time(DISPATCH_TIME_NOW, 30 * NSEC_PER_SEC))) {
            printf("nothing came\n");
            return 1;
        }
    // Send: now, at a time, and system exclusive.
    {
        Byte buf[64];
        MIDIPacketList *l = (MIDIPacketList *)buf;
        MIDIPacket *p = MIDIPacketListInit(l);
        Byte on[] = {0x90, 60, 100};
        p = MIDIPacketListAdd(l, sizeof buf, p, 0, 3, on);
        printf("send: %d\n", (int)MIDISend(out, synth, l));
        mach_timebase_info_data_t tb;
        mach_timebase_info(&tb);
        p = MIDIPacketListInit(l);
        Byte off[] = {0x80, 60, 0};
        MIDIPacketListAdd(l, sizeof buf, p, mach_absolute_time() + 150000000ull * tb.denom / tb.numer, 3, off);
        printf("send later: %d\n", (int)MIDISend(out, synth, l));
        completed = dispatch_semaphore_create(0);
        req.destination = synth;
        req.data = sysex;
        req.bytesToSend = sizeof sysex;
        req.complete = 0;
        req.completionProc = sysex_done;
        printf("send system exclusive: %d\n", (int)MIDISendSysex(&req));
        dispatch_semaphore_wait(completed, dispatch_time(DISPATCH_TIME_NOW, 30 * NSEC_PER_SEC));
        report_sysex();
    }
    // Hear what comes and goes.
    printf("waiting for a device to come and one to go\n");
    for (int i = 0; i < 300 && !(arrived && left); i++) spin(0.1);
    printf("a device came: %d, one went: %d\n", arrived, left);
    list_all("at the end");
    return 0;
}
