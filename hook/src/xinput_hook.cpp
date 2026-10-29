// XInput hooks: the copy sees its assigned pad as user index 0 and every other index as disconnected.
//
// Every xinput DLL the process has (system ones, or a copy shipped in the game folder) gets its own "slot" so the
// detours know which original to call. DLLs that load later are picked up through LoadLibrary hooks.
// Approach after ProtoInput's XinputHook (MIT, see THIRD_PARTY.md), reimplemented on MinHook.
#include "common.h"

#include <tlhelp32.h>
#include <xinput.h>

#include <cwchar>
#include <set>

namespace cl {
namespace {

constexpr DWORD kAnyUser = 0xFF;  // XUSER_INDEX_ANY
constexpr int kMaxSlots = 4;

struct Slot {
    HMODULE mod = nullptr;
    DWORD(WINAPI* getState)(DWORD, XINPUT_STATE*) = nullptr;
    DWORD(WINAPI* getStateEx)(DWORD, XINPUT_STATE*) = nullptr;
    DWORD(WINAPI* setState)(DWORD, XINPUT_VIBRATION*) = nullptr;
    DWORD(WINAPI* getCaps)(DWORD, DWORD, XINPUT_CAPABILITIES*) = nullptr;
    DWORD(WINAPI* getBattery)(DWORD, BYTE, XINPUT_BATTERY_INFORMATION*) = nullptr;
    DWORD(WINAPI* getKeystroke)(DWORD, DWORD, XINPUT_KEYSTROKE*) = nullptr;
    DWORD(WINAPI* getAudio)(DWORD, LPWSTR, UINT*, LPWSTR, UINT*) = nullptr;
    DWORD(WINAPI* getDSound)(DWORD, GUID*, GUID*) = nullptr;
    DWORD(WINAPI* baseBus)(DWORD, void*) = nullptr;
    DWORD(WINAPI* capsEx)(DWORD, DWORD, DWORD, void*) = nullptr;
};

Slot g_slots[kMaxSlots];
int g_slot_count = 0;
CRITICAL_SECTION g_lock;
INIT_ONCE g_once = INIT_ONCE_STATIC_INIT;
std::set<HMODULE> g_seen;
std::set<void*> g_targets;

BOOL CALLBACK InitLock(PINIT_ONCE, PVOID, PVOID*) {
    InitializeCriticalSection(&g_lock);
    return TRUE;
}
struct Lock {
    Lock() {
        InitOnceExecuteOnce(&g_once, InitLock, nullptr, nullptr);
        EnterCriticalSection(&g_lock);
    }
    ~Lock() { LeaveCriticalSection(&g_lock); }
};

constexpr int kFeedPad = 100;  // MapIndex result for "the pad feed"
constexpr uint16_t kGuideButton = 0x0400;

// Returns the pad the game's user index refers to: a real index 0..3, kFeedPad, or -1 for "disconnected".
int MapIndex(DWORD user) {
    if (user == 0 || user == kAnyUser) return g_feed ? kFeedPad : g_cfg.pad;
    return -1;
}

// True when the pad's data comes from the feed or the test pads instead of the real driver.
bool UseFake(int real) { return real == kFeedPad || g_cfg.use_test_pads; }

// Reads the fake pad: from the feed (seqlock) or the configured test pad. False = disconnected.
bool FakePad(int real, TestPad& t) {
    if (real != kFeedPad) {
        t = g_cfg.test_pads[real];
        return t.present;
    }
    volatile FeedBlock* f = g_feed;
    static TestPad last;  // last consistent read, used if the writer is busy for too long
    for (int tries = 0; tries < 64; ++tries) {
        uint32_t s1 = f->seq;
        if (s1 & 1) {
            YieldProcessor();
            continue;
        }
        MemoryBarrier();
        TestPad c;
        c.packet = f->packet;
        c.buttons = f->buttons;
        c.lt = f->lt;
        c.rt = f->rt;
        c.lx = f->lx;
        c.ly = f->ly;
        c.rx = f->rx;
        c.ry = f->ry;
        c.present = (f->flags & kFeedFlagConnected) != 0;
        MemoryBarrier();
        if (f->seq == s1) {
            last = c;
            break;
        }
    }
    t = last;
    f->game_beat = GetTickCount();
    Bump(C_XI_FEED_READS);
    return t.present;
}

DWORD FakeState(int real, XINPUT_STATE* st, bool ex) {
    if (!st) return ERROR_BAD_ARGUMENTS;
    TestPad t;
    if (!FakePad(real, t)) return ERROR_DEVICE_NOT_CONNECTED;
    ZeroMemory(st, sizeof(*st));
    st->dwPacketNumber = t.packet;
    st->Gamepad.wButtons = ex ? t.buttons : static_cast<WORD>(t.buttons & ~kGuideButton);
    st->Gamepad.bLeftTrigger = t.lt;
    st->Gamepad.bRightTrigger = t.rt;
    st->Gamepad.sThumbLX = t.lx;
    st->Gamepad.sThumbLY = t.ly;
    st->Gamepad.sThumbRX = t.rx;
    st->Gamepad.sThumbRY = t.ry;
    return ERROR_SUCCESS;
}

bool FakeConnected(int real) {
    TestPad t;
    return FakePad(real, t);
}

DWORD DoGetState(DWORD (WINAPI* orig)(DWORD, XINPUT_STATE*), DWORD user, XINPUT_STATE* st, bool ex) {
    Bump(C_XI_GET_STATE);
    int real = MapIndex(user);
    if (real < 0) {
        Bump(C_XI_BLOCKED);
        return ERROR_DEVICE_NOT_CONNECTED;
    }
    Bump(C_XI_MAPPED);
    if (UseFake(real)) return FakeState(real, st, ex);
    return orig ? orig(static_cast<DWORD>(real), st) : ERROR_DEVICE_NOT_CONNECTED;
}

DWORD DoSetState(DWORD (WINAPI* orig)(DWORD, XINPUT_VIBRATION*), DWORD user, XINPUT_VIBRATION* vib) {
    Bump(C_XI_SET_STATE);
    int real = MapIndex(user);
    if (real < 0) {
        Bump(C_XI_BLOCKED);
        return ERROR_DEVICE_NOT_CONNECTED;
    }
    Bump(C_XI_MAPPED);
    if (UseFake(real)) {
        if (!vib) return ERROR_BAD_ARGUMENTS;
        if (real == kFeedPad) {
            volatile FeedBlock* f = g_feed;
            if (!(f->flags & kFeedFlagConnected)) return ERROR_DEVICE_NOT_CONNECTED;
            // Games repeat the same value every frame; only a change is worth waking the daemon for.
            if (f->rumble_left != vib->wLeftMotorSpeed || f->rumble_right != vib->wRightMotorSpeed) {
                f->rumble_left = vib->wLeftMotorSpeed;
                f->rumble_right = vib->wRightMotorSpeed;
                MemoryBarrier();
                InterlockedIncrement(reinterpret_cast<volatile LONG*>(&f->rumble_seq));
                Bump(C_XI_FEED_RUMBLE);
            }
            return ERROR_SUCCESS;
        }
        if (!g_cfg.test_pads[real].present) return ERROR_DEVICE_NOT_CONNECTED;
        SetXInputVibration(real, vib->wLeftMotorSpeed, vib->wRightMotorSpeed);
        return ERROR_SUCCESS;
    }
    return orig ? orig(static_cast<DWORD>(real), vib) : ERROR_DEVICE_NOT_CONNECTED;
}

DWORD DoGetCaps(DWORD (WINAPI* orig)(DWORD, DWORD, XINPUT_CAPABILITIES*), DWORD user, DWORD flags,
                XINPUT_CAPABILITIES* caps) {
    Bump(C_XI_OTHER);
    int real = MapIndex(user);
    if (real < 0) {
        Bump(C_XI_BLOCKED);
        return ERROR_DEVICE_NOT_CONNECTED;
    }
    if (UseFake(real)) {
        if (!FakeConnected(real)) return ERROR_DEVICE_NOT_CONNECTED;
        if (!caps) return ERROR_BAD_ARGUMENTS;
        ZeroMemory(caps, sizeof(*caps));
        caps->Type = XINPUT_DEVTYPE_GAMEPAD;
        caps->SubType = XINPUT_DEVSUBTYPE_GAMEPAD;
        caps->Gamepad.wButtons = 0xF3FF;
        caps->Gamepad.bLeftTrigger = 255;
        caps->Gamepad.bRightTrigger = 255;
        caps->Gamepad.sThumbLX = caps->Gamepad.sThumbLY = caps->Gamepad.sThumbRX = caps->Gamepad.sThumbRY = -64;
        caps->Vibration.wLeftMotorSpeed = caps->Vibration.wRightMotorSpeed = 255;
        return ERROR_SUCCESS;
    }
    return orig ? orig(static_cast<DWORD>(real), flags, caps) : ERROR_DEVICE_NOT_CONNECTED;
}

DWORD DoGetBattery(DWORD (WINAPI* orig)(DWORD, BYTE, XINPUT_BATTERY_INFORMATION*), DWORD user, BYTE type,
                   XINPUT_BATTERY_INFORMATION* info) {
    Bump(C_XI_OTHER);
    int real = MapIndex(user);
    if (real < 0) {
        Bump(C_XI_BLOCKED);
        return ERROR_DEVICE_NOT_CONNECTED;
    }
    if (UseFake(real)) {
        if (!FakeConnected(real)) return ERROR_DEVICE_NOT_CONNECTED;
        if (!info) return ERROR_BAD_ARGUMENTS;
        info->BatteryType = BATTERY_TYPE_WIRED;
        info->BatteryLevel = BATTERY_LEVEL_FULL;
        return ERROR_SUCCESS;
    }
    return orig ? orig(static_cast<DWORD>(real), type, info) : ERROR_DEVICE_NOT_CONNECTED;
}

DWORD DoGetKeystroke(DWORD (WINAPI* orig)(DWORD, DWORD, XINPUT_KEYSTROKE*), DWORD user, DWORD reserved,
                     XINPUT_KEYSTROKE* ks) {
    Bump(C_XI_OTHER);
    int real = MapIndex(user);
    if (real < 0) {
        Bump(C_XI_BLOCKED);
        return ERROR_DEVICE_NOT_CONNECTED;
    }
    if (UseFake(real)) return FakeConnected(real) ? ERROR_EMPTY : ERROR_DEVICE_NOT_CONNECTED;
    // Keystrokes of "any" pad must not leak other pads: ask for our one pad explicitly.
    return orig ? orig(static_cast<DWORD>(real), reserved, ks) : ERROR_DEVICE_NOT_CONNECTED;
}

template <int S>
struct Detours {
    static DWORD WINAPI GetState(DWORD u, XINPUT_STATE* s) { return DoGetState(g_slots[S].getState, u, s, false); }
    static DWORD WINAPI GetStateEx(DWORD u, XINPUT_STATE* s) { return DoGetState(g_slots[S].getStateEx, u, s, true); }
    static DWORD WINAPI SetState(DWORD u, XINPUT_VIBRATION* v) { return DoSetState(g_slots[S].setState, u, v); }
    static DWORD WINAPI GetCaps(DWORD u, DWORD f, XINPUT_CAPABILITIES* c) {
        return DoGetCaps(g_slots[S].getCaps, u, f, c);
    }
    static DWORD WINAPI GetBattery(DWORD u, BYTE t, XINPUT_BATTERY_INFORMATION* i) {
        return DoGetBattery(g_slots[S].getBattery, u, t, i);
    }
    static DWORD WINAPI GetKeystroke(DWORD u, DWORD r, XINPUT_KEYSTROKE* k) {
        return DoGetKeystroke(g_slots[S].getKeystroke, u, r, k);
    }
    static DWORD WINAPI GetAudio(DWORD u, LPWSTR rid, UINT* rc, LPWSTR cid, UINT* cc) {
        Bump(C_XI_OTHER);
        int real = MapIndex(u);
        if (real < 0) {
            Bump(C_XI_BLOCKED);
            return ERROR_DEVICE_NOT_CONNECTED;
        }
        if (UseFake(real)) {
            if (!FakeConnected(real)) return ERROR_DEVICE_NOT_CONNECTED;
            if (rc) *rc = 0;
            if (cc) *cc = 0;
            return ERROR_SUCCESS;
        }
        auto o = g_slots[S].getAudio;
        return o ? o(static_cast<DWORD>(real), rid, rc, cid, cc) : ERROR_DEVICE_NOT_CONNECTED;
    }
    static DWORD WINAPI GetDSound(DWORD u, GUID* r, GUID* c) {
        Bump(C_XI_OTHER);
        int real = MapIndex(u);
        if (real < 0) {
            Bump(C_XI_BLOCKED);
            return ERROR_DEVICE_NOT_CONNECTED;
        }
        if (UseFake(real)) {
            if (!FakeConnected(real)) return ERROR_DEVICE_NOT_CONNECTED;
            if (r) ZeroMemory(r, sizeof(*r));
            if (c) ZeroMemory(c, sizeof(*c));
            return ERROR_SUCCESS;
        }
        auto o = g_slots[S].getDSound;
        return o ? o(static_cast<DWORD>(real), r, c) : ERROR_DEVICE_NOT_CONNECTED;
    }
    static DWORD WINAPI BaseBus(DWORD u, void* p) {
        Bump(C_XI_OTHER);
        int real = MapIndex(u);
        if (real < 0) {
            Bump(C_XI_BLOCKED);
            return ERROR_DEVICE_NOT_CONNECTED;
        }
        if (UseFake(real)) return ERROR_NOT_SUPPORTED;
        auto o = g_slots[S].baseBus;
        return o ? o(static_cast<DWORD>(real), p) : ERROR_DEVICE_NOT_CONNECTED;
    }
    static DWORD WINAPI CapsEx(DWORD unk, DWORD u, DWORD f, void* p) {
        Bump(C_XI_OTHER);
        int real = MapIndex(u);
        if (real < 0) {
            Bump(C_XI_BLOCKED);
            return ERROR_DEVICE_NOT_CONNECTED;
        }
        if (UseFake(real)) return ERROR_NOT_SUPPORTED;
        auto o = g_slots[S].capsEx;
        return o ? o(unk, static_cast<DWORD>(real), f, p) : ERROR_DEVICE_NOT_CONNECTED;
    }
};

struct DetourSet {
    void *getState, *getStateEx, *setState, *getCaps, *getBattery, *getKeystroke, *getAudio, *getDSound, *baseBus,
        *capsEx;
};

template <int S>
DetourSet MakeSet() {
    return DetourSet{reinterpret_cast<void*>(&Detours<S>::GetState),    reinterpret_cast<void*>(&Detours<S>::GetStateEx),
                     reinterpret_cast<void*>(&Detours<S>::SetState),    reinterpret_cast<void*>(&Detours<S>::GetCaps),
                     reinterpret_cast<void*>(&Detours<S>::GetBattery),  reinterpret_cast<void*>(&Detours<S>::GetKeystroke),
                     reinterpret_cast<void*>(&Detours<S>::GetAudio),    reinterpret_cast<void*>(&Detours<S>::GetDSound),
                     reinterpret_cast<void*>(&Detours<S>::BaseBus),     reinterpret_cast<void*>(&Detours<S>::CapsEx)};
}

const DetourSet kSets[kMaxSlots] = {MakeSet<0>(), MakeSet<1>(), MakeSet<2>(), MakeSet<3>()};

bool HookTarget(void* target, void* detour, void** orig, const char* label) {
    if (!target) return false;
    if (g_targets.count(target)) return false;  // forwarded export already hooked through another slot
    if (!InstallHookAddress(target, detour, orig, label)) return false;
    g_targets.insert(target);
    return true;
}

bool IsXInputName(const wchar_t* path, std::string* base_out) {
    const wchar_t* base = wcsrchr(path, L'\\');
    base = base ? base + 1 : path;
    static const wchar_t* const names[] = {L"xinput1_1.dll", L"xinput1_2.dll", L"xinput1_3.dll", L"xinput1_4.dll",
                                           L"xinput9_1_0.dll"};
    for (const wchar_t* n : names) {
        if (_wcsicmp(base, n) == 0) {
            if (base_out) *base_out = Narrow(base);
            return true;
        }
    }
    return false;
}

// Caller holds the lock. Returns true when new hooks were queued.
bool HookModuleLocked(HMODULE m, const std::string& name) {
    if (g_slot_count >= kMaxSlots) {
        Note("xinput: no free slot for " + name);
        return false;
    }
    int s = g_slot_count;
    Slot& sl = g_slots[s];
    const DetourSet& d = kSets[s];
    sl.mod = m;
    bool any = false;
    any |= HookTarget(GetProcAddress(m, "XInputGetState"), d.getState, reinterpret_cast<void**>(&sl.getState), "XInputGetState");
    any |= HookTarget(GetProcAddress(m, reinterpret_cast<LPCSTR>(100)), d.getStateEx, reinterpret_cast<void**>(&sl.getStateEx),
                      "XInputGetStateEx");
    any |= HookTarget(GetProcAddress(m, "XInputSetState"), d.setState, reinterpret_cast<void**>(&sl.setState), "XInputSetState");
    any |= HookTarget(GetProcAddress(m, "XInputGetCapabilities"), d.getCaps, reinterpret_cast<void**>(&sl.getCaps),
                      "XInputGetCapabilities");
    any |= HookTarget(GetProcAddress(m, "XInputGetBatteryInformation"), d.getBattery, reinterpret_cast<void**>(&sl.getBattery),
                      "XInputGetBatteryInformation");
    any |= HookTarget(GetProcAddress(m, "XInputGetKeystroke"), d.getKeystroke, reinterpret_cast<void**>(&sl.getKeystroke),
                      "XInputGetKeystroke");
    any |= HookTarget(GetProcAddress(m, "XInputGetAudioDeviceIds"), d.getAudio, reinterpret_cast<void**>(&sl.getAudio),
                      "XInputGetAudioDeviceIds");
    any |= HookTarget(GetProcAddress(m, "XInputGetDSoundAudioDeviceGuids"), d.getDSound, reinterpret_cast<void**>(&sl.getDSound),
                      "XInputGetDSoundAudioDeviceGuids");
    // Undocumented ordinals; only the xinput1_3 / 1_4 families have them with these signatures.
    if (_stricmp(name.c_str(), "xinput1_3.dll") == 0 || _stricmp(name.c_str(), "xinput1_4.dll") == 0) {
        any |= HookTarget(GetProcAddress(m, reinterpret_cast<LPCSTR>(104)), d.baseBus, reinterpret_cast<void**>(&sl.baseBus),
                          "XInputGetBaseBusInformation");
        any |= HookTarget(GetProcAddress(m, reinterpret_cast<LPCSTR>(108)), d.capsEx, reinterpret_cast<void**>(&sl.capsEx),
                          "XInputGetCapabilitiesEx");
    }
    if (any) {
        ++g_slot_count;
        AddXInputModule(name);
    }
    return any;
}

void MaybeHookLoaded(HMODULE m) {
    if (!m || (reinterpret_cast<uintptr_t>(m) & 3)) return;  // data file mappings
    if (m == SelfModule()) return;  // a stand-in DLL only jumps to the real one, which is what gets hooked
    wchar_t path[MAX_PATH * 2];
    {
        Lock l;
        if (!g_seen.insert(m).second) return;
    }
    DWORD n = GetModuleFileNameW(m, path, ARRAYSIZE(path));
    if (n == 0 || n >= ARRAYSIZE(path)) return;
    DInputModuleLoaded(m, path);  // no-op without a keyboard / mouse feed or pad hiding
    if (!g_cfg.xinput) return;
    std::string name;
    if (!IsXInputName(path, &name)) return;
    bool added;
    {
        Lock l;
        added = HookModuleLocked(m, name);
    }
    if (added) ApplyHooks();
}

HMODULE(WINAPI* o_LoadLibraryExW)(LPCWSTR, HANDLE, DWORD) = nullptr;
HMODULE(WINAPI* o_LoadLibraryExA)(LPCSTR, HANDLE, DWORD) = nullptr;
HMODULE(WINAPI* o_LoadLibraryW)(LPCWSTR) = nullptr;
HMODULE(WINAPI* o_LoadLibraryA)(LPCSTR) = nullptr;

constexpr DWORD kDataOnly = LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE | LOAD_LIBRARY_AS_DATAFILE_EXCLUSIVE;

HMODULE WINAPI d_LoadLibraryExW(LPCWSTR n, HANDLE f, DWORD fl) {
    HMODULE m = o_LoadLibraryExW(n, f, fl);
    if (m && !(fl & kDataOnly)) MaybeHookLoaded(m);
    return m;
}
HMODULE WINAPI d_LoadLibraryExA(LPCSTR n, HANDLE f, DWORD fl) {
    HMODULE m = o_LoadLibraryExA(n, f, fl);
    if (m && !(fl & kDataOnly)) MaybeHookLoaded(m);
    return m;
}
HMODULE WINAPI d_LoadLibraryW(LPCWSTR n) {
    HMODULE m = o_LoadLibraryW(n);
    if (m) MaybeHookLoaded(m);
    return m;
}
HMODULE WINAPI d_LoadLibraryA(LPCSTR n) {
    HMODULE m = o_LoadLibraryA(n);
    if (m) MaybeHookLoaded(m);
    return m;
}

}  // namespace

void InstallXInput() {
    // The module watch below also serves the DirectInput hook of the keyboard / mouse feed.
    if (!g_cfg.xinput && g_cfg.kbm_feed.empty() && !g_cfg.hide_pads) return;
    // Modules already in the process (static imports, DLLs next to the exe).
    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, GetCurrentProcessId());
    if (snap != INVALID_HANDLE_VALUE) {
        MODULEENTRY32W me;
        me.dwSize = sizeof(me);
        for (BOOL ok = Module32FirstW(snap, &me); ok; ok = Module32NextW(snap, &me)) MaybeHookLoaded(me.hModule);
        CloseHandle(snap);
    } else {
        Note("xinput: module snapshot failed");
    }
    // Modules loaded later.
    HMODULE kb = GetModuleHandleW(L"kernelbase.dll");
    if (!kb) kb = GetModuleHandleW(L"kernel32.dll");
    InstallHook(kb, "LoadLibraryExW", reinterpret_cast<void*>(&d_LoadLibraryExW), reinterpret_cast<void**>(&o_LoadLibraryExW));
    InstallHook(kb, "LoadLibraryExA", reinterpret_cast<void*>(&d_LoadLibraryExA), reinterpret_cast<void**>(&o_LoadLibraryExA));
    InstallHook(kb, "LoadLibraryW", reinterpret_cast<void*>(&d_LoadLibraryW), reinterpret_cast<void**>(&o_LoadLibraryW));
    InstallHook(kb, "LoadLibraryA", reinterpret_cast<void*>(&d_LoadLibraryA), reinterpret_cast<void**>(&o_LoadLibraryA));
    g_features.xinput = g_cfg.xinput;
}

}  // namespace cl
