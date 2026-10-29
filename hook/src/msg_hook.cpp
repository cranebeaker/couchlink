// Message queue filters shared by the focus faking and the keyboard / mouse feed.
//
// Everything a copy reads from its queue goes through GetMessage / PeekMessage: the focus code turns posted
// "inactive" messages into "active", the kbm code drops real keyboard and mouse messages (only the ones the hook
// posted itself, carrying a signature, get through) and TranslateMessage skips the keys the hook already
// translated. Same filter idea as ProtoInput's MessageFilterHook (MIT, see THIRD_PARTY.md); dropped messages are
// skipped here instead of being returned as WM_NULL.
#include "common.h"

namespace cl {
namespace {

BOOL(WINAPI* o_GetMessageW)(LPMSG, HWND, UINT, UINT) = nullptr;
BOOL(WINAPI* o_GetMessageA)(LPMSG, HWND, UINT, UINT) = nullptr;
BOOL(WINAPI* o_PeekMessageW)(LPMSG, HWND, UINT, UINT, UINT) = nullptr;
BOOL(WINAPI* o_PeekMessageA)(LPMSG, HWND, UINT, UINT, UINT) = nullptr;
BOOL(WINAPI* o_TranslateMessage)(const MSG*) = nullptr;

// True = deliver the message to the game.
bool Keep(MSG* m) {
    if (!PadsKeepMessage(m)) return false;
    FocusFixPosted(m);
    if (g_kbm) return KbmFilterMessage(m);
    return true;
}

BOOL WINAPI d_GetMessageW(LPMSG m, HWND h, UINT a, UINT b) {
    for (;;) {
        BOOL r = o_GetMessageW(m, h, a, b);
        if (r <= 0 || Keep(m)) return r;
    }
}
BOOL WINAPI d_GetMessageA(LPMSG m, HWND h, UINT a, UINT b) {
    for (;;) {
        BOOL r = o_GetMessageA(m, h, a, b);
        if (r <= 0 || Keep(m)) return r;
    }
}
BOOL WINAPI d_PeekMessageW(LPMSG m, HWND h, UINT a, UINT b, UINT rm) {
    for (;;) {
        BOOL r = o_PeekMessageW(m, h, a, b, rm);
        if (!r || Keep(m)) return r;
        if (!(rm & PM_REMOVE)) {
            // A dropped message at the head of the queue would hide everything behind it: take it out.
            MSG junk;
            o_PeekMessageW(&junk, h, a, b, (rm & ~static_cast<UINT>(PM_REMOVE)) | PM_REMOVE);
        }
    }
}
BOOL WINAPI d_PeekMessageA(LPMSG m, HWND h, UINT a, UINT b, UINT rm) {
    for (;;) {
        BOOL r = o_PeekMessageA(m, h, a, b, rm);
        if (!r || Keep(m)) return r;
        if (!(rm & PM_REMOVE)) {
            MSG junk;
            o_PeekMessageA(&junk, h, a, b, (rm & ~static_cast<UINT>(PM_REMOVE)) | PM_REMOVE);
        }
    }
}

BOOL WINAPI d_TranslateMessage(const MSG* m) {
    if (g_kbm && m && KbmTakeFakeKeyDown(m)) return TRUE;
    return o_TranslateMessage(m);
}

}  // namespace

#define HK(mod, name) InstallHook(mod, #name, reinterpret_cast<void*>(&d_##name), reinterpret_cast<void**>(&o_##name))

void InstallMessages() {
    if (!g_cfg.focus && g_cfg.kbm_feed.empty() && !g_cfg.hide_pads) return;
    HMODULE u = GetModuleHandleW(L"user32.dll");
    if (!u) u = LoadLibraryW(L"user32.dll");
    HK(u, GetMessageW);
    HK(u, GetMessageA);
    HK(u, PeekMessageW);
    HK(u, PeekMessageA);
    if (!g_cfg.kbm_feed.empty()) HK(u, TranslateMessage);
}

}  // namespace cl
