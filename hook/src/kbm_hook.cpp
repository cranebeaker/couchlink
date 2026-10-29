// Keyboard and mouse feed: one copy gets its own keyboard and mouse.
//
// The daemon writes the raw key and mouse events of the devices a seat owns into a shared-memory ring (kbmfeed.h).
// This file reads that ring and turns it into what the game would have seen from real hardware:
//   * window messages (WM_KEYDOWN/UP, WM_CHAR through ToUnicodeEx, WM_MOUSEMOVE, buttons, wheel, WM_INPUT), posted
//     to the copy's own windows and tagged with a signature in bits real messages never use;
//   * a filter in GetMessage/PeekMessage (msg_hook.cpp) that drops every keyboard and mouse message without that
//     signature, so the real keyboard and mouse of the desktop do not reach the copy;
//   * answers for GetKeyState, GetAsyncKeyState, GetKeyboardState, GetCursorPos, SetCursorPos, ClipCursor,
//     GetClipCursor, ShowCursor, SetCursor, GetCursorInfo, GetMessagePos, GetRawInputData, GetRawInputBuffer and
//     RegisterRawInputDevices from a per-copy state (a fake cursor that only this copy's mouse moves).
//
// This is the design of ProtoInput's raw input forwarding, message filters (signature bits), FakeMouseKeyboard
// state and the cursor / key state hooks (MIT, see THIRD_PARTY.md), written fresh on MinHook with a shared-memory
// ring instead of ProtoInput's pipe.
#include <algorithm>
#include <cstring>
#include <deque>

#include "kbm_internal.h"

namespace cl {
namespace {

// Signature bits. Real keyboard messages keep lParam bits 25 to 28 clear ("reserved"), real mouse messages keep
// wParam bits 8 to 11 clear (the key flags use bits 0 to 6) and real raw input handles never start with 0xAB.
constexpr LPARAM kSigKeyMask = 0x1E000000;
constexpr LPARAM kSigKey = 0x14000000;
constexpr WPARAM kSigMouseMask = 0x0F00;
constexpr WPARAM kSigMouse = 0x0A00;
constexpr uint32_t kSigRawMask = 0xFF000000;
constexpr uint32_t kSigRaw = 0xAB000000;

constexpr UINT kRawSlots = 256;
constexpr size_t kMaxPending = 512;

// RI_MOUSE_* button flags.
constexpr uint16_t kLeftDown = 0x1, kLeftUp = 0x2, kRightDown = 0x4, kRightUp = 0x8, kMiddleDown = 0x10, kMiddleUp = 0x20,
                   kX1Down = 0x40, kX1Up = 0x80, kX2Down = 0x100, kX2Up = 0x200, kWheel = 0x400, kHWheel = 0x800;

struct Post {
    HWND hwnd;
    UINT msg;
    WPARAM w;
    LPARAM l;
};

struct FakeKey {
    HWND hwnd;
    UINT msg;
    WPARAM w;
    LPARAM l;
    ULONGLONG at;
};

struct RawSlot {
    uint32_t id = 0;  // counter value the slot was filled for
    RAWINPUT data;
};

struct RawReg {
    bool on = false;
    bool nolegacy = false;
    HWND hwnd = nullptr;
};

struct State {
    uint32_t read = 0;
    bool first_reader = true;  // this process counts events_seen (a child hooked later turns it off)
    uint8_t down[256] = {};
    uint8_t toggled[256] = {};
    uint8_t async_press[256] = {};
    uint8_t dik[256] = {};
    uint32_t buttons = 0;  // bit 0 left, 1 right, 2 middle, 3 x1, 4 x2
    bool cursor_init = false;
    int cx = 0, cy = 0;  // client coordinates of `anchor`
    HWND anchor = nullptr;
    int show_count = 0;
    bool cursor_set = false;
    HCURSOR hcursor = nullptr;
    bool clip_on = false;
    RECT clip = {};
    RawReg reg_kbd, reg_mouse;
    RawSlot raw[kRawSlots];
    uint32_t raw_counter = 0;
    std::deque<RAWINPUT> raw_pending;
    std::deque<FakeKey> fake_keys;
    // DirectInput
    bool di_keys_on = false, di_mouse_on = false;
    std::deque<DiEvent> di_keys, di_mouse;
    int32_t di_dx = 0, di_dy = 0, di_dz = 0;
};

State g_s;
CRITICAL_SECTION g_cs;
INIT_ONCE g_cs_once = INIT_ONCE_STATIC_INIT;
std::atomic<bool> g_pump_started{false};

BOOL CALLBACK InitCs(PINIT_ONCE, PVOID, PVOID*) {
    InitializeCriticalSection(&g_cs);
    return TRUE;
}
struct Guard {
    Guard() {
        InitOnceExecuteOnce(&g_cs_once, InitCs, nullptr, nullptr);
        EnterCriticalSection(&g_cs);
    }
    ~Guard() { LeaveCriticalSection(&g_cs); }
};

// Originals.
SHORT(WINAPI* o_GetKeyState)(int) = nullptr;
SHORT(WINAPI* o_GetAsyncKeyState)(int) = nullptr;
BOOL(WINAPI* o_GetKeyboardState)(PBYTE) = nullptr;
BOOL(WINAPI* o_GetCursorPos)(LPPOINT) = nullptr;
BOOL(WINAPI* o_SetCursorPos)(int, int) = nullptr;
BOOL(WINAPI* o_ClipCursor)(const RECT*) = nullptr;
BOOL(WINAPI* o_GetClipCursor)(LPRECT) = nullptr;
int(WINAPI* o_ShowCursor)(BOOL) = nullptr;
HCURSOR(WINAPI* o_SetCursor)(HCURSOR) = nullptr;
BOOL(WINAPI* o_GetCursorInfo)(PCURSORINFO) = nullptr;
DWORD(WINAPI* o_GetMessagePos)() = nullptr;
UINT(WINAPI* o_GetRawInputData)(HRAWINPUT, UINT, LPVOID, PUINT, UINT) = nullptr;
UINT(WINAPI* o_GetRawInputBuffer)(PRAWINPUT, PUINT, UINT) = nullptr;
BOOL(WINAPI* o_RegisterRawInputDevices)(PCRAWINPUTDEVICE, UINT, UINT) = nullptr;

inline void Beat() {
    if (g_kbm) g_kbm->game_beat = GetTickCount();
}

// ---------------------------------------------------------------------------------------------- cursor

HWND Anchor() {
    HWND h = FindMainWindow();
    if (h) g_s.anchor = h;
    else if (g_s.anchor && !IsWindow(g_s.anchor)) g_s.anchor = nullptr;
    return g_s.anchor;
}

RECT ClientBounds(HWND anchor) {
    RECT r = {0, 0, GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)};
    if (anchor) {
        RECT c;
        if (GetClientRect(anchor, &c)) r = c;
    }
    return r;
}

void ClampCursor(HWND anchor) {
    RECT r = ClientBounds(anchor);
    if (g_s.clip_on) {
        RECT c = g_s.clip;
        if (anchor) {
            POINT a = {c.left, c.top}, b = {c.right, c.bottom};
            ScreenToClient(anchor, &a);
            ScreenToClient(anchor, &b);
            c = {a.x, a.y, b.x, b.y};
        }
        RECT i = {std::max(r.left, c.left), std::max(r.top, c.top), std::min(r.right, c.right), std::min(r.bottom, c.bottom)};
        if (i.right > i.left && i.bottom > i.top) r = i;
        else r = c;
    }
    if (r.right > r.left && r.bottom > r.top) {
        g_s.cx = std::min<int>(std::max<int>(g_s.cx, r.left), r.right - 1);
        g_s.cy = std::min<int>(std::max<int>(g_s.cy, r.top), r.bottom - 1);
    }
}

// Puts the cursor in the middle of the main window the first time it is needed.
void EnsureCursor(HWND anchor) {
    if (g_s.cursor_init) return;
    RECT r = ClientBounds(anchor);
    if (anchor && r.right <= r.left) return;  // window not laid out yet
    g_s.cx = (r.left + r.right) / 2;
    g_s.cy = (r.top + r.bottom) / 2;
    g_s.cursor_init = true;
}

POINT ScreenPos(HWND anchor) {
    POINT p = {g_s.cx, g_s.cy};
    if (anchor) ClientToScreen(anchor, &p);
    return p;
}

bool CursorVisible() { return g_s.show_count >= 0 && (!g_s.cursor_set || g_s.hcursor != nullptr); }

void PublishCursor(HWND anchor) {
    if (!g_kbm) return;
    g_kbm->cursor_x = g_s.cx;
    g_kbm->cursor_y = g_s.cy;
    g_kbm->cursor_visible = CursorVisible() ? 1 : 0;
    g_kbm->hook_pid = GetCurrentProcessId();
    (void)anchor;
}

// Window under the fake cursor (a child window when there is one) with the point in its client coordinates.
HWND MouseTarget(HWND anchor, POINT& client) {
    client = {g_s.cx, g_s.cy};
    HWND target = anchor;
    if (!anchor) return nullptr;
    for (int depth = 0; depth < 4; ++depth) {
        HWND child = ChildWindowFromPointEx(target, client, CWP_SKIPINVISIBLE | CWP_SKIPDISABLED | CWP_SKIPTRANSPARENT);
        if (!child || child == target) break;
        MapWindowPoints(target, child, &client, 1);
        target = child;
    }
    return target;
}

// ---------------------------------------------------------------------------------------------- keys

uint16_t NormalizeVk(uint16_t vk, uint16_t scan, bool e0) {
    switch (vk) {
        case VK_SHIFT: return scan == 0x36 ? VK_RSHIFT : VK_LSHIFT;
        case VK_CONTROL: return e0 ? VK_RCONTROL : VK_LCONTROL;
        case VK_MENU: return e0 ? VK_RMENU : VK_LMENU;
        default: return vk;
    }
}

uint16_t GenericVk(uint16_t vk) {
    switch (vk) {
        case VK_LSHIFT:
        case VK_RSHIFT: return VK_SHIFT;
        case VK_LCONTROL:
        case VK_RCONTROL: return VK_CONTROL;
        case VK_LMENU:
        case VK_RMENU: return VK_MENU;
        default: return vk;
    }
}

void FillKeyBytes(BYTE out[256]) {
    for (int i = 0; i < 256; ++i) out[i] = static_cast<BYTE>((g_s.down[i] ? 0x80 : 0) | (g_s.toggled[i] ? 1 : 0));
    const uint8_t mouse_vk[5] = {VK_LBUTTON, VK_RBUTTON, VK_MBUTTON, VK_XBUTTON1, VK_XBUTTON2};
    for (int b = 0; b < 5; ++b)
        if (g_s.buttons & (1u << b)) out[mouse_vk[b]] |= 0x80;
}

void QueueDi(std::deque<DiEvent>& q, uint32_t ofs, uint32_t data) {
    if (q.size() >= kMaxPending) q.pop_front();
    q.push_back({ofs, data, GetTickCount()});
}

struct Batch {
    std::vector<Post> posts;
    bool move_pending = false;
    // coalesced raw mouse movement
    bool raw_move = false;
    int raw_dx = 0, raw_dy = 0;
    uint8_t raw_dev = 0;
};

HANDLE DeviceHandle(uint8_t dev) {
    uint64_t h = g_kbm ? g_kbm->dev_handles[dev & 7] : 0;
    if (!h) h = 0x1000u + dev;
    return reinterpret_cast<HANDLE>(static_cast<uintptr_t>(h));
}

HWND RawTarget(const RawReg& reg, HWND anchor) { return (reg.hwnd && IsWindow(reg.hwnd)) ? reg.hwnd : anchor; }

void PostRaw(Batch& b, const RAWINPUT& ri, const RawReg& reg, HWND anchor) {
    // Keep a copy for GetRawInputBuffer and one addressable slot per WM_INPUT.
    if (g_s.raw_pending.size() >= kMaxPending) g_s.raw_pending.pop_front();
    g_s.raw_pending.push_back(ri);
    HWND target = RawTarget(reg, anchor);
    if (!target) return;
    uint32_t id = ++g_s.raw_counter & 0x00FFFFFF;
    RawSlot& slot = g_s.raw[id % kRawSlots];
    slot.id = id;
    slot.data = ri;
    b.posts.push_back({target, WM_INPUT, RIM_INPUT, static_cast<LPARAM>(kSigRaw | id)});
    Bump(C_KBM_RAW_MSGS);
}

void FlushRawMove(Batch& b, HWND anchor) {
    if (!b.raw_move) return;
    b.raw_move = false;
    if (!g_s.reg_mouse.on) return;
    RAWINPUT ri;
    memset(&ri, 0, sizeof(ri));
    ri.header.dwType = RIM_TYPEMOUSE;
    ri.header.dwSize = sizeof(RAWINPUTHEADER) + sizeof(RAWMOUSE);
    ri.header.hDevice = DeviceHandle(b.raw_dev);
    ri.header.wParam = RIM_INPUT;
    ri.data.mouse.usFlags = MOUSE_MOVE_RELATIVE;
    ri.data.mouse.lLastX = b.raw_dx;
    ri.data.mouse.lLastY = b.raw_dy;
    b.raw_dx = b.raw_dy = 0;
    PostRaw(b, ri, g_s.reg_mouse, anchor);
}

WPARAM MouseKeys() {
    WPARAM w = 0;
    if (g_s.buttons & 1) w |= MK_LBUTTON;
    if (g_s.buttons & 2) w |= MK_RBUTTON;
    if (g_s.buttons & 4) w |= MK_MBUTTON;
    if (g_s.buttons & 8) w |= MK_XBUTTON1;
    if (g_s.buttons & 16) w |= MK_XBUTTON2;
    if (g_s.down[VK_SHIFT]) w |= MK_SHIFT;
    if (g_s.down[VK_CONTROL]) w |= MK_CONTROL;
    return w;
}

void FlushMove(Batch& b, HWND anchor) {
    if (!b.move_pending) return;
    b.move_pending = false;
    if (g_s.reg_mouse.on && g_s.reg_mouse.nolegacy) return;
    POINT c;
    HWND target = MouseTarget(anchor, c);
    if (!target) return;
    b.posts.push_back({target, WM_MOUSEMOVE, MouseKeys() | kSigMouse, MAKELPARAM(static_cast<short>(c.x), static_cast<short>(c.y))});
    Bump(C_KBM_MOUSE_MSGS);
}

void OnKey(const KbmEvent& e, Batch& b, HWND anchor) {
    uint16_t scan = e.b;
    bool up = (e.c & kKbmKeyBreak) != 0;
    bool e0 = (e.c & kKbmKeyE0) != 0;
    uint16_t vk = e.a;
    if (vk == 0 || vk == 0xFF) return;
    if (e0 && scan == 0x2A) return;  // fake shift the keyboard driver adds around some extended keys
    vk = NormalizeVk(vk, scan, e0);
    bool was_down = g_s.down[vk] != 0;
    g_s.down[vk] = up ? 0 : 1;
    // Generic modifier = either side.
    g_s.down[VK_SHIFT] = g_s.down[VK_LSHIFT] | g_s.down[VK_RSHIFT];
    g_s.down[VK_CONTROL] = g_s.down[VK_LCONTROL] | g_s.down[VK_RCONTROL];
    g_s.down[VK_MENU] = g_s.down[VK_LMENU] | g_s.down[VK_RMENU];
    if (!up && !was_down) {
        g_s.async_press[vk] = 1;
        g_s.async_press[GenericVk(vk)] = 1;
        if (vk == VK_CAPITAL || vk == VK_NUMLOCK || vk == VK_SCROLL) g_s.toggled[vk] ^= 1;
    }
    uint8_t dik = static_cast<uint8_t>((scan & 0x7F) | (e0 ? 0x80 : 0));
    g_s.dik[dik] = up ? 0 : 0x80;
    if (g_s.di_keys_on) QueueDi(g_s.di_keys, dik, up ? 0 : 0x80);

    FlushMove(b, anchor);
    FlushRawMove(b, anchor);

    if (g_s.reg_kbd.on) {
        RAWINPUT ri;
        memset(&ri, 0, sizeof(ri));
        ri.header.dwType = RIM_TYPEKEYBOARD;
        ri.header.dwSize = sizeof(RAWINPUTHEADER) + sizeof(RAWKEYBOARD);
        ri.header.hDevice = DeviceHandle(e.dev);
        ri.header.wParam = RIM_INPUT;
        ri.data.keyboard.MakeCode = scan;
        ri.data.keyboard.Flags = static_cast<USHORT>((up ? RI_KEY_BREAK : RI_KEY_MAKE) | (e0 ? RI_KEY_E0 : 0) | ((e.c & kKbmKeyE1) ? RI_KEY_E1 : 0));
        ri.data.keyboard.VKey = GenericVk(vk);
        ri.data.keyboard.Message = up ? WM_KEYUP : WM_KEYDOWN;
        PostRaw(b, ri, g_s.reg_kbd, anchor);
    }
    if (!anchor || (g_s.reg_kbd.on && g_s.reg_kbd.nolegacy)) return;

    bool alt = g_s.down[VK_MENU] != 0;
    bool sys = alt || vk == VK_F10;
    UINT msg = up ? (sys ? WM_SYSKEYUP : WM_KEYUP) : (sys ? WM_SYSKEYDOWN : WM_KEYDOWN);
    uint32_t l = 1u | (static_cast<uint32_t>(scan & 0xFF) << 16) | (e0 ? 1u << 24 : 0) | ((alt && sys) ? 1u << 29 : 0) |
                 ((was_down || up) ? 1u << 30 : 0) | (up ? 1u << 31 : 0);
    WPARAM w = GenericVk(vk);
    b.posts.push_back({anchor, msg, w, static_cast<LPARAM>(l | static_cast<uint32_t>(kSigKey))});
    Bump(C_KBM_KEY_MSGS);
    if (up) return;
    FakeKey fk = {anchor, msg, w, static_cast<LPARAM>(l), GetTickCount64()};
    g_s.fake_keys.push_back(fk);
    while (g_s.fake_keys.size() > 32) g_s.fake_keys.pop_front();

    // WM_CHAR from the layout of the window's thread and the fake key state.
    BYTE keys[256];
    FillKeyBytes(keys);
    HKL layout = GetKeyboardLayout(GetWindowThreadProcessId(anchor, nullptr));
    wchar_t buf[8] = {};
    int n = ToUnicodeEx(GenericVk(vk), scan & 0xFF, keys, buf, 8, 0x4, layout);
    uint32_t chl = (l & ~(1u << 31)) | (was_down ? 1u << 30 : 0);
    if (n < 0) {
        b.posts.push_back({anchor, static_cast<UINT>(alt ? WM_SYSDEADCHAR : WM_DEADCHAR), static_cast<WPARAM>(buf[0]), static_cast<LPARAM>(chl | static_cast<uint32_t>(kSigKey))});
        Bump(C_KBM_CHAR_MSGS);
    } else {
        for (int i = 0; i < n; ++i) {
            b.posts.push_back({anchor, static_cast<UINT>(alt ? WM_SYSCHAR : WM_CHAR), static_cast<WPARAM>(buf[i]), static_cast<LPARAM>(chl | static_cast<uint32_t>(kSigKey))});
            Bump(C_KBM_CHAR_MSGS);
        }
    }
}

struct ButtonMap {
    uint16_t down, up;
    uint32_t bit;
    UINT msg_down, msg_up;
    WPARAM xbutton;
    uint8_t di_button;
};
const ButtonMap kButtons[5] = {
    {kLeftDown, kLeftUp, 1, WM_LBUTTONDOWN, WM_LBUTTONUP, 0, 0},
    {kRightDown, kRightUp, 2, WM_RBUTTONDOWN, WM_RBUTTONUP, 0, 1},
    {kMiddleDown, kMiddleUp, 4, WM_MBUTTONDOWN, WM_MBUTTONUP, 0, 2},
    {kX1Down, kX1Up, 8, WM_XBUTTONDOWN, WM_XBUTTONUP, XBUTTON1, 3},
    {kX2Down, kX2Up, 16, WM_XBUTTONDOWN, WM_XBUTTONUP, XBUTTON2, 4},
};

void OnMouse(const KbmEvent& e, Batch& b, HWND anchor) {
    EnsureCursor(anchor);
    int dx = e.x, dy = e.y;
    uint16_t flags = e.a;
    bool moved = false;
    if (dx || dy) {
        int ox = g_s.cx, oy = g_s.cy;
        g_s.cx += dx;
        g_s.cy += dy;
        ClampCursor(anchor);
        moved = g_s.cx != ox || g_s.cy != oy;
        g_s.di_dx += dx;
        g_s.di_dy += dy;
        if (g_s.di_mouse_on) {
            if (dx) QueueDi(g_s.di_mouse, 0, static_cast<uint32_t>(dx));
            if (dy) QueueDi(g_s.di_mouse, 4, static_cast<uint32_t>(dy));
        }
        if (moved) b.move_pending = true;
        if (g_s.reg_mouse.on) {
            if (b.raw_move && b.raw_dev != e.dev) FlushRawMove(b, anchor);
            b.raw_move = true;
            b.raw_dev = e.dev;
            b.raw_dx += dx;
            b.raw_dy += dy;
        }
    }
    if (!(flags & (kLeftDown | kLeftUp | kRightDown | kRightUp | kMiddleDown | kMiddleUp | kX1Down | kX1Up | kX2Down | kX2Up | kWheel | kHWheel)))
        return;

    FlushMove(b, anchor);
    FlushRawMove(b, anchor);
    bool legacy = !(g_s.reg_mouse.on && g_s.reg_mouse.nolegacy);
    POINT client;
    HWND target = MouseTarget(anchor, client);
    for (const ButtonMap& m : kButtons) {
        for (int is_up = 0; is_up < 2; ++is_up) {
            if (!(flags & (is_up ? m.up : m.down))) continue;
            if (is_up) g_s.buttons &= ~m.bit;
            else {
                g_s.buttons |= m.bit;
                g_s.async_press[m.bit == 1 ? VK_LBUTTON : m.bit == 2 ? VK_RBUTTON : m.bit == 4 ? VK_MBUTTON : m.bit == 8 ? VK_XBUTTON1 : VK_XBUTTON2] = 1;
            }
            if (g_s.di_mouse_on) QueueDi(g_s.di_mouse, 12u + m.di_button, is_up ? 0 : 0x80);
            if (legacy && target) {
                WPARAM w = MouseKeys() | kSigMouse | (m.xbutton << 16);
                b.posts.push_back({target, is_up ? m.msg_up : m.msg_down, w, MAKELPARAM(static_cast<short>(client.x), static_cast<short>(client.y))});
                Bump(C_KBM_MOUSE_MSGS);
            }
        }
    }
    for (int h = 0; h < 2; ++h) {
        if (!(flags & (h ? kHWheel : kWheel))) continue;
        short delta = static_cast<short>(e.b);
        if (!h) g_s.di_dz += delta;
        if (!h && g_s.di_mouse_on) QueueDi(g_s.di_mouse, 8, static_cast<uint32_t>(static_cast<int32_t>(delta)));
        HWND wnd = anchor;
        if (legacy && wnd) {
            POINT s = ScreenPos(anchor);
            WPARAM w = MAKEWPARAM(MouseKeys() | kSigMouse, static_cast<WORD>(delta));
            b.posts.push_back({wnd, static_cast<UINT>(h ? WM_MOUSEHWHEEL : WM_MOUSEWHEEL), w, MAKELPARAM(static_cast<short>(s.x), static_cast<short>(s.y))});
            Bump(C_KBM_MOUSE_MSGS);
        }
    }
    if (g_s.reg_mouse.on) {
        RAWINPUT ri;
        memset(&ri, 0, sizeof(ri));
        ri.header.dwType = RIM_TYPEMOUSE;
        ri.header.dwSize = sizeof(RAWINPUTHEADER) + sizeof(RAWMOUSE);
        ri.header.hDevice = DeviceHandle(e.dev);
        ri.header.wParam = RIM_INPUT;
        ri.data.mouse.usFlags = MOUSE_MOVE_RELATIVE;
        ri.data.mouse.usButtonFlags = flags;
        ri.data.mouse.usButtonData = e.b;
        PostRaw(b, ri, g_s.reg_mouse, anchor);
    }
}

bool ReadEvent(uint32_t idx, KbmEvent& out) {
    const volatile KbmEvent* ring = reinterpret_cast<const volatile KbmEvent*>(reinterpret_cast<const volatile uint8_t*>(g_kbm) + kKbmHeaderSize);
    const volatile KbmEvent& s = ring[idx & (kKbmRingCap - 1)];
    out.kind = s.kind;
    out.dev = s.dev;
    out.a = s.a;
    out.b = s.b;
    out.c = s.c;
    out.x = s.x;
    out.y = s.y;
    MemoryBarrier();
    uint32_t head = g_kbm->head;
    return static_cast<uint32_t>(head - idx) <= kKbmRingCap;  // false: the writer lapped us while we read
}

}  // namespace

void KbmDrain() {
    if (!g_kbm) return;
    if (g_kbm->head == g_s.read) return;
    Batch b;
    {
        Guard g;
        uint32_t head = g_kbm->head;
        MemoryBarrier();
        if (static_cast<uint32_t>(head - g_s.read) > kKbmRingCap) g_s.read = head - kKbmRingCap / 2;  // fell behind: skip the oldest
        HWND anchor = Anchor();
        EnsureCursor(anchor);
        uint32_t consumed = 0;
        while (g_s.read != head) {
            KbmEvent e;
            bool ok = ReadEvent(g_s.read, e);
            ++g_s.read;
            if (!ok) {
                g_s.read = g_kbm->head;
                break;
            }
            Bump(C_KBM_EVENTS);
            ++consumed;
            if (e.kind == kKbmKey) OnKey(e, b, anchor);
            else if (e.kind == kKbmMouse) OnMouse(e, b, anchor);
        }
        FlushMove(b, anchor);
        FlushRawMove(b, anchor);
        PublishCursor(anchor);
        if (consumed && g_s.first_reader) g_kbm->events_seen = g_kbm->events_seen + consumed;
    }
    for (const Post& p : b.posts) PostMessageW(p.hwnd, p.msg, p.w, p.l);
}


// ---------------------------------------------------------------------------------------------- message filter

bool KbmFilterMessage(MSG* m) {
    Beat();
    UINT u = m->message;
    if (u >= 0x100 && u <= 0x109) {  // WM_KEYDOWN .. WM_UNICHAR
        if ((m->lParam & kSigKeyMask) == kSigKey) {
            m->lParam &= ~kSigKeyMask;
            return true;
        }
        Bump(C_KBM_REAL_DROPPED);
        return false;
    }
    if (u >= WM_MOUSEFIRST && u <= 0x20E) {  // WM_MOUSEMOVE .. WM_MOUSEHWHEEL
        if ((m->wParam & kSigMouseMask) == kSigMouse) {
            m->wParam &= ~kSigMouseMask;
            Guard g;
            HWND anchor = Anchor();
            m->pt = ScreenPos(anchor);
            return true;
        }
        Bump(C_KBM_REAL_DROPPED);
        return false;
    }
    if (u >= WM_NCMOUSEMOVE && u <= 0xAD && u != 0xAA) {  // non-client mouse messages of the real mouse
        Bump(C_KBM_REAL_DROPPED);
        return false;
    }
    if (u == WM_INPUT) {
        if ((static_cast<uint32_t>(m->lParam) & kSigRawMask) == kSigRaw) return true;
        Bump(C_KBM_REAL_DROPPED);
        return false;
    }
    return true;
}

bool KbmTakeFakeKeyDown(const MSG* m) {
    if (m->message != WM_KEYDOWN && m->message != WM_SYSKEYDOWN) return false;
    Guard g;
    ULONGLONG now = GetTickCount64();
    for (auto it = g_s.fake_keys.begin(); it != g_s.fake_keys.end();) {
        if (now - it->at > 5000) {
            it = g_s.fake_keys.erase(it);
            continue;
        }
        if (it->hwnd == m->hwnd && it->msg == m->message && it->w == m->wParam && it->l == m->lParam) {
            g_s.fake_keys.erase(it);
            return true;
        }
        ++it;
    }
    return false;
}

// ---------------------------------------------------------------------------------------------- state hooks

namespace {

SHORT KeyStateFor(int vk, bool async) {
    vk &= 0xFF;
    bool d;
    switch (vk) {
        case VK_LBUTTON: d = (g_s.buttons & 1) != 0; break;
        case VK_RBUTTON: d = (g_s.buttons & 2) != 0; break;
        case VK_MBUTTON: d = (g_s.buttons & 4) != 0; break;
        case VK_XBUTTON1: d = (g_s.buttons & 8) != 0; break;
        case VK_XBUTTON2: d = (g_s.buttons & 16) != 0; break;
        default: d = g_s.down[vk] != 0; break;
    }
    SHORT r = d ? static_cast<SHORT>(0x8000) : 0;
    if (async) {
        if (g_s.async_press[vk]) {
            r |= 1;
            g_s.async_press[vk] = 0;
        }
    } else if (g_s.toggled[vk]) {
        r |= 1;
    }
    return r;
}

SHORT WINAPI d_GetKeyState(int vk) {
    Bump(C_KBM_STATE_QUERIES);
    Beat();
    KbmDrain();
    Guard g;
    return KeyStateFor(vk, false);
}
SHORT WINAPI d_GetAsyncKeyState(int vk) {
    Bump(C_KBM_STATE_QUERIES);
    Beat();
    KbmDrain();
    Guard g;
    return KeyStateFor(vk, true);
}
BOOL WINAPI d_GetKeyboardState(PBYTE out) {
    Bump(C_KBM_STATE_QUERIES);
    Beat();
    if (!out) return FALSE;
    KbmDrain();
    Guard g;
    FillKeyBytes(out);
    return TRUE;
}
BOOL WINAPI d_GetCursorPos(LPPOINT p) {
    Bump(C_KBM_CURSOR_QUERIES);
    Beat();
    if (!p) return FALSE;
    KbmDrain();
    Guard g;
    HWND anchor = Anchor();
    EnsureCursor(anchor);
    ClampCursor(anchor);
    *p = ScreenPos(anchor);
    return TRUE;
}
DWORD WINAPI d_GetMessagePos() {
    Beat();
    Guard g;
    HWND anchor = Anchor();
    EnsureCursor(anchor);
    POINT p = ScreenPos(anchor);
    return MAKELONG(static_cast<WORD>(p.x), static_cast<WORD>(p.y));
}
BOOL WINAPI d_SetCursorPos(int x, int y) {
    Bump(C_KBM_CURSOR_QUERIES);
    KbmDrain();
    Guard g;
    HWND anchor = Anchor();
    POINT p = {x, y};
    if (anchor) ScreenToClient(anchor, &p);
    g_s.cx = p.x;
    g_s.cy = p.y;
    g_s.cursor_init = true;
    ClampCursor(anchor);
    PublishCursor(anchor);
    return TRUE;  // the desktop's real cursor belongs to somebody else
}
BOOL WINAPI d_ClipCursor(const RECT* r) {
    Guard g;
    if (r) {
        g_s.clip = *r;
        g_s.clip_on = true;
    } else {
        g_s.clip_on = false;
    }
    ClampCursor(Anchor());
    return TRUE;
}
BOOL WINAPI d_GetClipCursor(LPRECT r) {
    if (!r) return FALSE;
    Guard g;
    if (g_s.clip_on) {
        *r = g_s.clip;
    } else {
        r->left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        r->top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        r->right = r->left + GetSystemMetrics(SM_CXVIRTUALSCREEN);
        r->bottom = r->top + GetSystemMetrics(SM_CYVIRTUALSCREEN);
    }
    return TRUE;
}
int WINAPI d_ShowCursor(BOOL show) {
    Guard g;
    g_s.show_count += show ? 1 : -1;
    PublishCursor(g_s.anchor);
    return g_s.show_count;  // the real counter would hide the desktop's cursor over this window
}
HCURSOR WINAPI d_SetCursor(HCURSOR h) {
    {
        Guard g;
        g_s.cursor_set = true;
        g_s.hcursor = h;
        PublishCursor(g_s.anchor);
    }
    return o_SetCursor(h);
}
BOOL WINAPI d_GetCursorInfo(PCURSORINFO ci) {
    Bump(C_KBM_CURSOR_QUERIES);
    if (!ci || ci->cbSize != sizeof(CURSORINFO)) return FALSE;
    KbmDrain();
    Guard g;
    HWND anchor = Anchor();
    EnsureCursor(anchor);
    bool vis = CursorVisible();
    ci->flags = vis ? CURSOR_SHOWING : 0;
    ci->hCursor = vis ? (g_s.hcursor ? g_s.hcursor : LoadCursorW(nullptr, IDC_ARROW)) : nullptr;
    ci->ptScreenPos = ScreenPos(anchor);
    return TRUE;
}

// Raw input registration: the keyboard and mouse entries are remembered and not given to the system (the real
// devices are not wanted); everything else (game pads, HID) goes through.
BOOL WINAPI d_RegisterRawInputDevices(PCRAWINPUTDEVICE devs, UINT n, UINT cb) {
    if (!devs || cb != sizeof(RAWINPUTDEVICE)) return o_RegisterRawInputDevices(devs, n, cb);
    std::vector<RAWINPUTDEVICE> rest;
    {
        Guard g;
        for (UINT i = 0; i < n; ++i) {
            const RAWINPUTDEVICE& d = devs[i];
            bool page_only = (d.dwFlags & RIDEV_PAGEONLY) && d.usUsage == 0;
            bool kbd = d.usUsagePage == 1 && (d.usUsage == 6 || d.usUsage == 7 || page_only);
            bool mouse = d.usUsagePage == 1 && (d.usUsage == 2 || d.usUsage == 1 || page_only);
            if (!kbd && !mouse) {
                rest.push_back(d);
                continue;
            }
            RawReg reg;
            if (!(d.dwFlags & RIDEV_REMOVE)) {
                reg.on = true;
                reg.nolegacy = (d.dwFlags & RIDEV_NOLEGACY) == RIDEV_NOLEGACY;
                reg.hwnd = d.hwndTarget;
            }
            if (kbd) g_s.reg_kbd = reg;
            if (mouse) g_s.reg_mouse = reg;
        }
    }
    if (rest.empty()) return TRUE;
    return o_RegisterRawInputDevices(rest.data(), static_cast<UINT>(rest.size()), cb);
}

UINT WINAPI d_GetRawInputData(HRAWINPUT h, UINT cmd, LPVOID data, PUINT size, UINT header_size) {
    uint32_t handle = static_cast<uint32_t>(reinterpret_cast<uintptr_t>(h));
    if ((handle & kSigRawMask) != kSigRaw) return o_GetRawInputData(h, cmd, data, size, header_size);
    Bump(C_KBM_RAW_READS);
    Beat();
    if (!size || header_size != sizeof(RAWINPUTHEADER)) return static_cast<UINT>(-1);
    RAWINPUT copy;
    {
        Guard g;
        uint32_t id = handle & 0x00FFFFFF;
        const RawSlot& slot = g_s.raw[id % kRawSlots];
        if (slot.id != id) return static_cast<UINT>(-1);
        copy = slot.data;
    }
    UINT need = cmd == RID_HEADER ? sizeof(RAWINPUTHEADER) : copy.header.dwSize;
    if (!data) {
        *size = need;
        return 0;
    }
    if (*size < need) {
        *size = need;
        return static_cast<UINT>(-1);
    }
    memcpy(data, &copy, need);
    return need;
}

UINT WINAPI d_GetRawInputBuffer(PRAWINPUT data, PUINT size, UINT header_size) {
    Bump(C_KBM_RAW_READS);
    Beat();
    if (!size || header_size != sizeof(RAWINPUTHEADER)) return static_cast<UINT>(-1);
    KbmDrain();
    Guard g;
    if (!data) {
        *size = g_s.raw_pending.empty() ? 0 : g_s.raw_pending.front().header.dwSize;
        return 0;
    }
    UINT used = 0, count = 0;
    const UINT align = sizeof(void*) == 8 ? 8 : 4;
    while (!g_s.raw_pending.empty()) {
        const RAWINPUT& ri = g_s.raw_pending.front();
        UINT block = (ri.header.dwSize + align - 1) & ~(align - 1);
        if (used + ri.header.dwSize > *size) break;
        memcpy(reinterpret_cast<BYTE*>(data) + used, &ri, ri.header.dwSize);
        used += block;
        ++count;
        g_s.raw_pending.pop_front();
    }
    if (count == 0 && !g_s.raw_pending.empty()) {
        *size = g_s.raw_pending.front().header.dwSize;
        return static_cast<UINT>(-1);  // the buffer is too small for even one entry
    }
    return count;
}

size_t TakeQueue(std::deque<DiEvent>& q, DiEvent* out, size_t max, bool peek) {
    size_t n = 0;
    while (n < max && n < q.size()) {
        out[n] = q[n];
        ++n;
    }
    if (!peek) q.erase(q.begin(), q.begin() + static_cast<long>(n));
    return n;
}

}  // namespace

// ---------------------------------------------------------------------------------------------- DirectInput feed

void KbmDiEnable(bool keyboard, bool mouse) {
    Guard g;
    if (keyboard) g_s.di_keys_on = true;
    if (mouse) g_s.di_mouse_on = true;
}

size_t KbmDiTakeKeys(DiEvent* out, size_t max, bool peek) {
    Bump(C_KBM_DINPUT_READS);
    Beat();
    KbmDrain();
    Guard g;
    return TakeQueue(g_s.di_keys, out, max, peek);
}

size_t KbmDiTakeMouse(DiEvent* out, size_t max, bool peek) {
    Bump(C_KBM_DINPUT_READS);
    Beat();
    KbmDrain();
    Guard g;
    return TakeQueue(g_s.di_mouse, out, max, peek);
}

void KbmDiKeyState(uint8_t out[256]) {
    Bump(C_KBM_DINPUT_READS);
    Beat();
    KbmDrain();
    Guard g;
    memcpy(out, g_s.dik, 256);
}

void KbmDiMouseState(int32_t* x, int32_t* y, int32_t* z, uint8_t buttons[8], bool consume) {
    Bump(C_KBM_DINPUT_READS);
    Beat();
    KbmDrain();
    Guard g;
    *x = g_s.di_dx;
    *y = g_s.di_dy;
    *z = g_s.di_dz;
    for (int i = 0; i < 8; ++i) buttons[i] = (i < 5 && (g_s.buttons & (1u << i))) ? 0x80 : 0;
    if (consume) g_s.di_dx = g_s.di_dy = g_s.di_dz = 0;
}

KbmCursorView KbmCursor() {
    KbmCursorView v;
    Guard g;
    HWND anchor = Anchor();
    if (!anchor || !g_s.cursor_init) return v;
    v.window = anchor;
    v.screen = ScreenPos(anchor);
    v.visible = CursorVisible();
    v.shape = g_s.hcursor;
    return v;
}

// ---------------------------------------------------------------------------------------------- install

namespace {

DWORD WINAPI PumpThread(LPVOID) {
    HANDLE timer = CreateWaitableTimerExW(nullptr, nullptr, 0x2 /* CREATE_WAITABLE_TIMER_HIGH_RESOLUTION */, TIMER_ALL_ACCESS);
    if (!timer) timer = CreateWaitableTimerExW(nullptr, nullptr, 0, TIMER_ALL_ACCESS);
    if (timer) {
        LARGE_INTEGER due;
        due.QuadPart = -20000;  // 2 ms
        SetWaitableTimer(timer, &due, 2, nullptr, nullptr, FALSE);
    }
    for (;;) {
        if (timer) WaitForSingleObject(timer, 50);
        else Sleep(1);
        KbmDrain();
    }
}

#define HK(mod, name) InstallHook(mod, #name, reinterpret_cast<void*>(&d_##name), reinterpret_cast<void**>(&o_##name))

}  // namespace

void InstallKbm() {
    if (!g_kbm) return;
    HMODULE u = GetModuleHandleW(L"user32.dll");
    if (!u) u = LoadLibraryW(L"user32.dll");
    {
        Guard g;
        g_s.read = g_kbm->head;
        g_s.first_reader = g_cfg.depth == 0;
    }
    HK(u, GetKeyState);
    HK(u, GetAsyncKeyState);
    HK(u, GetKeyboardState);
    HK(u, GetCursorPos);
    HK(u, SetCursorPos);
    HK(u, ClipCursor);
    HK(u, GetClipCursor);
    HK(u, ShowCursor);
    HK(u, SetCursor);
    HK(u, GetCursorInfo);
    HK(u, GetMessagePos);
    HK(u, GetRawInputData);
    HK(u, GetRawInputBuffer);
    HK(u, RegisterRawInputDevices);
    if (!g_pump_started.exchange(true)) {
        HANDLE t = CreateThread(nullptr, 0, PumpThread, nullptr, 0, nullptr);
        if (t) CloseHandle(t);
    }
    if (g_cfg.fake_cursor) StartFakeCursor();
    g_features.kbm = true;
    InstallKbmDInput();
}

}  // namespace cl
