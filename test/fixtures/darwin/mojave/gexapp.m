// gexapp: the smallest application with a window, for Mojave's desktop
// under gex -- what Calculator does to put its window on the screen,
// without the rest of Calculator. An NSApplication whose delegate, once
// AppKit has finished launching it (its first open event), makes a window
// with a coloured view, orders it front and makes itself the active
// application; then says, each time it changes, whether the window is
// visible, ordered in, on the screen (WindowServer's own list of what is
// on screen) and whether it is the active application. Every mouse and
// key event the application is given is said too. Built as an
// application bundle (with an Info.plist): see test/darwin/mojave-app.
#import <AppKit/AppKit.h>
#include <stdio.h>

@interface GexView : NSView
@end
@implementation GexView
- (void)drawRect:(NSRect)dirty {
    (void)dirty;
    [[NSColor colorWithCalibratedRed:0.9 green:0.3 blue:0.1 alpha:1] setFill];
    NSRectFill([self bounds]);
    [[NSColor whiteColor] setFill];
    NSRectFill(NSMakeRect(20, 20, 60, 60));
    static int drawn;
    if (!drawn++) printf("gexapp: drawn\n");
}
- (BOOL)acceptsFirstResponder { return YES; }
- (BOOL)acceptsFirstMouse:(NSEvent *)e { (void)e; return YES; }
@end

@interface GexDelegate : NSObject <NSApplicationDelegate>
@property(retain) NSWindow *window;
@end
@implementation GexDelegate
- (void)applicationWillFinishLaunching:(NSNotification *)n {
    (void)n;
    printf("gexapp: will finish launching\n");
}
- (void)applicationDidFinishLaunching:(NSNotification *)n {
    (void)n;
    printf("gexapp: did finish launching\n");
    NSWindow *w = [[NSWindow alloc] initWithContentRect:NSMakeRect(120, 200, 320, 200)
                                              styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable
                                                backing:NSBackingStoreBuffered
                                                  defer:NO];
    [w setTitle:@"gexapp"];
    GexView *v = [[GexView alloc] initWithFrame:NSMakeRect(0, 0, 320, 200)];
    [w setContentView:v];
    [w makeFirstResponder:v];
    self.window = w;
    [w makeKeyAndOrderFront:nil];
    [NSApp activateIgnoringOtherApps:YES];
    printf("gexapp: window %ld ordered front\n", (long)[w windowNumber]);
    [NSTimer scheduledTimerWithTimeInterval:2 target:self selector:@selector(look:) userInfo:nil repeats:YES];
    [self look:nil];
}
// What is so of the window now: said when it changes.
- (void)look:(NSTimer *)t {
    (void)t;
    NSWindow *w = self.window;
    BOOL listed = NO;
    CFArrayRef on = CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID);
    for (NSDictionary *d in (NSArray *)on)
        if ([d[(id)kCGWindowNumber] longValue] == [w windowNumber]) listed = YES;
    if (on) CFRelease(on);
    NSRect f = [w frame];
    char now[256];
    snprintf(now, sizeof now, "gexapp: window %ld at %.0f,%.0f %.0fx%.0f: visible %d, on screen %d, key %d, occlusion visible %d; active %d",
             (long)[w windowNumber], f.origin.x, f.origin.y, f.size.width, f.size.height, (int)[w isVisible], (int)listed, (int)[w isKeyWindow],
             (int)(([w occlusionState] & NSWindowOcclusionStateVisible) != 0), (int)[NSApp isActive]);
    static char said[256];
    if (strcmp(now, said)) {
        strcpy(said, now);
        printf("%s\n", now);
    }
}
@end

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    @autoreleasepool {
        printf("gexapp: up, pid %d\n", (int)getpid());
        NSApplication *app = [NSApplication sharedApplication];
        [app setActivationPolicy:NSApplicationActivationPolicyRegular];
        GexDelegate *d = [[GexDelegate alloc] init];
        [app setDelegate:d];
        // Every mouse and key event the application is given, before
        // AppKit sends it on.
        [NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskLeftMouseDown | NSEventMaskLeftMouseUp | NSEventMaskRightMouseDown |
                                                      NSEventMaskKeyDown | NSEventMaskKeyUp
                                              handler:^NSEvent *(NSEvent *e) {
                                                  NSPoint p = [e locationInWindow];
                                                  if ([e type] == NSEventTypeKeyDown || [e type] == NSEventTypeKeyUp)
                                                      printf("gexapp: event %lu key %u '%s' in window %ld\n", (unsigned long)[e type], (unsigned)[e keyCode],
                                                             [[e characters] UTF8String] ?: "", (long)[e windowNumber]);
                                                  else
                                                      printf("gexapp: event %lu at %.0f,%.0f in window %ld\n", (unsigned long)[e type], p.x, p.y, (long)[e windowNumber]);
                                                  return e;
                                              }];
        [app run];
    }
    return 0;
}
