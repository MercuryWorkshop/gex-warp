// macview: a window on a Mac guest's display, for the Mac host. It shows
// the framebuffer gexc keeps live (GEX_FRAMEBUFFER_LIVE=FILE) and gives
// the guest the window's mouse and keys (GEX_HID_INPUT=FILE), as lines of
// text appended to that file.
//
//   macview FRAMEBUFFER [INPUT]
//
// A run is shown only when it is asked for: nothing here starts by itself.
#import <Cocoa/Cocoa.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

static const char *fbPath;
static FILE *input;

// What the guest is told, a line at a time.
static void say(NSString *line) {
    if (!input) return;
    fprintf(input, "%s\n", line.UTF8String);
    fflush(input);
}

@interface GexView : NSView
@property(nonatomic) uint32_t width, height;
@end

@implementation GexView {
    void *map;
    size_t mapped;
    uint64_t seq;
}

- (BOOL)acceptsFirstResponder { return YES; }
- (BOOL)isFlipped { return YES; }

// The file mapped afresh when its size is not what was mapped.
- (BOOL)remap {
    int fd = open(fbPath, O_RDONLY);
    if (fd < 0) return NO;
    struct stat st;
    BOOL ok = NO;
    if (fstat(fd, &st) == 0 && st.st_size > 4096) {
        if ((size_t)st.st_size != mapped) {
            if (map) munmap(map, mapped);
            map = mmap(NULL, st.st_size, PROT_READ, MAP_SHARED, fd, 0);
            mapped = map == MAP_FAILED ? 0 : st.st_size;
            if (!mapped) map = NULL;
        }
        ok = map != NULL && memcmp(map, "GEXFB001", 8) == 0;
    }
    close(fd);
    return ok;
}

- (void)tick {
    if (![self remap]) return;
    const uint8_t *h = map;
    uint32_t w, ht, row, format;
    uint64_t s;
    memcpy(&w, h + 8, 4);
    memcpy(&ht, h + 12, 4);
    memcpy(&row, h + 16, 4);
    memcpy(&format, h + 20, 4);
    memcpy(&s, h + 24, 8);
    if (s == seq || format != 1 || 4096 + (size_t)row * ht > mapped) return;
    seq = s;
    if (w != self.width || ht != self.height) {
        self.width = w;
        self.height = ht;
        [self.window setContentSize:NSMakeSize(w, ht)];
    }
    // (A copy: the file is written over while it is shown.)
    CFDataRef data = CFDataCreate(NULL, h + 4096, (CFIndex)row * ht);
    CGDataProviderRef p = CGDataProviderCreateWithCFData(data);
    CGColorSpaceRef cs = CGColorSpaceCreateDeviceRGB();
    CGImageRef img = CGImageCreate(w, ht, 8, 32, row, cs, kCGBitmapByteOrder32Little | kCGImageAlphaNoneSkipFirst, p, NULL, false, kCGRenderingIntentDefault);
    self.layer.contents = (__bridge id)img;
    CGImageRelease(img);
    CGColorSpaceRelease(cs);
    CGDataProviderRelease(p);
    CFRelease(data);
}

- (NSPoint)guestPoint:(NSEvent *)e {
    NSPoint p = [self convertPoint:e.locationInWindow fromView:nil];
    NSSize s = self.bounds.size;
    if (s.width > 0 && s.height > 0 && self.width) {
        p.x = p.x * self.width / s.width;
        p.y = p.y * self.height / s.height;
    }
    return p;
}

- (void)moved:(NSEvent *)e {
    NSPoint p = [self guestPoint:e];
    say([NSString stringWithFormat:@"move %d %d", (int)p.x, (int)p.y]);
}

- (void)mouseMoved:(NSEvent *)e { [self moved:e]; }
- (void)mouseDragged:(NSEvent *)e { [self moved:e]; }
- (void)rightMouseDragged:(NSEvent *)e { [self moved:e]; }
- (void)mouseDown:(NSEvent *)e { [self moved:e]; say(@"down 0"); }
- (void)mouseUp:(NSEvent *)e { [self moved:e]; say(@"up 0"); }
- (void)rightMouseDown:(NSEvent *)e { [self moved:e]; say(@"down 1"); }
- (void)rightMouseUp:(NSEvent *)e { [self moved:e]; say(@"up 1"); }
- (void)scrollWheel:(NSEvent *)e {
    int dx = (int)e.scrollingDeltaX, dy = (int)e.scrollingDeltaY;
    if (dx || dy) say([NSString stringWithFormat:@"scroll %d %d", dx, dy]);
}
- (void)keyDown:(NSEvent *)e {
    if (!e.isARepeat) say([NSString stringWithFormat:@"key %d down", e.keyCode]);
}
- (void)keyUp:(NSEvent *)e { say([NSString stringWithFormat:@"key %d up", e.keyCode]); }

// The modifier keys come as changes of the flags: each one's key went
// down or up.
- (void)flagsChanged:(NSEvent *)e {
    NSEventModifierFlags f = e.modifierFlags;
    BOOL down = NO;
    switch (e.keyCode) {
    case 56: case 60: down = (f & NSEventModifierFlagShift) != 0; break;
    case 59: case 62: down = (f & NSEventModifierFlagControl) != 0; break;
    case 58: case 61: down = (f & NSEventModifierFlagOption) != 0; break;
    case 55: case 54: down = (f & NSEventModifierFlagCommand) != 0; break;
    case 57: down = (f & NSEventModifierFlagCapsLock) != 0; break;
    default: return;
    }
    say([NSString stringWithFormat:@"key %d %s", e.keyCode, down ? "down" : "up"]);
}
@end

int main(int argc, char **argv) {
    @autoreleasepool {
        if (argc < 2) {
            fprintf(stderr, "usage: macview FRAMEBUFFER [INPUT]\n");
            return 2;
        }
        fbPath = argv[1];
        if (argc > 2 && !(input = fopen(argv[2], "a"))) {
            perror(argv[2]);
            return 1;
        }
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
        NSWindow *w = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 1024, 768)
                                                  styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskMiniaturizable | NSWindowStyleMaskResizable
                                                    backing:NSBackingStoreBuffered
                                                      defer:NO];
        w.title = [NSString stringWithFormat:@"gex: %s", fbPath];
        GexView *v = [[GexView alloc] initWithFrame:w.contentView.bounds];
        v.wantsLayer = YES;
        v.layer.contentsGravity = kCAGravityResize;
        v.layer.backgroundColor = NSColor.blackColor.CGColor;
        w.contentView = v;
        w.acceptsMouseMovedEvents = YES;
        [w makeFirstResponder:v];
        [w center];
        [w makeKeyAndOrderFront:nil];
        [NSTimer scheduledTimerWithTimeInterval:1.0 / 30 repeats:YES block:^(NSTimer *t) { (void)t; [v tick]; }];
        [[NSNotificationCenter defaultCenter] addObserverForName:NSWindowWillCloseNotification object:w queue:nil usingBlock:^(NSNotification *n) { (void)n; [NSApp terminate:nil]; }];
        [NSApp activateIgnoringOtherApps:YES];
        [NSApp run];
    }
    return 0;
}
