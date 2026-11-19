// Single-instance checks by window: many games look for their own window (FindWindow with the window class or the
// title) before they start and quit when another copy has one. A copy that runs next to another copy of the same
// game must not see the other copy's windows, so the find calls skip a window that belongs to another process
// running the same program. Other programs' windows (Steam, a launcher, the desktop) are found as before.
// Installed with the lock renaming: a copy that may run next to others is the copy that needs it.
#include <algorithm>
#include <cwchar>
#include <map>

#include "common.h"

namespace cl {
namespace {

HWND(WINAPI* o_FindWindowW)(LPCWSTR, LPCWSTR) = nullptr;
HWND(WINAPI* o_FindWindowA)(LPCSTR, LPCSTR) = nullptr;
HWND(WINAPI* o_FindWindowExW)(HWND, HWND, LPCWSTR, LPCWSTR) = nullptr;
HWND(WINAPI* o_FindWindowExA)(HWND, HWND, LPCSTR, LPCSTR) = nullptr;

std::wstring ExeOf(DWORD pid) {
    HANDLE h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
    if (!h) return L"";
    wchar_t buf[1024];
    DWORD n = 1024;
    std::wstring out;
    if (QueryFullProcessImageNameW(h, 0, buf, &n)) out.assign(buf, n);
    CloseHandle(h);
    return out;
}

std::wstring BaseName(const std::wstring& path) {
    size_t i = path.find_last_of(L"\/");
    return i == std::wstring::npos ? path : path.substr(i + 1);
}

// Process ids the session set aside (a launcher's own copy of the game, parked while the copies play): COUCHLINK_IGNORE_PIDS.
bool IsIgnoredPid(DWORD pid) {
    static const std::wstring list = [] {
        wchar_t buf[256];
        DWORD n = GetEnvironmentVariableW(L"COUCHLINK_IGNORE_PIDS", buf, 256);
        return n && n < 256 ? std::wstring(buf, n) : std::wstring();
    }();
    for (size_t at = 0; at < list.size();) {
        size_t end = list.find_first_of(L",;", at);
        if (end == std::wstring::npos) end = list.size();
        if (_wtoi(list.substr(at, end - at).c_str()) == static_cast<int>(pid)) return true;
        at = end + 1;
    }
    return false;
}

// True when the process runs under Couchlink too (its hook marks the process with an event).
bool IsCouchlinkCopy(DWORD pid) {
    wchar_t name[96];
    wsprintfW(name, L"Local\couchlink-hook-active-%lu", pid);
    HANDLE h = OpenEventW(SYNCHRONIZE, FALSE, name);
    if (!h) return false;
    CloseHandle(h);
    return true;
}

// True for a window of another copy of this game: a process Couchlink set aside, or one that runs a program of the same
// name (from its own folder, or the same one) under Couchlink.
bool IsOtherCopy(HWND w) {
    static const std::wstring own = ExeOf(GetCurrentProcessId());
    DWORD pid = 0;
    GetWindowThreadProcessId(w, &pid);
    if (!pid || pid == GetCurrentProcessId() || own.empty()) return false;
    if (IsIgnoredPid(pid)) return true;
    std::wstring other = ExeOf(pid);
    if (_wcsicmp(other.c_str(), own.c_str()) == 0) return true;
    return _wcsicmp(BaseName(other).c_str(), BaseName(own).c_str()) == 0 && IsCouchlinkCopy(pid);
}

template <class Find, class Str>
HWND FirstNotOtherCopy(Find find, HWND parent, HWND after, Str cls, Str title) {
    HWND w = find(parent, after, cls, title);
    for (int guard = 0; w && IsOtherCopy(w) && guard < 256; ++guard) {
        Bump(C_FINDWINDOW_SKIPPED);
        w = find(parent, w, cls, title);
    }
    return w;
}

HWND WINAPI d_FindWindowExW(HWND parent, HWND after, LPCWSTR cls, LPCWSTR title) {
    return FirstNotOtherCopy(o_FindWindowExW, parent, after, cls, title);
}
HWND WINAPI d_FindWindowExA(HWND parent, HWND after, LPCSTR cls, LPCSTR title) {
    return FirstNotOtherCopy(o_FindWindowExA, parent, after, cls, title);
}
HWND WINAPI d_FindWindowW(LPCWSTR cls, LPCWSTR title) {
    return FirstNotOtherCopy(o_FindWindowExW, nullptr, nullptr, cls, title);
}
HWND WINAPI d_FindWindowA(LPCSTR cls, LPCSTR title) {
    return FirstNotOtherCopy(o_FindWindowExA, nullptr, nullptr, cls, title);
}

#define HK(mod, name) InstallHook(mod, #name, reinterpret_cast<void*>(&d_##name), reinterpret_cast<void**>(&o_##name))

}  // namespace

void InstallFindWindow() {
    HMODULE u = GetModuleHandleW(L"user32.dll");
    if (!u) u = LoadLibraryW(L"user32.dll");
    if (!u) return;
    HK(u, FindWindowExW);
    HK(u, FindWindowExA);
    HK(u, FindWindowW);
    HK(u, FindWindowA);
}

}  // namespace cl
