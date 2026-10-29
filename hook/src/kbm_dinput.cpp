// DirectInput keyboard and mouse from the feed.
//
// A game that reads keys and mouse movement through DirectInput 8 (IDirectInputDevice8::GetDeviceState /
// GetDeviceData) bypasses window messages, so those two calls are answered from the feed state for keyboard and mouse
// devices. How: DirectInput8Create is hooked; the vtable of the IDirectInput8 object it returns gives CreateDevice;
// every device CreateDevice makes is classified (keyboard or mouse) and remembered; the functions behind
// GetDeviceState, GetDeviceData, Acquire, Unacquire, Poll and Release (found in the device's vtable) are hooked once
// and answer for the remembered devices only. Joysticks and everything else pass through. The real keyboard and
// mouse are never acquired, so the copy cannot read the desktop's devices and exclusive access cannot fail.
// ProtoInput hooks the same DirectInput entry points (MIT, see THIRD_PARTY.md).
#define DIRECTINPUT_VERSION 0x0800
#include <dinput.h>

#include <map>

#include "kbm_internal.h"

namespace cl {
namespace {

enum DevType { kKeyboard = 1, kMouse = 2 };

CRITICAL_SECTION g_lock;
INIT_ONCE g_lock_once = INIT_ONCE_STATIC_INIT;
BOOL CALLBACK InitLock(PINIT_ONCE, PVOID, PVOID*) {
    InitializeCriticalSection(&g_lock);
    return TRUE;
}
struct Lock {
    Lock() {
        InitOnceExecuteOnce(&g_lock_once, InitLock, nullptr, nullptr);
        EnterCriticalSection(&g_lock);
    }
    ~Lock() { LeaveCriticalSection(&g_lock); }
};

std::map<void*, int> g_devices;  // IDirectInputDevice8 pointer -> DevType
std::map<void*, void*> g_state_tramp, g_data_tramp, g_acquire_tramp, g_unacquire_tramp, g_poll_tramp, g_release_tramp;
std::map<void*, void*> g_create_tramp;  // CreateDevice target -> trampoline
bool g_dinput_hooked = false;
uint32_t g_sequence = 0;

HRESULT(WINAPI* o_DirectInput8Create)(HINSTANCE, DWORD, REFIID, LPVOID*, LPUNKNOWN) = nullptr;

typedef HRESULT(STDMETHODCALLTYPE* CreateDeviceFn)(void*, REFGUID, void**, LPUNKNOWN);
typedef HRESULT(STDMETHODCALLTYPE* StateFn)(void*, DWORD, LPVOID);
typedef HRESULT(STDMETHODCALLTYPE* DataFn)(void*, DWORD, void*, LPDWORD, DWORD);
typedef HRESULT(STDMETHODCALLTYPE* NoArgFn)(void*);
typedef ULONG(STDMETHODCALLTYPE* ReleaseFn)(void*);
typedef HRESULT(STDMETHODCALLTYPE* CapsFn)(void*, DIDEVCAPS*);

void* VtableEntry(void* obj, int index) { return (*reinterpret_cast<void***>(obj))[index]; }

template <typename Fn>
Fn Tramp(std::map<void*, void*>& m, void* self, int slot) {
    Lock l;
    auto it = m.find(VtableEntry(self, slot));
    return it == m.end() ? nullptr : reinterpret_cast<Fn>(it->second);
}

int TrackedType(void* self) {
    Lock l;
    auto it = g_devices.find(self);
    return it == g_devices.end() ? 0 : it->second;
}

HRESULT STDMETHODCALLTYPE d_GetDeviceState(void* self, DWORD cb, LPVOID data) {
    int type = TrackedType(self);
    if (!type) {
        StateFn f = Tramp<StateFn>(g_state_tramp, self, 9);
        return f ? f(self, cb, data) : E_FAIL;
    }
    if (!data) return E_POINTER;
    if (type == kKeyboard) {
        if (cb != 256) return DIERR_INVALIDPARAM;
        KbmDiKeyState(static_cast<uint8_t*>(data));
        return DI_OK;
    }
    if (cb != 16 && cb != 20) return DIERR_INVALIDPARAM;  // DIMOUSESTATE or DIMOUSESTATE2
    int32_t x, y, z;
    uint8_t buttons[8];
    KbmDiMouseState(&x, &y, &z, buttons, true);
    LONG* axes = static_cast<LONG*>(data);
    axes[0] = x;
    axes[1] = y;
    axes[2] = z;
    memcpy(reinterpret_cast<uint8_t*>(data) + 12, buttons, cb - 12);
    return DI_OK;
}

HRESULT STDMETHODCALLTYPE d_GetDeviceData(void* self, DWORD cb, void* out, LPDWORD count, DWORD flags) {
    int type = TrackedType(self);
    if (!type) {
        DataFn f = Tramp<DataFn>(g_data_tramp, self, 10);
        return f ? f(self, cb, out, count, flags) : E_FAIL;
    }
    if (!count) return E_POINTER;
    if (cb != sizeof(DIDEVICEOBJECTDATA) && cb != 16) return DIERR_INVALIDPARAM;
    bool peek = (flags & DIGDD_PEEK) != 0;
    static const size_t kBatch = 128;
    DiEvent ev[kBatch];
    auto take = [&](size_t max, bool keep) {
        return type == kKeyboard ? KbmDiTakeKeys(ev, max, keep) : KbmDiTakeMouse(ev, max, keep);
    };
    if (!out) {
        // A null buffer flushes the queue (unless DIGDD_PEEK).
        if (!peek)
            while (take(kBatch, false) == kBatch) {
            }
        *count = 0;
        return DI_OK;
    }
    size_t want = *count < kBatch ? *count : kBatch;
    size_t n = take(want, peek);
    for (size_t i = 0; i < n; ++i) {
        uint8_t* p = static_cast<uint8_t*>(out) + i * cb;
        DIDEVICEOBJECTDATA full = {};
        full.dwOfs = ev[i].ofs;
        full.dwData = ev[i].data;
        full.dwTimeStamp = ev[i].time;
        full.dwSequence = ++g_sequence;
        memcpy(p, &full, cb);
    }
    *count = static_cast<DWORD>(n);
    return DI_OK;
}

HRESULT STDMETHODCALLTYPE d_Acquire(void* self) {
    if (TrackedType(self)) return DI_OK;
    NoArgFn f = Tramp<NoArgFn>(g_acquire_tramp, self, 7);
    return f ? f(self) : E_FAIL;
}
HRESULT STDMETHODCALLTYPE d_Unacquire(void* self) {
    if (TrackedType(self)) return DI_OK;
    NoArgFn f = Tramp<NoArgFn>(g_unacquire_tramp, self, 8);
    return f ? f(self) : E_FAIL;
}
HRESULT STDMETHODCALLTYPE d_Poll(void* self) {
    if (TrackedType(self)) return DI_NOEFFECT;
    NoArgFn f = Tramp<NoArgFn>(g_poll_tramp, self, 25);
    return f ? f(self) : E_FAIL;
}
ULONG STDMETHODCALLTYPE d_Release(void* self) {
    ReleaseFn f = Tramp<ReleaseFn>(g_release_tramp, self, 2);
    if (!f) return 0;
    ULONG left = f(self);
    if (left == 0) {
        Lock l;
        g_devices.erase(self);
    }
    return left;
}

// Hooks vtable slot `slot` of `obj` once per target address.
void HookSlot(void* obj, int slot, void* detour, std::map<void*, void*>& tramps, const char* label) {
    void* target = VtableEntry(obj, slot);
    {
        Lock l;
        if (tramps.count(target)) return;
    }
    void* orig = nullptr;
    if (!InstallHookAddress(target, detour, &orig, label) || !orig) return;
    Lock l;
    tramps[target] = orig;
}

void HookDeviceClass(void* dev) {
    HookSlot(dev, 2, reinterpret_cast<void*>(&d_Release), g_release_tramp, "IDirectInputDevice8::Release");
    HookSlot(dev, 7, reinterpret_cast<void*>(&d_Acquire), g_acquire_tramp, "IDirectInputDevice8::Acquire");
    HookSlot(dev, 8, reinterpret_cast<void*>(&d_Unacquire), g_unacquire_tramp, "IDirectInputDevice8::Unacquire");
    HookSlot(dev, 9, reinterpret_cast<void*>(&d_GetDeviceState), g_state_tramp, "IDirectInputDevice8::GetDeviceState");
    HookSlot(dev, 10, reinterpret_cast<void*>(&d_GetDeviceData), g_data_tramp, "IDirectInputDevice8::GetDeviceData");
    HookSlot(dev, 25, reinterpret_cast<void*>(&d_Poll), g_poll_tramp, "IDirectInputDevice8::Poll");
    ApplyHooks();
}

HRESULT STDMETHODCALLTYPE CreateDeviceCommon(void* self, REFGUID guid, void** out, LPUNKNOWN outer) {
    CreateDeviceFn orig = Tramp<CreateDeviceFn>(g_create_tramp, self, 3);
    if (!orig) return E_FAIL;
    HRESULT hr = orig(self, guid, out, outer);
    if (FAILED(hr) || !out || !*out) return hr;
    void* dev = *out;
    DIDEVCAPS caps = {};
    caps.dwSize = sizeof(caps);
    int type = 0;
    if (SUCCEEDED(reinterpret_cast<CapsFn>(VtableEntry(dev, 3))(dev, &caps))) {
        DWORD t = caps.dwDevType & 0xFF;
        if (t == DI8DEVTYPE_KEYBOARD) type = kKeyboard;
        else if (t == DI8DEVTYPE_MOUSE) type = kMouse;
    }
    if (!type) return hr;
    HookDeviceClass(dev);
    {
        Lock l;
        g_devices[dev] = type;
    }
    KbmDiEnable(type == kKeyboard, type == kMouse);
    return hr;
}

// IDirectInput8A and W have separate vtables; both CreateDevice hooks land here.
HRESULT STDMETHODCALLTYPE d_CreateDevice(void* self, REFGUID guid, void** out, LPUNKNOWN outer) {
    return CreateDeviceCommon(self, guid, out, outer);
}

HRESULT WINAPI d_DirectInput8Create(HINSTANCE inst, DWORD ver, REFIID riid, LPVOID* out, LPUNKNOWN outer) {
    HRESULT hr = o_DirectInput8Create(inst, ver, riid, out, outer);
    if (SUCCEEDED(hr) && out && *out) HookSlot(*out, 3, reinterpret_cast<void*>(&d_CreateDevice), g_create_tramp, "IDirectInput8::CreateDevice");
    if (SUCCEEDED(hr)) ApplyHooks();
    return hr;
}

void HookModule(HMODULE m) {
    {
        Lock l;
        if (g_dinput_hooked) return;
        g_dinput_hooked = true;
    }
    void* target = reinterpret_cast<void*>(GetProcAddress(m, "DirectInput8Create"));
    if (!InstallHookAddress(target, reinterpret_cast<void*>(&d_DirectInput8Create), reinterpret_cast<void**>(&o_DirectInput8Create),
                            "DirectInput8Create")) {
        Lock l;
        g_dinput_hooked = false;
        return;
    }
    ApplyHooks();
}

}  // namespace

void DInputModuleLoaded(HMODULE m, const wchar_t* path) {
    if (g_cfg.kbm_feed.empty() || !g_kbm) return;
    const wchar_t* base = wcsrchr(path, L'\\');
    base = base ? base + 1 : path;
    if (_wcsicmp(base, L"dinput8.dll") == 0) HookModule(m);
}

void InstallKbmDInput() {
    HMODULE m = GetModuleHandleW(L"dinput8.dll");
    if (m) HookModule(m);
    // Not loaded yet: the LoadLibrary watch in xinput_hook.cpp calls DInputModuleLoaded when it is.
}

}  // namespace cl
