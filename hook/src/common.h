// Shared state and helpers for the Couchlink input hook.
#pragma once
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

#include <atomic>
#include <cstdint>
#include <string>
#include <vector>

#include "MinHook.h"
#include "feed.h"
#include "json.h"
#include "kbmfeed.h"

namespace cl {

struct SavePair {
    std::wstring from;
    std::wstring to;
};

// A fake pad used by tests. When Config::use_test_pads is set the real XInput is never called;
// slot i is what "real pad index i" reports.
struct TestPad {
    bool present = false;
    uint16_t buttons = 0;
    uint8_t lt = 0, rt = 0;
    int16_t lx = 0, ly = 0, rx = 0, ry = 0;
    uint32_t packet = 1;
};

struct Config {
    bool xinput = true;
    int pad = -1;  // real pad index this copy owns, -1 = none
    bool focus = false;
    std::vector<std::wstring> locks;
    bool locks_all = false;
    std::wstring cow_root;  // normalised full path, empty = off
    std::vector<SavePair> saves;  // normalised, longest `from` first
    bool use_test_pads = false;
    TestPad test_pads[4];
    std::wstring feed;  // name of the pad feed section, empty = none
    std::wstring kbm_feed;  // name of the keyboard and mouse feed section, empty = none
    bool fake_cursor = false;  // draw an overlay cursor for the copy's fake mouse
    bool hook_children = false;  // inject this DLL into every process this process starts
    bool hide_pads = false;  // own_pads was given: hide every game controller not in it
    std::vector<std::wstring> own_pads;  // this copy's controllers, as normalised device paths (pads_hook.cpp)
    uint32_t copy_id = 0;  // pid of the process the host injected first (suffix of renamed locks); 0 = this process
    uint32_t depth = 0;  // 0 for the process the host injected, +1 per child generation
    Json raw;  // the config exactly as received, handed on to children
};

extern Config g_cfg;
// Mapped pad feed (null when no feed is configured).
extern volatile FeedBlock* g_feed;
// Mapped keyboard and mouse feed (null when none is configured).
extern volatile KbmHeader* g_kbm;

// Which hook groups ended up installed (reported in status).
struct Features {
    bool xinput = false, focus = false, names = false, cow = false, saves = false, feed = false, kbm = false, children = false,
         pads = false;
};
extern Features g_features;
// This DLL's own module handle.
HMODULE SelfModule();
// How this DLL got into the process: "inject" (a host loaded it) or "proxy" (the game loaded it as a stand-in DLL).
extern const char* g_load_mode;

enum Counter {
    C_XI_GET_STATE,
    C_XI_SET_STATE,
    C_XI_OTHER,
    C_XI_MAPPED,
    C_XI_BLOCKED,
    C_XI_FEED_READS,
    C_XI_FEED_RUMBLE,
    C_FOCUS_QUERY,
    C_ACTIVATE_SWALLOWED,
    C_WINDOWS_SUBCLASSED,
    C_NAMES_RENAMED,
    C_COW_BROKEN,
    C_COW_FAILED,
    C_PATH_REDIRECTED,
    C_KBM_EVENTS,
    C_KBM_KEY_MSGS,
    C_KBM_CHAR_MSGS,
    C_KBM_MOUSE_MSGS,
    C_KBM_RAW_MSGS,
    C_KBM_REAL_DROPPED,
    C_KBM_STATE_QUERIES,
    C_KBM_CURSOR_QUERIES,
    C_KBM_DINPUT_READS,
    C_KBM_RAW_READS,
    C_CHILDREN_HOOKED,
    C_CHILDREN_FAILED,
    C_CHILDREN_SKIPPED,
    C_PADS_HIDDEN,
    C_COUNT
};
void Bump(Counter c, uint64_t n = 1);

void Note(const std::string& text);
void NoteHookFailure(const char* what);
void SetXInputVibration(int real_index, uint16_t left, uint16_t right);
void AddXInputModule(const std::string& name);

std::string StatusJson();

std::wstring Widen(const std::string& utf8);
std::string Narrow(const std::wstring& wide);

// Absolute, backslash-separated, no trailing backslash, no \\?\ prefix. Empty on failure.
std::wstring NormalizePath(const wchar_t* path);
// True when `path` equals `prefix` or lies below it (component aware, case-insensitive).
bool PathUnder(const std::wstring& path, const std::wstring& prefix);

// Hook installation. Detours are queued; ApplyHooks() enables them in one batch.
bool InstallHook(HMODULE module, const char* name, void* detour, void** original);
bool InstallHookAddress(void* target, void* detour, void** original, const char* label);
void ApplyHooks();

// Feature installers (each is a no-op when its config part is empty).
void InstallXInput();
void InstallFocus();
void InstallNames();
void InstallFiles();
void InstallMessages();  // GetMessage / PeekMessage / TranslateMessage filters shared by focus and kbm
void InstallKbm();
void InstallKbmDInput();
void InstallChildren();
void InstallPads();
// Other players' controllers (pads_hook.cpp). True when this copy must not see the HID device at `path`.
bool PadPathHidden(const wchar_t* path);
// Same for a Raw Input device handle.
bool PadHandleHidden(HANDLE device);
// False for a WM_INPUT from a hidden controller.
bool PadsKeepMessage(const MSG* m);
// A module was loaded into the process (called by the LoadLibrary watch in xinput_hook.cpp).
void DInputModuleLoaded(HMODULE m, const wchar_t* path);
void PadsModuleLoaded(HMODULE m, const wchar_t* path);  // GameInput.dll, when pads are hidden

// The largest visible top-level window of this process; nullptr until there is one.
HWND FindMainWindow();
// Focus faking: rewrites a posted "you lost focus" message. True when something changed.
bool FocusFixPosted(MSG* m);
// Keyboard and mouse feed. Filter: false = drop the message (real input), true = keep (possibly rewritten).
bool KbmFilterMessage(MSG* m);
// True once for a WM_KEYDOWN / WM_SYSKEYDOWN the feed delivered (the hook posts the WM_CHAR itself).
bool KbmTakeFakeKeyDown(const MSG* m);
// Pulls pending feed events into the state and posts the resulting window messages.
void KbmDrain();

// Children: JSON status of the processes this process hooked, for StatusJson.
std::string ChildrenJson();
// Config line for a child of this process (same config, own copy id, one generation deeper).
std::string ChildConfigLine();
// Name suffix that renamed locks get: the copy id when set, else this process id.
uint32_t CopyId();

// Re-entrancy guard for hooks that call APIs which may themselves be hooked.
struct ReentryGuard {
    static thread_local int depth;
    ReentryGuard() { ++depth; }
    ~ReentryGuard() { --depth; }
    static bool active() { return depth > 0; }
};

}  // namespace cl
