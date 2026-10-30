// inputprobe: a stand-in game for the macOS input isolation test. It reads input the way games do (the
// GameController framework, IOKit HID, and AppKit key and mouse events) and prints every change on stdout:
//
//   ready injected=1 hidpads=0
//   pads 1 a=1 b=0 x=0 y=0 lx=0.00
//   key down 0
//   key up 0
//   mouse moved 5 -3
//
// usage: inputprobe [SECONDS] [--window]   (default 30 s; --window opens a 640x400 window titled "inputprobe")

#import <AppKit/AppKit.h>
#import <GameController/GameController.h>
#import <IOKit/hid/IOHIDManager.h>
#include <dlfcn.h>
#include <stdio.h>

static long hid_pads(void) {
    IOHIDManagerRef m = IOHIDManagerCreate(kCFAllocatorDefault, kIOHIDOptionsTypeNone);
    NSArray *match = @[
        @{@kIOHIDDeviceUsagePageKey : @(kHIDPage_GenericDesktop), @kIOHIDDeviceUsageKey : @(kHIDUsage_GD_GamePad)},
        @{@kIOHIDDeviceUsagePageKey : @(kHIDPage_GenericDesktop), @kIOHIDDeviceUsageKey : @(kHIDUsage_GD_Joystick)},
    ];
    IOHIDManagerSetDeviceMatchingMultiple(m, (__bridge CFArrayRef)match);
    CFSetRef set = IOHIDManagerCopyDevices(m);
    long n = set ? CFSetGetCount(set) : 0;
    if (set) CFRelease(set);
    CFRelease(m);
    return n;
}

int main(int argc, char **argv) {
    @autoreleasepool {
        double seconds = 30;
        BOOL window = NO;
        for (int i = 1; i < argc; i++) {
            if (strcmp(argv[i], "--window") == 0)
                window = YES;
            else
                seconds = atof(argv[i]);
        }
        setvbuf(stdout, NULL, _IOLBF, 0);
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:window ? NSApplicationActivationPolicyRegular : NSApplicationActivationPolicyProhibited];
        [NSApp finishLaunching];
        NSWindow *win = nil;
        if (window) {
            NSWindowStyleMask style = NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskResizable;
            win = [[NSWindow alloc] initWithContentRect:NSMakeRect(200, 200, 640, 400) styleMask:style backing:NSBackingStoreBuffered defer:NO];
            win.title = @"inputprobe";
            win.releasedWhenClosed = NO;
            [win makeKeyAndOrderFront:nil];
        }
        int *loaded = dlsym(RTLD_DEFAULT, "couchlink_inject_loaded");
        printf("ready injected=%d hidpads=%ld\n", loaded ? *loaded : 0, hid_pads());
        NSDate *end = [NSDate dateWithTimeIntervalSinceNow:seconds];
        char last[256] = "";
        while ([end timeIntervalSinceNow] > 0) {
            NSEvent *ev = [NSApp nextEventMatchingMask:NSEventMaskAny
                                             untilDate:[NSDate dateWithTimeIntervalSinceNow:0.02]
                                                inMode:NSDefaultRunLoopMode
                                               dequeue:YES];
            if (ev) {
                switch (ev.type) {
                case NSEventTypeKeyDown: printf("key down %d\n", ev.keyCode); break;
                case NSEventTypeKeyUp: printf("key up %d\n", ev.keyCode); break;
                case NSEventTypeMouseMoved: printf("mouse moved %.0f %.0f\n", ev.deltaX, ev.deltaY); break;
                case NSEventTypeLeftMouseDown: printf("mouse down left\n"); break;
                default: break;
                }
                [NSApp sendEvent:ev];
            }
            NSArray<GCController *> *cs = [GCController controllers];
            GCExtendedGamepad *g = cs.firstObject.extendedGamepad;
            char now[256];
            snprintf(now, sizeof now, "pads %lu a=%d b=%d x=%d y=%d lx=%.2f", (unsigned long)cs.count, g.buttonA.pressed,
                     g.buttonB.pressed, g.buttonX.pressed, g.buttonY.pressed, g.leftThumbstick.xAxis.value);
            if (strcmp(now, last) != 0) {
                printf("%s\n", now);
                strcpy(last, now);
            }
        }
    }
    return 0;
}
