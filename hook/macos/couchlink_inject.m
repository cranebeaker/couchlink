// libcouchlink_inject.dylib: gives one macOS game copy only its own seat's input (docs/macos.md).
//
// couchlinkd starts a native copy with DYLD_INSERT_LIBRARIES pointing here and these variables:
//   COUCHLINK_OWN_INPUT=1     hide every real controller from the game
//   COUCHLINK_PAD_FEED=PATH   the seat's pad feed (FeedBlock, hook/src/feed.h): shown as one controller
//   COUCHLINK_KBM_FEED=PATH   the seat's keyboard and mouse feed (hook/src/kbmfeed.h): posted as NSEvents
//
// What the game sees:
//   GameController framework: +[GCController controllers] and +current answer with one controller built from the
//     pad feed (a snapshot controller whose values are set from the feed), or none. Connect notifications for any
//     other controller are dropped.
//   IOKit HID: IOHIDManager hides joysticks and gamepads (and keyboards and mice when the seat has its own), both in
//     IOHIDManagerCopyDevices and in the matching and input callbacks.
//   AppKit: with a kbm feed, the seat's keys and mouse are posted into the app's own event queue, marked, and every
//     unmarked key and mouse event (the desktop keyboard) is dropped before the game sees it.
//
// macOS ignores DYLD_INSERT_LIBRARIES for programs built with the hardened runtime (most notarized games) and for
// system programs; this library then never loads and the copy sees every controller.

#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>
#import <GameController/GameController.h>
#import <IOKit/hid/IOHIDManager.h>
#include <fcntl.h>
#include <objc/runtime.h>
#include <pthread.h>
#include <stdatomic.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#define DYLD_INTERPOSE(_replacement, _replacee)                                                                  \
    __attribute__((used)) static struct {                                                                         \
        const void *replacement;                                                                                  \
        const void *replacee;                                                                                     \
    } _interpose_##_replacee __attribute__((section("__DATA,__interpose"))) = {(const void *)(unsigned long)&_replacement, \
                                                                              (const void *)(unsigned long)&_replacee};

// ---- feed layouts (little endian, same as Windows) ----------------------------------------------------------

typedef struct {
    uint32_t magic, version, seq, packet;
    uint16_t buttons;
    uint8_t lt, rt;
    int16_t lx, ly, rx, ry;
    uint32_t flags;
    uint16_t rumble_left, rumble_right;
    uint32_t rumble_seq, game_beat, host_beat;
    uint32_t reserved[4];
} FeedBlock;
_Static_assert(sizeof(FeedBlock) == 64, "FeedBlock is 64 bytes");

#define KBM_HEADER 128
#define KBM_CAP 1024
#define KBM_EVENT 16
#define KBM_TOTAL (KBM_HEADER + KBM_CAP * KBM_EVENT)

enum { XB_UP = 0x1, XB_DOWN = 0x2, XB_LEFT = 0x4, XB_RIGHT = 0x8, XB_START = 0x10, XB_BACK = 0x20, XB_LTHUMB = 0x40,
       XB_RTHUMB = 0x80, XB_LSHOULDER = 0x100, XB_RSHOULDER = 0x200, XB_GUIDE = 0x400, XB_A = 0x1000, XB_B = 0x2000,
       XB_X = 0x4000, XB_Y = 0x8000 };
enum { KEY_BREAK = 1 };
enum { M_LDOWN = 0x1, M_LUP = 0x2, M_RDOWN = 0x4, M_RUP = 0x8, M_MDOWN = 0x10, M_MUP = 0x20, M_WHEEL = 0x400 };

// Marks the events this library posts, so they pass the filter (kCGEventSourceUserData).
static const int64_t MARK = 0x434C4E4B; // "CLNK"

static int g_own;
static volatile FeedBlock *g_pad;
static volatile uint8_t *g_kbm;
static GCController *g_ctrl;
static uint32_t g_kbm_read;
static CGEventFlags g_mods;
static CGPoint g_cursor = {100, 100};

static uint32_t tick_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint32_t)((uint64_t)ts.tv_sec * 1000 + (uint64_t)ts.tv_nsec / 1000000);
}

static void *map_file(const char *path, size_t len) {
    if (!path || !*path) return NULL;
    int fd = open(path, O_RDWR | O_CLOEXEC);
    if (fd < 0) return NULL;
    void *p = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    close(fd);
    return p == MAP_FAILED ? NULL : p;
}

// Exported so a test program can tell that the library is loaded.
__attribute__((visibility("default"))) int couchlink_inject_loaded = 0;

// ---- pad: one snapshot controller fed from the feed ------------------------------------------------------------

static float axis(int16_t v) { return v < 0 ? v / 32768.0f : v / 32767.0f; }

static void apply_pad(void) {
    if (!g_pad || !g_ctrl) return;
    uint32_t s1, packet;
    FeedBlock b;
    do {
        s1 = g_pad->seq;
        if (s1 & 1) continue;
        atomic_thread_fence(memory_order_seq_cst);
        b = *(FeedBlock *)g_pad;
        atomic_thread_fence(memory_order_seq_cst);
    } while ((s1 & 1) || g_pad->seq != s1);
    packet = b.packet;
    g_pad->game_beat = tick_ms();
    static uint32_t last = UINT32_MAX;
    if (packet == last) return;
    last = packet;
    GCExtendedGamepad *g = g_ctrl.extendedGamepad;
    uint16_t k = b.buttons;
    [g.buttonA setValue:(k & XB_A) ? 1 : 0];
    [g.buttonB setValue:(k & XB_B) ? 1 : 0];
    [g.buttonX setValue:(k & XB_X) ? 1 : 0];
    [g.buttonY setValue:(k & XB_Y) ? 1 : 0];
    [g.leftShoulder setValue:(k & XB_LSHOULDER) ? 1 : 0];
    [g.rightShoulder setValue:(k & XB_RSHOULDER) ? 1 : 0];
    [g.buttonMenu setValue:(k & XB_START) ? 1 : 0];
    [g.buttonOptions setValue:(k & XB_BACK) ? 1 : 0];
    [g.buttonHome setValue:(k & XB_GUIDE) ? 1 : 0];
    [g.leftThumbstickButton setValue:(k & XB_LTHUMB) ? 1 : 0];
    [g.rightThumbstickButton setValue:(k & XB_RTHUMB) ? 1 : 0];
    [g.leftTrigger setValue:b.lt / 255.0f];
    [g.rightTrigger setValue:b.rt / 255.0f];
    [g.dpad setValueForXAxis:((k & XB_RIGHT) ? 1 : 0) - ((k & XB_LEFT) ? 1 : 0)
                       yAxis:((k & XB_UP) ? 1 : 0) - ((k & XB_DOWN) ? 1 : 0)];
    [g.leftThumbstick setValueForXAxis:axis(b.lx) yAxis:axis(b.ly)];
    [g.rightThumbstick setValueForXAxis:axis(b.rx) yAxis:axis(b.ry)];
}

static NSArray *own_controllers(id self, SEL _cmd) {
    return (g_ctrl && g_pad && (g_pad->flags & 1)) ? @[ g_ctrl ] : @[];
}

static GCController *own_current(id self, SEL _cmd) {
    return (g_ctrl && g_pad && (g_pad->flags & 1)) ? g_ctrl : nil;
}

static void (*orig_post)(id, SEL, NSNotification *);

static void filtered_post(id self, SEL _cmd, NSNotification *n) {
    NSString *name = n.name;
    BOOL controller = [name isEqualToString:GCControllerDidConnectNotification] ||
                      [name isEqualToString:GCControllerDidBecomeCurrentNotification];
    if (controller && n.object != g_ctrl) return;
    orig_post(self, _cmd, n);
}

static void hook_game_controller(void) {
    if (g_pad) {
        g_ctrl = [GCController controllerWithExtendedGamepad];
        g_ctrl.playerIndex = GCControllerPlayerIndex1;
    }
    Class meta = object_getClass([GCController class]);
    Method m = class_getClassMethod([GCController class], @selector(controllers));
    if (m) method_setImplementation(m, (IMP)own_controllers);
    m = class_getClassMethod([GCController class], @selector(current));
    if (m) method_setImplementation(m, (IMP)own_current);
    (void)meta;
    Method post = class_getInstanceMethod([NSNotificationCenter class], @selector(postNotification:));
    orig_post = (void (*)(id, SEL, NSNotification *))method_setImplementation(post, (IMP)filtered_post);
    if (g_ctrl) {
        // A game that waits for a connect notification gets one for the seat's controller.
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 500 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
          [[NSNotificationCenter defaultCenter] postNotificationName:GCControllerDidConnectNotification object:g_ctrl];
        });
    }
}

// ---- IOKit HID: hide the real devices ---------------------------------------------------------------------------

static int32_t prop_int(IOHIDDeviceRef d, CFStringRef key) {
    CFTypeRef v = IOHIDDeviceGetProperty(d, key);
    int32_t out = 0;
    if (v && CFGetTypeID(v) == CFNumberGetTypeID()) CFNumberGetValue((CFNumberRef)v, kCFNumberSInt32Type, &out);
    return out;
}

static bool hidden(IOHIDDeviceRef d) {
    if (!g_own || !d) return false;
    int32_t page = prop_int(d, CFSTR(kIOHIDPrimaryUsagePageKey));
    int32_t usage = prop_int(d, CFSTR(kIOHIDPrimaryUsageKey));
    if (page != kHIDPage_GenericDesktop) return false;
    if (usage == kHIDUsage_GD_Joystick || usage == kHIDUsage_GD_GamePad || usage == kHIDUsage_GD_MultiAxisController) return true;
    return g_kbm && (usage == kHIDUsage_GD_Keyboard || usage == kHIDUsage_GD_Keypad || usage == kHIDUsage_GD_Mouse);
}

static CFSetRef my_IOHIDManagerCopyDevices(IOHIDManagerRef m) {
    CFSetRef all = IOHIDManagerCopyDevices(m);
    if (!all || !g_own) return all;
    CFIndex n = CFSetGetCount(all);
    const void **items = calloc(n ? n : 1, sizeof(void *));
    CFSetGetValues(all, items);
    CFMutableSetRef out = CFSetCreateMutable(kCFAllocatorDefault, n, &kCFTypeSetCallBacks);
    for (CFIndex i = 0; i < n; i++)
        if (!hidden((IOHIDDeviceRef)items[i])) CFSetAddValue(out, items[i]);
    free(items);
    CFRelease(all);
    return out;
}
DYLD_INTERPOSE(my_IOHIDManagerCopyDevices, IOHIDManagerCopyDevices)

#define MAX_HOOKS 32
typedef struct {
    IOHIDManagerRef mgr;
    IOHIDDeviceCallback dev_cb;
    void *dev_ctx;
    IOHIDValueCallback val_cb;
    void *val_ctx;
} Hook;
static Hook g_match[MAX_HOOKS], g_remove[MAX_HOOKS], g_value[MAX_HOOKS];
static pthread_mutex_t g_hooks_lock = PTHREAD_MUTEX_INITIALIZER;

static Hook *slot(Hook *table, IOHIDManagerRef m) {
    for (int i = 0; i < MAX_HOOKS; i++)
        if (table[i].mgr == m) return &table[i];
    for (int i = 0; i < MAX_HOOKS; i++)
        if (!table[i].mgr) {
            table[i].mgr = m;
            return &table[i];
        }
    return NULL;
}

static void match_tramp(void *ctx, IOReturn r, void *sender, IOHIDDeviceRef d) {
    Hook *h = ctx;
    if (!hidden(d) && h->dev_cb) h->dev_cb(h->dev_ctx, r, sender, d);
}

static void value_tramp(void *ctx, IOReturn r, void *sender, IOHIDValueRef v) {
    Hook *h = ctx;
    if (!hidden(IOHIDElementGetDevice(IOHIDValueGetElement(v))) && h->val_cb) h->val_cb(h->val_ctx, r, sender, v);
}

static void register_dev(Hook *table, IOHIDManagerRef m, IOHIDDeviceCallback cb, void *ctx,
                         void (*real)(IOHIDManagerRef, IOHIDDeviceCallback, void *)) {
    pthread_mutex_lock(&g_hooks_lock);
    Hook *h = slot(table, m);
    if (h) {
        h->dev_cb = cb;
        h->dev_ctx = ctx;
    }
    pthread_mutex_unlock(&g_hooks_lock);
    if (h && cb)
        real(m, match_tramp, h);
    else
        real(m, cb, ctx);
}

static void my_IOHIDManagerRegisterDeviceMatchingCallback(IOHIDManagerRef m, IOHIDDeviceCallback cb, void *ctx) {
    register_dev(g_match, m, cb, ctx, IOHIDManagerRegisterDeviceMatchingCallback);
}
DYLD_INTERPOSE(my_IOHIDManagerRegisterDeviceMatchingCallback, IOHIDManagerRegisterDeviceMatchingCallback)

static void my_IOHIDManagerRegisterDeviceRemovalCallback(IOHIDManagerRef m, IOHIDDeviceCallback cb, void *ctx) {
    register_dev(g_remove, m, cb, ctx, IOHIDManagerRegisterDeviceRemovalCallback);
}
DYLD_INTERPOSE(my_IOHIDManagerRegisterDeviceRemovalCallback, IOHIDManagerRegisterDeviceRemovalCallback)

static void my_IOHIDManagerRegisterInputValueCallback(IOHIDManagerRef m, IOHIDValueCallback cb, void *ctx) {
    pthread_mutex_lock(&g_hooks_lock);
    Hook *h = slot(g_value, m);
    if (h) {
        h->val_cb = cb;
        h->val_ctx = ctx;
    }
    pthread_mutex_unlock(&g_hooks_lock);
    if (h && cb)
        IOHIDManagerRegisterInputValueCallback(m, value_tramp, h);
    else
        IOHIDManagerRegisterInputValueCallback(m, cb, ctx);
}
DYLD_INTERPOSE(my_IOHIDManagerRegisterInputValueCallback, IOHIDManagerRegisterInputValueCallback)

// ---- keyboard and mouse: the seat's own, as NSEvents ------------------------------------------------------------

// HID keyboard usage to macOS virtual key code (kVK_*); 0xFFFF when there is none.
static uint16_t mac_key(uint16_t u) {
    static const uint16_t letters[26] = {0x00, 0x0B, 0x08, 0x02, 0x0E, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2E,
                                         0x2D, 0x1F, 0x23, 0x0C, 0x0F, 0x01, 0x11, 0x20, 0x09, 0x0D, 0x07, 0x10, 0x06};
    static const uint16_t digits[10] = {0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19, 0x1D};
    static const uint16_t fkeys[12] = {0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F};
    if (u >= 0x04 && u <= 0x1D) return letters[u - 0x04];
    if (u >= 0x1E && u <= 0x27) return digits[u - 0x1E];
    if (u >= 0x3A && u <= 0x45) return fkeys[u - 0x3A];
    switch (u) {
    case 0x28: return 0x24; case 0x29: return 0x35; case 0x2A: return 0x33; case 0x2B: return 0x30;
    case 0x2C: return 0x31; case 0x2D: return 0x1B; case 0x2E: return 0x18; case 0x2F: return 0x21;
    case 0x30: return 0x1E; case 0x31: return 0x2A; case 0x33: return 0x29; case 0x34: return 0x27;
    case 0x35: return 0x32; case 0x36: return 0x2B; case 0x37: return 0x2F; case 0x38: return 0x2C;
    case 0x39: return 0x39; case 0x4A: return 0x73; case 0x4B: return 0x74; case 0x4C: return 0x75;
    case 0x4D: return 0x77; case 0x4E: return 0x79; case 0x4F: return 0x7C; case 0x50: return 0x7B;
    case 0x51: return 0x7D; case 0x52: return 0x7E; case 0x58: return 0x4C;
    case 0xE0: return 0x3B; case 0xE1: return 0x38; case 0xE2: return 0x3A; case 0xE3: return 0x37;
    case 0xE4: return 0x3E; case 0xE5: return 0x3C; case 0xE6: return 0x3D; case 0xE7: return 0x36;
    }
    return 0xFFFF;
}

static CGEventFlags mod_flag(uint16_t u) {
    switch (u) {
    case 0xE0: case 0xE4: return kCGEventFlagMaskControl;
    case 0xE1: case 0xE5: return kCGEventFlagMaskShift;
    case 0xE2: case 0xE6: return kCGEventFlagMaskAlternate;
    case 0xE3: case 0xE7: return kCGEventFlagMaskCommand;
    }
    return 0;
}

static void post_cg(CGEventRef e) {
    if (!e) return;
    CGEventSetIntegerValueField(e, kCGEventSourceUserData, MARK);
    CGEventSetFlags(e, g_mods);
    NSEvent *ev = [NSEvent eventWithCGEvent:e];
    CFRelease(e);
    if (ev && NSApp) [NSApp postEvent:ev atStart:NO];
}

static void post_key(uint16_t usage, bool down) {
    uint16_t code = mac_key(usage);
    if (code == 0xFFFF) return;
    CGEventFlags f = mod_flag(usage);
    if (f) g_mods = down ? (g_mods | f) : (g_mods & ~f);
    CGEventRef e = CGEventCreateKeyboardEvent(NULL, code, down);
    if (f && e) CGEventSetType(e, kCGEventFlagsChanged);
    post_cg(e);
}

static void post_mouse(int32_t dx, int32_t dy, uint16_t flags, int16_t wheel) {
    if (dx || dy) {
        g_cursor.x += dx;
        g_cursor.y += dy;
        CGEventRef e = CGEventCreateMouseEvent(NULL, kCGEventMouseMoved, g_cursor, kCGMouseButtonLeft);
        if (e) {
            CGEventSetIntegerValueField(e, kCGMouseEventDeltaX, dx);
            CGEventSetIntegerValueField(e, kCGMouseEventDeltaY, dy);
        }
        post_cg(e);
    }
    struct { uint16_t flag; CGEventType type; CGMouseButton button; } map[] = {
        {M_LDOWN, kCGEventLeftMouseDown, kCGMouseButtonLeft},   {M_LUP, kCGEventLeftMouseUp, kCGMouseButtonLeft},
        {M_RDOWN, kCGEventRightMouseDown, kCGMouseButtonRight}, {M_RUP, kCGEventRightMouseUp, kCGMouseButtonRight},
        {M_MDOWN, kCGEventOtherMouseDown, kCGMouseButtonCenter}, {M_MUP, kCGEventOtherMouseUp, kCGMouseButtonCenter},
    };
    for (size_t i = 0; i < sizeof map / sizeof map[0]; i++)
        if (flags & map[i].flag) post_cg(CGEventCreateMouseEvent(NULL, map[i].type, g_cursor, map[i].button));
    if ((flags & M_WHEEL) && wheel) post_cg(CGEventCreateScrollWheelEvent(NULL, kCGScrollEventUnitLine, 1, wheel / 120));
}

static void drain_kbm(void) {
    if (!g_kbm || !NSApp) return;
    uint32_t head = *(volatile uint32_t *)(g_kbm + 12);
    atomic_thread_fence(memory_order_seq_cst);
    if (head - g_kbm_read > KBM_CAP) g_kbm_read = head - KBM_CAP; // fell behind: skip what was overwritten
    while (g_kbm_read != head) {
        const volatile uint8_t *p = g_kbm + KBM_HEADER + (g_kbm_read & (KBM_CAP - 1)) * KBM_EVENT;
        uint8_t kind = p[0];
        uint16_t a = *(volatile uint16_t *)(p + 2), b = *(volatile uint16_t *)(p + 4), c = *(volatile uint16_t *)(p + 6);
        int32_t x = *(volatile int32_t *)(p + 8), y = *(volatile int32_t *)(p + 12);
        if (kind == 1)
            post_key(b, !(c & KEY_BREAK));
        else if (kind == 2)
            post_mouse(x, y, a, (int16_t)b);
        g_kbm_read++;
    }
    *(volatile uint32_t *)(g_kbm + 24) = tick_ms();
    *(volatile uint32_t *)(g_kbm + 44) = g_kbm_read;
    *(volatile uint32_t *)(g_kbm + 48) = (uint32_t)getpid();
}

// The desktop keyboard and mouse never reach a copy whose seat has its own.
static NSEvent *(*orig_next)(id, SEL, NSEventMask, NSDate *, NSRunLoopMode, BOOL);

static bool foreign_input(NSEvent *ev) {
    switch (ev.type) {
    case NSEventTypeKeyDown: case NSEventTypeKeyUp: case NSEventTypeFlagsChanged:
    case NSEventTypeMouseMoved: case NSEventTypeLeftMouseDown: case NSEventTypeLeftMouseUp:
    case NSEventTypeRightMouseDown: case NSEventTypeRightMouseUp: case NSEventTypeOtherMouseDown:
    case NSEventTypeOtherMouseUp: case NSEventTypeLeftMouseDragged: case NSEventTypeRightMouseDragged:
    case NSEventTypeOtherMouseDragged: case NSEventTypeScrollWheel: break;
    default: return false;
    }
    CGEventRef cg = ev.CGEvent;
    return !cg || CGEventGetIntegerValueField(cg, kCGEventSourceUserData) != MARK;
}

static NSEvent *filtered_next(id self, SEL _cmd, NSEventMask mask, NSDate *until, NSRunLoopMode mode, BOOL dequeue) {
    for (;;) {
        NSEvent *ev = orig_next(self, _cmd, mask, until, mode, dequeue);
        if (!ev || !dequeue || !foreign_input(ev)) return ev;
    }
}

static void *feed_thread(void *arg) {
    (void)arg;
    for (;;) {
        @autoreleasepool {
            apply_pad();
            drain_kbm();
        }
        usleep(2000);
    }
    return NULL;
}

__attribute__((constructor)) static void couchlink_inject_init(void) {
    @autoreleasepool {
        const char *own = getenv("COUCHLINK_OWN_INPUT");
        g_own = own && own[0] == '1';
        const char *pad = getenv("COUCHLINK_PAD_FEED");
        const char *kbm = getenv("COUCHLINK_KBM_FEED");
        g_pad = map_file(pad, sizeof(FeedBlock));
        g_kbm = map_file(kbm, KBM_TOTAL);
        if (g_kbm) g_kbm_read = *(volatile uint32_t *)(g_kbm + 12);
        // Children the game starts do not get the library again unless they are games themselves.
        unsetenv("DYLD_INSERT_LIBRARIES");
        if (!g_own) return;
        hook_game_controller();
        if (g_kbm) {
            Method m = class_getInstanceMethod([NSApplication class], @selector(nextEventMatchingMask:untilDate:inMode:dequeue:));
            orig_next = (NSEvent * (*)(id, SEL, NSEventMask, NSDate *, NSRunLoopMode, BOOL)) method_setImplementation(m, (IMP)filtered_next);
        }
        pthread_t t;
        pthread_create(&t, NULL, feed_thread, NULL);
        pthread_detach(t);
        couchlink_inject_loaded = 1;
    }
}
