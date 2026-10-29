// Focus faking: the copy always believes its window is the foreground, active and focused window, and the
// deactivate messages the system sends when another copy is clicked are turned into "still active".
//
// Same idea as ProtoInput's FocusHook / WindowActivate filters (MIT, see THIRD_PARTY.md). WM_ACTIVATE is usually
// *sent*, not posted, so besides filtering GetMessage/PeekMessage we subclass every top-level window the process
// creates and fix the messages in front of the game's own window procedure.
#include <cwchar>

#include "common.h"

namespace cl {
namespace {

std::atomic<HWND> g_main{nullptr};
std::atomic<ULONGLONG> g_last_scan{0};
const wchar_t kProp[] = L"CouchlinkOrigProc";

HWND(WINAPI* o_GetForegroundWindow)() = nullptr;
HWND(WINAPI* o_GetActiveWindow)() = nullptr;
HWND(WINAPI* o_GetFocus)() = nullptr;
BOOL(WINAPI* o_SetForegroundWindow)(HWND) = nullptr;
HWND(WINAPI* o_CreateWindowExW)(DWORD, LPCWSTR, LPCWSTR, DWORD, int, int, int, int, HWND, HMENU, HINSTANCE, LPVOID) = nullptr;
HWND(WINAPI* o_CreateWindowExA)(DWORD, LPCSTR, LPCSTR, DWORD, int, int, int, int, HWND, HMENU, HINSTANCE, LPVOID) = nullptr;

bool IsSystemHelperClass(HWND h) {
    wchar_t cls[64] = {};
    GetClassNameW(h, cls, ARRAYSIZE(cls));
    return _wcsicmp(cls, L"IME") == 0 || _wcsicmp(cls, L"MSCTFIME UI") == 0 || _wcsicmp(cls, L"Default IME") == 0;
}

struct FindCtx {
    DWORD pid;
    HWND best = nullptr;
    long long area = 0;
};

BOOL CALLBACK EnumProc(HWND h, LPARAM lp) {
    FindCtx* c = reinterpret_cast<FindCtx*>(lp);
    DWORD pid = 0;
    GetWindowThreadProcessId(h, &pid);
    if (pid != c->pid || !IsWindowVisible(h) || GetWindow(h, GW_OWNER) != nullptr || IsSystemHelperClass(h)) return TRUE;
    RECT r;
    if (!GetWindowRect(h, &r)) return TRUE;
    long long area = static_cast<long long>(r.right - r.left) * (r.bottom - r.top);
    if (area > c->area || !c->best) {
        c->area = area;
        c->best = h;
    }
    return TRUE;
}

}  // namespace

// The largest visible top-level window of this process; nullptr until there is one.
HWND FindMainWindow() {
    HWND h = g_main.load();
    if (h && IsWindow(h) && IsWindowVisible(h)) return h;
    ULONGLONG now = GetTickCount64();
    ULONGLONG last = g_last_scan.load();
    if (now - last < 100 && last != 0) return nullptr;
    g_last_scan = now;
    FindCtx ctx;
    ctx.pid = GetCurrentProcessId();
    EnumWindows(EnumProc, reinterpret_cast<LPARAM>(&ctx));
    g_main = ctx.best;
    return ctx.best;
}

namespace {

HWND MainWindow() { return FindMainWindow(); }

HWND WINAPI d_GetForegroundWindow() {
    Bump(C_FOCUS_QUERY);
    HWND h = MainWindow();
    return h ? h : o_GetForegroundWindow();
}
HWND WINAPI d_GetActiveWindow() {
    Bump(C_FOCUS_QUERY);
    HWND h = MainWindow();
    return h ? h : o_GetActiveWindow();
}
HWND WINAPI d_GetFocus() {
    Bump(C_FOCUS_QUERY);
    HWND h = MainWindow();
    return h ? h : o_GetFocus();
}
BOOL WINAPI d_SetForegroundWindow(HWND h) {
    // A copy must never pull the foreground away from the player's real window.
    (void)h;
    return TRUE;
}

// Rewrites a "you lost focus" message into "you have focus". Returns true when something was changed.
bool FixActivation(UINT msg, WPARAM& w, LPARAM& l) {
    switch (msg) {
        case WM_ACTIVATE:
            if (LOWORD(w) == WA_INACTIVE) {
                w = WA_ACTIVE;
                l = 0;
                return true;
            }
            return false;
        case WM_ACTIVATEAPP:
            if (w == FALSE) {
                w = TRUE;
                l = 0;
                return true;
            }
            return false;
        case WM_NCACTIVATE:
            if (w == FALSE) {
                w = TRUE;
                return true;
            }
            return false;
        default: return false;
    }
}

LRESULT CALLBACK SubclassProc(HWND h, UINT msg, WPARAM w, LPARAM l) {
    WNDPROC orig = reinterpret_cast<WNDPROC>(GetPropW(h, kProp));
    if (msg == WM_KILLFOCUS) {
        Bump(C_ACTIVATE_SWALLOWED);
        return 0;
    }
    if (FixActivation(msg, w, l)) Bump(C_ACTIVATE_SWALLOWED);
    if (!orig) return DefWindowProcW(h, msg, w, l);
    LRESULT r = IsWindowUnicode(h) ? CallWindowProcW(orig, h, msg, w, l) : CallWindowProcA(orig, h, msg, w, l);
    if (msg == WM_NCDESTROY) RemovePropW(h, kProp);
    return r;
}

void Subclass(HWND h) {
    if (!h || GetPropW(h, kProp)) return;
    LONG_PTR old = IsWindowUnicode(h) ? SetWindowLongPtrW(h, GWLP_WNDPROC, reinterpret_cast<LONG_PTR>(&SubclassProc))
                                      : SetWindowLongPtrA(h, GWLP_WNDPROC, reinterpret_cast<LONG_PTR>(&SubclassProc));
    if (!old) return;
    SetPropW(h, kProp, reinterpret_cast<HANDLE>(old));
    Bump(C_WINDOWS_SUBCLASSED);
    // Tell the game it is active even if the system never activated it (it was created behind another copy).
    PostMessageW(h, WM_ACTIVATEAPP, TRUE, 0);
    PostMessageW(h, WM_ACTIVATE, WA_ACTIVE, 0);
    PostMessageW(h, WM_SETFOCUS, 0, 0);
}

bool WantsSubclass(DWORD style, HWND parent, bool is_ime) {
    if (is_ime) return false;
    if (style & WS_CHILD) return false;
    if (parent == HWND_MESSAGE) return false;
    return true;
}

HWND WINAPI d_CreateWindowExW(DWORD ex, LPCWSTR cls, LPCWSTR title, DWORD style, int x, int y, int w, int h, HWND parent,
                              HMENU menu, HINSTANCE inst, LPVOID param) {
    HWND r = o_CreateWindowExW(ex, cls, title, style, x, y, w, h, parent, menu, inst, param);
    if (r && WantsSubclass(style, parent, IsSystemHelperClass(r))) Subclass(r);
    return r;
}
HWND WINAPI d_CreateWindowExA(DWORD ex, LPCSTR cls, LPCSTR title, DWORD style, int x, int y, int w, int h, HWND parent,
                              HMENU menu, HINSTANCE inst, LPVOID param) {
    HWND r = o_CreateWindowExA(ex, cls, title, style, x, y, w, h, parent, menu, inst, param);
    if (r && WantsSubclass(style, parent, IsSystemHelperClass(r))) Subclass(r);
    return r;
}

#define HK(mod, name) InstallHook(mod, #name, reinterpret_cast<void*>(&d_##name), reinterpret_cast<void**>(&o_##name))

}  // namespace

bool FocusFixPosted(MSG* m) {
    if (!g_cfg.focus) return false;
    if (FixActivation(m->message, m->wParam, m->lParam)) {
        Bump(C_ACTIVATE_SWALLOWED);
        return true;
    }
    return false;
}

void InstallFocus() {
    if (!g_cfg.focus) return;
    HMODULE u = GetModuleHandleW(L"user32.dll");
    if (!u) u = LoadLibraryW(L"user32.dll");
    HK(u, GetForegroundWindow);
    HK(u, GetActiveWindow);
    HK(u, GetFocus);
    HK(u, SetForegroundWindow);
    HK(u, CreateWindowExW);
    HK(u, CreateWindowExA);
    g_features.focus = true;
}

}  // namespace cl
