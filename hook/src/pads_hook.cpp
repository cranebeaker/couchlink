// Only this copy's own controllers (`own_pads`).
//
// The XInput hook gives a copy its pad as XInput user 0, but many games also read controllers another way: Unity and
// SDL open the HID device itself, older games use DirectInput, others listen to Raw Input. Through those a copy would
// see every controller on the PC, so one pad would steer every copy. With `own_pads` set, every game controller
// (HID usage page 1, usage joystick, gamepad or multi-axis) whose device path is not in the list is hidden:
//
// - HID: CreateFileW/A and CreateFile2 on its device path fail with "not found" (files_hook.cpp asks PadPathHidden);
// - Raw Input: GetRawInputDeviceList leaves it out and its WM_INPUT messages are dropped (msg_hook.cpp);
// - DirectInput 8: EnumDevices skips it and CreateDevice refuses it (kbm_dinput.cpp).
//
// Keyboards, mice and every other kind of device are never touched. An empty list hides every controller (a seat that
// plays with keyboard and mouse). Same idea as ProtoInput's BlockRawInputHook, DinputDeviceHook and CreateSingleHIDHook
// (MIT, see THIRD_PARTY.md), with the selection made by device path so one list serves all three APIs.
#include <map>
#include <mutex>

#include "common.h"

namespace cl {
namespace {

std::mutex g_lock;
std::map<std::wstring, bool> g_path_is_pad;  // normalised path -> is a game controller
std::map<HANDLE, bool> g_handle_hidden;       // Raw Input device -> hidden

UINT(WINAPI* o_GetRawInputDeviceList)(PRAWINPUTDEVICELIST, PUINT, UINT) = nullptr;

// HID usage page 1 (generic desktop): 4 joystick, 5 gamepad, 8 multi-axis controller.
bool IsControllerUsage(USHORT page, USHORT usage) { return page == 1 && (usage == 4 || usage == 5 || usage == 8); }

// `\\?\HID#VID_054C&PID_0CE6&MI_03#7&1&0&0000#{4d1e55b2-...}` -> `hid#vid_054c&pid_0ce6&mi_03#7&1&0&0000`: the device
// part, without the prefix and the interface class, so paths from SDL, Raw Input, DirectInput and SetupAPI compare equal.
std::wstring NormPad(const wchar_t* path) {
    std::wstring s(path);
    for (auto& ch : s) ch = static_cast<wchar_t>(towlower(ch));
    if (s.rfind(L"\\\\?\\", 0) == 0 || s.rfind(L"\\\\.\\", 0) == 0) s = s.substr(4);
    size_t g = s.rfind(L"#{");
    if (g != std::wstring::npos) s.resize(g);
    return s;
}

bool IsOwn(const std::wstring& norm) {
    for (const auto& own : g_cfg.own_pads)
        if (own == norm) return true;
    return false;
}

// hid.dll is loaded on first use, never while the loader lock is held (opens only happen from game code).
struct HidApi {
    BOOLEAN(WINAPI* get_preparsed)(HANDLE, void**) = nullptr;
    BOOLEAN(WINAPI* free_preparsed)(void*) = nullptr;
    LONG(WINAPI* get_caps)(void*, void*) = nullptr;
    bool loaded = false;
};
HidApi& Hid() {
    static HidApi api;
    static std::once_flag once;
    std::call_once(once, [] {
        HMODULE m = LoadLibraryW(L"hid.dll");
        if (!m) return;
        api.get_preparsed = reinterpret_cast<BOOLEAN(WINAPI*)(HANDLE, void**)>(GetProcAddress(m, "HidD_GetPreparsedData"));
        api.free_preparsed = reinterpret_cast<BOOLEAN(WINAPI*)(void*)>(GetProcAddress(m, "HidD_FreePreparsedData"));
        api.get_caps = reinterpret_cast<LONG(WINAPI*)(void*, void*)>(GetProcAddress(m, "HidP_GetCaps"));
        api.loaded = api.get_preparsed && api.free_preparsed && api.get_caps;
    });
    return api;
}

// HIDP_CAPS starts with Usage, UsagePage (USHORT each); the rest is not needed.
struct HidCapsHead {
    USHORT usage;
    USHORT usage_page;
    USHORT rest[62];
};

// True when the HID device at `path` is a game controller. Opens it without access rights (allowed for any HID).
bool HidIsController(const wchar_t* path) {
    HidApi& h = Hid();
    if (!h.loaded) return false;
    HANDLE f;
    {
        ReentryGuard guard;  // our own CreateFile detours let this through
        f = CreateFileW(path, 0, FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, OPEN_EXISTING, 0, nullptr);
    }
    if (f == INVALID_HANDLE_VALUE) return false;
    bool pad = false;
    void* pp = nullptr;
    if (h.get_preparsed(f, &pp) && pp) {
        HidCapsHead caps = {};
        if (h.get_caps(pp, &caps) == 0x00110000 /* HIDP_STATUS_SUCCESS */) pad = IsControllerUsage(caps.usage_page, caps.usage);
        h.free_preparsed(pp);
    }
    CloseHandle(f);
    return pad;
}

bool RawIsController(HANDLE dev) {
    RID_DEVICE_INFO info = {};
    info.cbSize = sizeof(info);
    UINT size = sizeof(info);
    if (GetRawInputDeviceInfoW(dev, RIDI_DEVICEINFO, &info, &size) == static_cast<UINT>(-1)) return false;
    return info.dwType == RIM_TYPEHID && IsControllerUsage(info.hid.usUsagePage, info.hid.usUsage);
}

std::wstring RawName(HANDLE dev) {
    UINT chars = 0;
    if (GetRawInputDeviceInfoW(dev, RIDI_DEVICENAME, nullptr, &chars) != 0 || chars == 0) return std::wstring();
    std::wstring name(chars, L'\0');
    if (GetRawInputDeviceInfoW(dev, RIDI_DEVICENAME, &name[0], &chars) == static_cast<UINT>(-1)) return std::wstring();
    name.resize(wcslen(name.c_str()));
    return name;
}

UINT WINAPI d_GetRawInputDeviceList(PRAWINPUTDEVICELIST list, PUINT num, UINT cb) {
    if (!num || cb != sizeof(RAWINPUTDEVICELIST)) return o_GetRawInputDeviceList(list, num, cb);
    UINT n = 0;
    if (o_GetRawInputDeviceList(nullptr, &n, cb) == static_cast<UINT>(-1)) return o_GetRawInputDeviceList(list, num, cb);
    std::vector<RAWINPUTDEVICELIST> all(n + 8);
    UINT cap = static_cast<UINT>(all.size());
    UINT got = o_GetRawInputDeviceList(all.data(), &cap, cb);
    if (got == static_cast<UINT>(-1)) return o_GetRawInputDeviceList(list, num, cb);
    all.resize(got);
    std::vector<RAWINPUTDEVICELIST> kept;
    for (const auto& d : all) {
        if (d.dwType == RIM_TYPEHID && PadHandleHidden(d.hDevice)) {
            Bump(C_PADS_HIDDEN);
            continue;
        }
        kept.push_back(d);
    }
    UINT k = static_cast<UINT>(kept.size());
    if (!list) {
        *num = k;
        return 0;
    }
    if (*num < k) {
        *num = k;
        SetLastError(ERROR_INSUFFICIENT_BUFFER);
        return static_cast<UINT>(-1);
    }
    memcpy(list, kept.data(), k * sizeof(RAWINPUTDEVICELIST));
    return k;
}

}  // namespace

bool PadPathHidden(const wchar_t* path) {
    if (!g_cfg.hide_pads || !path || ReentryGuard::active()) return false;
    // Fast way out for every ordinary file the game opens: only `\\?\hid#...` and `\\.\hid#...` are devices.
    const wchar_t* p = path;
    if (p[0] == L'\\' && p[1] == L'\\' && (p[2] == L'?' || p[2] == L'.') && p[3] == L'\\') p += 4;
    if (towlower(p[0]) != L'h' || towlower(p[1]) != L'i' || towlower(p[2]) != L'd' || p[3] != L'#') return false;
    std::wstring norm = NormPad(path);
    if (norm.rfind(L"hid#", 0) != 0 || IsOwn(norm)) return false;
    {
        std::lock_guard<std::mutex> l(g_lock);
        auto it = g_path_is_pad.find(norm);
        if (it != g_path_is_pad.end()) return it->second;
    }
    bool pad = HidIsController(path);
    std::lock_guard<std::mutex> l(g_lock);
    g_path_is_pad[norm] = pad;
    return pad;
}

bool PadHandleHidden(HANDLE dev) {
    if (!g_cfg.hide_pads || !dev) return false;
    {
        std::lock_guard<std::mutex> l(g_lock);
        auto it = g_handle_hidden.find(dev);
        if (it != g_handle_hidden.end()) return it->second;
    }
    bool hidden = false;
    if (RawIsController(dev)) {
        std::wstring name = RawName(dev);
        hidden = name.empty() || !IsOwn(NormPad(name.c_str()));
    }
    std::lock_guard<std::mutex> l(g_lock);
    g_handle_hidden[dev] = hidden;
    return hidden;
}

bool PadsKeepMessage(const MSG* m) {
    if (!g_cfg.hide_pads || !m || m->message != WM_INPUT) return true;
    RAWINPUTHEADER head = {};
    UINT size = sizeof(head);
    if (GetRawInputData(reinterpret_cast<HRAWINPUT>(m->lParam), RID_HEADER, &head, &size, sizeof(RAWINPUTHEADER)) == static_cast<UINT>(-1))
        return true;
    if (head.dwType != RIM_TYPEHID || !PadHandleHidden(head.hDevice)) return true;
    Bump(C_PADS_HIDDEN);
    return false;
}

void InstallPads() {
    if (!g_cfg.hide_pads) return;
    HMODULE u = GetModuleHandleW(L"user32.dll");
    if (!u) u = LoadLibraryW(L"user32.dll");
    if (InstallHook(u, "GetRawInputDeviceList", reinterpret_cast<void*>(&d_GetRawInputDeviceList),
                    reinterpret_cast<void**>(&o_GetRawInputDeviceList)))
        g_features.pads = true;
    // DirectInput 8 (now or once dinput8.dll is loaded); a no-op when the keyboard and mouse feed hooked it already.
    InstallKbmDInput();
}

}  // namespace cl
