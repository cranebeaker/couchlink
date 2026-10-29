// Single-instance locks: named mutexes, events and semaphores get a per-process suffix so a second copy of the
// game can start while the first runs. A name is renamed when it contains one of the configured strings
// (case-insensitive); the entry "*" renames every named mutex (not events or semaphores).
//
// ProtoInput's RenameHandlesHook (MIT, see THIRD_PARTY.md) does this at the Nt level; this version hooks the
// documented Create/Open entry points in kernelbase.
#include <cwchar>

#include "common.h"

namespace cl {
namespace {

enum class Kind { Mutex, Event, Semaphore };

std::wstring g_suffix_w;
std::string g_suffix_a;

bool ContainsNoCase(const std::wstring& hay, const std::wstring& needle) {
    if (needle.empty() || hay.size() < needle.size()) return false;
    for (size_t i = 0; i + needle.size() <= hay.size(); ++i) {
        if (CompareStringOrdinal(hay.c_str() + i, static_cast<int>(needle.size()), needle.c_str(),
                                 static_cast<int>(needle.size()), TRUE) == CSTR_EQUAL)
            return true;
    }
    return false;
}

bool EndsWithNoCase(const std::wstring& s, const std::wstring& tail) {
    return s.size() >= tail.size() &&
           CompareStringOrdinal(s.c_str() + s.size() - tail.size(), static_cast<int>(tail.size()), tail.c_str(),
                                static_cast<int>(tail.size()), TRUE) == CSTR_EQUAL;
}

// Windows-owned names that "*" must not touch.
bool IsSystemName(const std::wstring& n) {
    static const wchar_t* const parts[] = {L"SM0:", L"MSCTF", L"CicLoad", L"ShimCache", L"RasPbFile", L"DBWinMutex"};
    for (const wchar_t* p : parts)
        if (ContainsNoCase(n, p)) return true;
    return false;
}

bool ShouldRename(const std::wstring& name, Kind kind) {
    if (name.empty() || EndsWithNoCase(name, g_suffix_w)) return false;
    for (const auto& l : g_cfg.locks)
        if (ContainsNoCase(name, l)) return true;
    return g_cfg.locks_all && kind == Kind::Mutex && !IsSystemName(name);
}

// Returns true and fills `out` when `name` must be renamed.
bool RenameW(LPCWSTR name, Kind kind, std::wstring& out) {
    if (!name || !*name) return false;
    std::wstring n(name);
    if (!ShouldRename(n, kind)) return false;
    out = n + g_suffix_w;
    Bump(C_NAMES_RENAMED);
    return true;
}

bool RenameA(LPCSTR name, Kind kind, std::string& out) {
    if (!name || !*name) return false;
    std::wstring w = Widen(std::string(name));  // ANSI names in practice are ASCII; good enough for matching
    if (!ShouldRename(w, kind)) return false;
    out = std::string(name) + g_suffix_a;
    Bump(C_NAMES_RENAMED);
    return true;
}

#define W_ARG(kind, name)       \
    std::wstring renamed_;      \
    if (RenameW(name, kind, renamed_)) name = renamed_.c_str()
#define A_ARG(kind, name)       \
    std::string renamed_;       \
    if (RenameA(name, kind, renamed_)) name = renamed_.c_str()

HANDLE(WINAPI* o_CreateMutexW)(LPSECURITY_ATTRIBUTES, BOOL, LPCWSTR) = nullptr;
HANDLE(WINAPI* o_CreateMutexA)(LPSECURITY_ATTRIBUTES, BOOL, LPCSTR) = nullptr;
HANDLE(WINAPI* o_CreateMutexExW)(LPSECURITY_ATTRIBUTES, LPCWSTR, DWORD, DWORD) = nullptr;
HANDLE(WINAPI* o_CreateMutexExA)(LPSECURITY_ATTRIBUTES, LPCSTR, DWORD, DWORD) = nullptr;
HANDLE(WINAPI* o_OpenMutexW)(DWORD, BOOL, LPCWSTR) = nullptr;
HANDLE(WINAPI* o_OpenMutexA)(DWORD, BOOL, LPCSTR) = nullptr;
HANDLE(WINAPI* o_CreateEventW)(LPSECURITY_ATTRIBUTES, BOOL, BOOL, LPCWSTR) = nullptr;
HANDLE(WINAPI* o_CreateEventA)(LPSECURITY_ATTRIBUTES, BOOL, BOOL, LPCSTR) = nullptr;
HANDLE(WINAPI* o_CreateEventExW)(LPSECURITY_ATTRIBUTES, LPCWSTR, DWORD, DWORD) = nullptr;
HANDLE(WINAPI* o_CreateEventExA)(LPSECURITY_ATTRIBUTES, LPCSTR, DWORD, DWORD) = nullptr;
HANDLE(WINAPI* o_OpenEventW)(DWORD, BOOL, LPCWSTR) = nullptr;
HANDLE(WINAPI* o_OpenEventA)(DWORD, BOOL, LPCSTR) = nullptr;
HANDLE(WINAPI* o_CreateSemaphoreW)(LPSECURITY_ATTRIBUTES, LONG, LONG, LPCWSTR) = nullptr;
HANDLE(WINAPI* o_CreateSemaphoreA)(LPSECURITY_ATTRIBUTES, LONG, LONG, LPCSTR) = nullptr;
HANDLE(WINAPI* o_CreateSemaphoreExW)(LPSECURITY_ATTRIBUTES, LONG, LONG, LPCWSTR, DWORD, DWORD) = nullptr;
HANDLE(WINAPI* o_CreateSemaphoreExA)(LPSECURITY_ATTRIBUTES, LONG, LONG, LPCSTR, DWORD, DWORD) = nullptr;
HANDLE(WINAPI* o_OpenSemaphoreW)(DWORD, BOOL, LPCWSTR) = nullptr;
HANDLE(WINAPI* o_OpenSemaphoreA)(DWORD, BOOL, LPCSTR) = nullptr;

HANDLE WINAPI d_CreateMutexW(LPSECURITY_ATTRIBUTES sa, BOOL own, LPCWSTR name) {
    W_ARG(Kind::Mutex, name);
    return o_CreateMutexW(sa, own, name);
}
HANDLE WINAPI d_CreateMutexA(LPSECURITY_ATTRIBUTES sa, BOOL own, LPCSTR name) {
    A_ARG(Kind::Mutex, name);
    return o_CreateMutexA(sa, own, name);
}
HANDLE WINAPI d_CreateMutexExW(LPSECURITY_ATTRIBUTES sa, LPCWSTR name, DWORD flags, DWORD access) {
    W_ARG(Kind::Mutex, name);
    return o_CreateMutexExW(sa, name, flags, access);
}
HANDLE WINAPI d_CreateMutexExA(LPSECURITY_ATTRIBUTES sa, LPCSTR name, DWORD flags, DWORD access) {
    A_ARG(Kind::Mutex, name);
    return o_CreateMutexExA(sa, name, flags, access);
}
HANDLE WINAPI d_OpenMutexW(DWORD access, BOOL inherit, LPCWSTR name) {
    W_ARG(Kind::Mutex, name);
    return o_OpenMutexW(access, inherit, name);
}
HANDLE WINAPI d_OpenMutexA(DWORD access, BOOL inherit, LPCSTR name) {
    A_ARG(Kind::Mutex, name);
    return o_OpenMutexA(access, inherit, name);
}
HANDLE WINAPI d_CreateEventW(LPSECURITY_ATTRIBUTES sa, BOOL manual, BOOL initial, LPCWSTR name) {
    W_ARG(Kind::Event, name);
    return o_CreateEventW(sa, manual, initial, name);
}
HANDLE WINAPI d_CreateEventA(LPSECURITY_ATTRIBUTES sa, BOOL manual, BOOL initial, LPCSTR name) {
    A_ARG(Kind::Event, name);
    return o_CreateEventA(sa, manual, initial, name);
}
HANDLE WINAPI d_CreateEventExW(LPSECURITY_ATTRIBUTES sa, LPCWSTR name, DWORD flags, DWORD access) {
    W_ARG(Kind::Event, name);
    return o_CreateEventExW(sa, name, flags, access);
}
HANDLE WINAPI d_CreateEventExA(LPSECURITY_ATTRIBUTES sa, LPCSTR name, DWORD flags, DWORD access) {
    A_ARG(Kind::Event, name);
    return o_CreateEventExA(sa, name, flags, access);
}
HANDLE WINAPI d_OpenEventW(DWORD access, BOOL inherit, LPCWSTR name) {
    W_ARG(Kind::Event, name);
    return o_OpenEventW(access, inherit, name);
}
HANDLE WINAPI d_OpenEventA(DWORD access, BOOL inherit, LPCSTR name) {
    A_ARG(Kind::Event, name);
    return o_OpenEventA(access, inherit, name);
}
HANDLE WINAPI d_CreateSemaphoreW(LPSECURITY_ATTRIBUTES sa, LONG initial, LONG max, LPCWSTR name) {
    W_ARG(Kind::Semaphore, name);
    return o_CreateSemaphoreW(sa, initial, max, name);
}
HANDLE WINAPI d_CreateSemaphoreA(LPSECURITY_ATTRIBUTES sa, LONG initial, LONG max, LPCSTR name) {
    A_ARG(Kind::Semaphore, name);
    return o_CreateSemaphoreA(sa, initial, max, name);
}
HANDLE WINAPI d_CreateSemaphoreExW(LPSECURITY_ATTRIBUTES sa, LONG initial, LONG max, LPCWSTR name, DWORD flags, DWORD access) {
    W_ARG(Kind::Semaphore, name);
    return o_CreateSemaphoreExW(sa, initial, max, name, flags, access);
}
HANDLE WINAPI d_CreateSemaphoreExA(LPSECURITY_ATTRIBUTES sa, LONG initial, LONG max, LPCSTR name, DWORD flags, DWORD access) {
    A_ARG(Kind::Semaphore, name);
    return o_CreateSemaphoreExA(sa, initial, max, name, flags, access);
}
HANDLE WINAPI d_OpenSemaphoreW(DWORD access, BOOL inherit, LPCWSTR name) {
    W_ARG(Kind::Semaphore, name);
    return o_OpenSemaphoreW(access, inherit, name);
}
HANDLE WINAPI d_OpenSemaphoreA(DWORD access, BOOL inherit, LPCSTR name) {
    A_ARG(Kind::Semaphore, name);
    return o_OpenSemaphoreA(access, inherit, name);
}

// Most Create/Open functions live in kernelbase.dll; some ANSI ones are only exported by kernel32.dll.
HMODULE g_kb = nullptr;
HMODULE g_k32 = nullptr;
bool InstallEither(const char* name, void* detour, void** original) {
    HMODULE m = (g_kb && GetProcAddress(g_kb, name)) ? g_kb : g_k32;
    return InstallHook(m, name, detour, original);
}
#define HK(unused, name) InstallEither(#name, reinterpret_cast<void*>(&d_##name), reinterpret_cast<void**>(&o_##name))

}  // namespace

void InstallNames() {
    if (g_cfg.locks.empty() && !g_cfg.locks_all) return;
    g_suffix_a = "_cl" + std::to_string(CopyId());
    g_suffix_w = Widen(g_suffix_a);
    g_kb = GetModuleHandleW(L"kernelbase.dll");
    g_k32 = GetModuleHandleW(L"kernel32.dll");
    HMODULE k = nullptr;
    HK(k, CreateMutexW);
    HK(k, CreateMutexA);
    HK(k, CreateMutexExW);
    HK(k, CreateMutexExA);
    HK(k, OpenMutexW);
    HK(k, OpenMutexA);
    HK(k, CreateEventW);
    HK(k, CreateEventA);
    HK(k, CreateEventExW);
    HK(k, CreateEventExA);
    HK(k, OpenEventW);
    HK(k, OpenEventA);
    HK(k, CreateSemaphoreW);
    HK(k, CreateSemaphoreA);
    HK(k, CreateSemaphoreExW);
    HK(k, CreateSemaphoreExA);
    HK(k, OpenSemaphoreW);
    HK(k, OpenSemaphoreA);
    g_features.names = true;
}

}  // namespace cl
