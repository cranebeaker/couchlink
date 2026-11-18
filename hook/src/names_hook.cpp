// Single-instance locks: named mutexes, events and semaphores get a per-process suffix so a second copy of the
// game can start while the first runs. A name is renamed when it contains one of the configured strings
// (case-insensitive); the entry "*" renames every named mutex (not events or semaphores).
//
// Steam's own channels: `steam_names` (mutexes, events, semaphores and shared memory) get the fixed `steam_suffix`
// instead. A seat's own Steam and that seat's game copy both get it, so the overlay channels Steam names the same in
// every Steam (GameOverlayRender_PIDStream, ...) are their own and not the ones the Steam of the person at the PC
// made first.
//
// ProtoInput's RenameHandlesHook (MIT, see THIRD_PARTY.md) does this at the Nt level; this version hooks the
// documented Create/Open entry points in kernelbase.
#include <cwchar>

#include "common.h"

namespace cl {
namespace {

enum class Kind { Mutex, Event, Semaphore, Section };

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

bool IsSteamName(const std::wstring& name) {
    if (EndsWithNoCase(name, g_cfg.steam_suffix)) return false;
    for (const auto& n : g_cfg.steam_names)
        if (ContainsNoCase(name, n)) return true;
    return false;
}

bool ShouldRename(const std::wstring& name, Kind kind) {
    if (kind == Kind::Section || g_suffix_w.empty() || name.empty() || EndsWithNoCase(name, g_suffix_w)) return false;
    for (const auto& l : g_cfg.locks)
        if (ContainsNoCase(name, l)) return true;
    return g_cfg.locks_all && kind == Kind::Mutex && !IsSystemName(name);
}

// Returns true and fills `out` when `name` must be renamed.
bool RenameW(LPCWSTR name, Kind kind, std::wstring& out) {
    if (!name || !*name) return false;
    std::wstring n(name);
    if (IsSteamName(n)) {
        out = n + g_cfg.steam_suffix;
    } else if (ShouldRename(n, kind)) {
        out = n + g_suffix_w;
    } else {
        return false;
    }
    Bump(C_NAMES_RENAMED);
    return true;
}

bool RenameA(LPCSTR name, Kind kind, std::string& out) {
    if (!name || !*name) return false;
    std::wstring w = Widen(std::string(name));  // ANSI names in practice are ASCII; good enough for matching
    if (IsSteamName(w)) {
        out = std::string(name) + Narrow(g_cfg.steam_suffix);
    } else if (ShouldRename(w, kind)) {
        out = std::string(name) + g_suffix_a;
    } else {
        return false;
    }
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
HANDLE(WINAPI* o_CreateFileMappingW)(HANDLE, LPSECURITY_ATTRIBUTES, DWORD, DWORD, DWORD, LPCWSTR) = nullptr;
HANDLE(WINAPI* o_CreateFileMappingA)(HANDLE, LPSECURITY_ATTRIBUTES, DWORD, DWORD, DWORD, LPCSTR) = nullptr;
HANDLE(WINAPI* o_OpenFileMappingW)(DWORD, BOOL, LPCWSTR) = nullptr;
HANDLE(WINAPI* o_OpenFileMappingA)(DWORD, BOOL, LPCSTR) = nullptr;

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

HANDLE WINAPI d_CreateFileMappingW(HANDLE file, LPSECURITY_ATTRIBUTES sa, DWORD protect, DWORD hi, DWORD lo, LPCWSTR name) {
    W_ARG(Kind::Section, name);
    return o_CreateFileMappingW(file, sa, protect, hi, lo, name);
}
HANDLE WINAPI d_CreateFileMappingA(HANDLE file, LPSECURITY_ATTRIBUTES sa, DWORD protect, DWORD hi, DWORD lo, LPCSTR name) {
    A_ARG(Kind::Section, name);
    return o_CreateFileMappingA(file, sa, protect, hi, lo, name);
}
HANDLE WINAPI d_OpenFileMappingW(DWORD access, BOOL inherit, LPCWSTR name) {
    W_ARG(Kind::Section, name);
    return o_OpenFileMappingW(access, inherit, name);
}
HANDLE WINAPI d_OpenFileMappingA(DWORD access, BOOL inherit, LPCSTR name) {
    A_ARG(Kind::Section, name);
    return o_OpenFileMappingA(access, inherit, name);
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
    bool locks = !g_cfg.locks.empty() || g_cfg.locks_all;
    if (!locks && g_cfg.steam_names.empty()) return;
    if (locks) {
        g_suffix_a = "_cl" + std::to_string(CopyId());
        g_suffix_w = Widen(g_suffix_a);
    }
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
    if (!g_cfg.steam_names.empty()) {
        // A seat's Steam (not its children or games) shows it got its own channels: Couchlink checks for this event.
        wchar_t exe[MAX_PATH] = L"";
        GetModuleFileNameW(nullptr, exe, MAX_PATH);
        const wchar_t* base = wcsrchr(exe, L'\\');
        if (base && _wcsicmp(base + 1, L"steam.exe") == 0) {
            std::wstring marker = L"Couchlink_SteamChannels" + g_cfg.steam_suffix;
            CreateEventW(nullptr, TRUE, FALSE, marker.c_str());  // kept open for the life of Steam
        }
        // A program that Steam started (its browser, its overlay window for a game) finds its Steam by the name Steam
        // got on its command line (-master_ipc_name_override), which does not reach the programs it starts: without it
        // they talk to the other Steam, and the browser dies when that Steam starts a game. The suffix is "_" + the name.
        // Each program the hook followed into shows it (Couchlink's checks look for this event).
        if (g_cfg.depth > 0) {
            std::wstring mark = L"Couchlink_Hooked_" + std::to_wstring(GetCurrentProcessId());
            CreateEventW(nullptr, TRUE, FALSE, mark.c_str());
        }
        if (g_cfg.depth > 0 && base && (_wcsnicmp(base + 1, L"gameoverlayui", 13) == 0 || _wcsicmp(base + 1, L"steamwebhelper.exe") == 0) &&
            g_cfg.steam_suffix.size() > 1 && !GetEnvironmentVariableW(L"steam_master_ipc_name_override", nullptr, 0))
            SetEnvironmentVariableW(L"steam_master_ipc_name_override", g_cfg.steam_suffix.c_str() + 1);
        HK(k, CreateFileMappingW);
        HK(k, CreateFileMappingA);
        HK(k, OpenFileMappingW);
        HK(k, OpenFileMappingA);
    }
    g_features.names = true;
    // A copy that may run next to others does not see their windows either (single-instance checks by window).
    InstallFindWindow();
}

}  // namespace cl
