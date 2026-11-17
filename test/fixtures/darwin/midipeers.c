// link: -framework CoreMIDI -framework CoreFoundation
// cwd: yes
// Virtual endpoints between processes, as on a Mac with no MIDI hardware:
// a process lists the endpoints another made, connects a port to its source,
// sends to its destination, is told when it ends. This program starts a
// copy of itself; each makes a source and a destination, and what goes
// between them is printed by the one that gets it. The same on this Mac and
// under gex-warp.
#include <CoreFoundation/CoreFoundation.h>
#include <CoreMIDI/CoreMIDI.h>
#include <dispatch/dispatch.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;

static MIDIPortRef out;
static dispatch_semaphore_t sem;

static MIDIEndpointRef find(const char *name, int dest, int seconds) {
    for (int i = 0; i < seconds * 100; i++) {
        ItemCount n = dest ? MIDIGetNumberOfDestinations() : MIDIGetNumberOfSources();
        for (ItemCount k = 0; k < n; k++) {
            MIDIEndpointRef e = dest ? MIDIGetDestination(k) : MIDIGetSource(k);
            CFStringRef s = NULL;
            char b[100] = "";
            if (!MIDIObjectGetStringProperty(e, kMIDIPropertyName, &s) && s) {
                CFStringGetCString(s, b, sizeof b, kCFStringEncodingUTF8);
                CFRelease(s);
            }
            if (!strcmp(b, name)) return e;
        }
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, false);
    }
    return 0;
}
static int gone(const char *name, int dest, int seconds) {
    for (int i = 0; i < seconds * 100; i++) {
        ItemCount n = dest ? MIDIGetNumberOfDestinations() : MIDIGetNumberOfSources();
        int there = 0;
        for (ItemCount k = 0; k < n; k++) {
            MIDIEndpointRef e = dest ? MIDIGetDestination(k) : MIDIGetSource(k);
            CFStringRef s = NULL;
            char b[100] = "";
            if (!MIDIObjectGetStringProperty(e, kMIDIPropertyName, &s) && s) {
                CFStringGetCString(s, b, sizeof b, kCFStringEncodingUTF8);
                CFRelease(s);
            }
            if (!strcmp(b, name)) there = 1;
        }
        if (!there) return 1;
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, false);
    }
    return 0;
}
static void send_bytes(MIDIEndpointRef dest, const Byte *d, int n) {
    Byte buf[64];
    MIDIPacketList *l = (MIDIPacketList *)buf;
    MIDIPacket *p = MIDIPacketListInit(l);
    MIDIPacketListAdd(l, sizeof buf, p, 0, (ByteCount)n, d);
    MIDISend(out, dest, l);
}
static void emit_bytes(MIDIEndpointRef src, const Byte *d, int n) {
    Byte buf[64];
    MIDIPacketList *l = (MIDIPacketList *)buf;
    MIDIPacket *p = MIDIPacketListInit(l);
    MIDIPacketListAdd(l, sizeof buf, p, 0, (ByteCount)n, d);
    MIDIReceived(src, l);
}
static void show(const char *who, const MIDIPacketList *l) {
    printf("%s:", who);
    const MIDIPacket *p = &l->packet[0];
    for (unsigned i = 0; i < l->numPackets; i++, p = MIDIPacketNext(p))
        for (int j = 0; j < p->length; j++) printf(" %02x", p->data[j]);
    printf("\n");
}
static int wait_for(int n) {
    for (int i = 0; i < n; i++)
        if (dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 20 * NSEC_PER_SEC))) return 0;
    return 1;
}

static int child(void) {
    MIDIClientRef c;
    MIDIClientCreate(CFSTR("child"), 0, 0, &c);
    MIDIOutputPortCreate(c, CFSTR("out"), &out);
    sem = dispatch_semaphore_create(0);
    static Byte from_source[8];
    static int nfrom;
    MIDIEndpointRef src, dst;
    MIDISourceCreate(c, CFSTR("Child Source"), &src);
    MIDIPortRef in;
    MIDIInputPortCreateWithBlock(c, CFSTR("in"), &in, ^(const MIDIPacketList *l, void *r) {
        const MIDIPacket *p = &l->packet[0];
        for (int j = 0; j < p->length && nfrom < 8; j++) from_source[nfrom++] = p->data[j];
        dispatch_semaphore_signal(sem);
    });
    MIDIDestinationCreateWithBlock(c, CFSTR("Child Destination"), &dst, ^(const MIDIPacketList *l, void *r) {
        const MIDIPacket *p = &l->packet[0];
        if (p->data[0] == 0x90) {
            show("child got", l);
            MIDIEndpointRef psrc = find("Parent Source", 0, 5), pdst = find("Parent Destination", 1, 5);
            static const Byte cc[] = {0xb0, 7, 127}, off[] = {0x80, 60, 0};
            emit_bytes(src, cc, 3);
            send_bytes(pdst, off, 3);
            (void)psrc;
        } else {
            dispatch_semaphore_signal(sem);  // (the end)
        }
    });
    MIDIEndpointRef psrc = find("Parent Source", 0, 20);
    MIDIPortConnectSource(in, psrc, NULL);
    // Told the parent that this is ready: the parent sends when it has seen
    // the endpoints.
    // The note, then (from the parent's source and its end): two more.
    if (!wait_for(2)) {
        printf("child: nothing came\n");
        return 1;
    }
    printf("child got from the parent's source:");
    for (int i = 0; i < nfrom; i++) printf(" %02x", from_source[i]);
    printf("\n");
    return 0;
}

int main(int argc, char **argv) {
    setvbuf(stdout, 0, _IONBF, 0);
    if (getenv("MIDI_PEER_CHILD")) return child();
    MIDIClientRef c;
    printf("client: %d\n", (int)MIDIClientCreate(CFSTR("parent"), 0, 0, &c));
    MIDIOutputPortCreate(c, CFSTR("out"), &out);
    sem = dispatch_semaphore_create(0);
    static Byte from_child_source[8], to_destination[8];
    static int nsrc, ndst;
    MIDIEndpointRef src, dst;
    MIDISourceCreate(c, CFSTR("Parent Source"), &src);
    MIDIDestinationCreateWithBlock(c, CFSTR("Parent Destination"), &dst, ^(const MIDIPacketList *l, void *r) {
        const MIDIPacket *p = &l->packet[0];
        for (int j = 0; j < p->length && ndst < 8; j++) to_destination[ndst++] = p->data[j];
        dispatch_semaphore_signal(sem);
    });
    MIDIPortRef in;
    MIDIInputPortCreateWithBlock(c, CFSTR("in"), &in, ^(const MIDIPacketList *l, void *r) {
        const MIDIPacket *p = &l->packet[0];
        for (int j = 0; j < p->length && nsrc < 8; j++) from_child_source[nsrc++] = p->data[j];
        dispatch_semaphore_signal(sem);
    });
    // The other process.
    char **env = NULL;
    int ne = 0;
    while (environ[ne]) ne++;
    env = malloc((size_t)(ne + 2) * sizeof *env);
    memcpy(env, environ, (size_t)ne * sizeof *env);
    env[ne] = "MIDI_PEER_CHILD=1";
    env[ne + 1] = NULL;
    pid_t pid;
    char *args[] = {argv[0], NULL};
    printf("spawn: %d\n", posix_spawn(&pid, argv[0], NULL, NULL, args, env));
    MIDIEndpointRef csrc = find("Child Source", 0, 20), cdst = find("Child Destination", 1, 20);
    printf("the child's endpoints: source %d, destination %d\n", csrc != 0, cdst != 0);
    if (!csrc || !cdst) return 1;
    CFStringRef s = NULL;
    char b[100];
    MIDIObjectGetStringProperty(csrc, kMIDIPropertyDisplayName, &s);
    CFStringGetCString(s, b, sizeof b, kCFStringEncodingUTF8);
    printf("its name: %s\n", b);
    SInt32 uid = 0;
    printf("its id: %d nonzero %d\n", (int)MIDIObjectGetIntegerProperty(csrc, kMIDIPropertyUniqueID, &uid), uid != 0);
    MIDIObjectRef o = 0;
    MIDIObjectType t = 0;
    printf("found by id: %d %d %d\n", (int)MIDIObjectFindByUniqueID(uid, &o, &t), o == csrc, (int)t);
    printf("connect: %d\n", (int)MIDIPortConnectSource(in, csrc, NULL));
    static const Byte on[] = {0x90, 60, 100};
    send_bytes(cdst, on, 3);
    if (!wait_for(2)) {
        printf("parent: nothing came\n");
        return 1;
    }
    printf("from the child's source:");
    for (int i = 0; i < nsrc; i++) printf(" %02x", from_child_source[i]);
    printf("\nto the parent's destination:");
    for (int i = 0; i < ndst; i++) printf(" %02x", to_destination[i]);
    printf("\n");
    static const Byte pb[] = {0xe0, 0, 0x40}, end[] = {0xfc};
    emit_bytes(src, pb, 3);
    send_bytes(cdst, end, 1);
    int status = 0;
    waitpid(pid, &status, 0);
    printf("the child: exit %d\n", WIFEXITED(status) ? WEXITSTATUS(status) : -1);
    printf("its endpoints are gone: %d %d\n", gone("Child Source", 0, 20), gone("Child Destination", 1, 20));
    return 0;
}
