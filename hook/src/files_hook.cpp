// File hooks: copy-on-write for hardlinked game files and save-path redirection.
//
// Copies of a game are trees of NTFS hardlinks. Opening a linked file for writing would change every copy, so
// before such an open the link in this copy is replaced by a private copy of the data (the other links keep the
// old data). Save redirection rewrites paths below a `from` prefix to the same place below `to`.
#include "common.h"

#include <objbase.h>
#include <shlobj.h>

#include <cwchar>

namespace cl {
namespace {

bool g_cow = false;
bool g_saves = false;
CRITICAL_SECTION g_cow_lock;
std::atomic<int> g_cow_fail_notes{0};

HANDLE(WINAPI* o_CreateFileW)(LPCWSTR, DWORD, DWORD, LPSECURITY_ATTRIBUTES, DWORD, DWORD, HANDLE) = nullptr;
HANDLE(WINAPI* o_CreateFileA)(LPCSTR, DWORD, DWORD, LPSECURITY_ATTRIBUTES, DWORD, DWORD, HANDLE) = nullptr;
HANDLE(WINAPI* o_CreateFile2)(LPCWSTR, DWORD, DWORD, DWORD, LPCREATEFILE2_EXTENDED_PARAMETERS) = nullptr;
BOOL(WINAPI* o_CreateDirectoryW)(LPCWSTR, LPSECURITY_ATTRIBUTES) = nullptr;
DWORD(WINAPI* o_GetFileAttributesW)(LPCWSTR) = nullptr;
BOOL(WINAPI* o_GetFileAttributesExW)(LPCWSTR, GET_FILEEX_INFO_LEVELS, LPVOID) = nullptr;
HANDLE(WINAPI* o_FindFirstFileW)(LPCWSTR, LPWIN32_FIND_DATAW) = nullptr;
HANDLE(WINAPI* o_FindFirstFileExW)(LPCWSTR, FINDEX_INFO_LEVELS, LPVOID, FINDEX_SEARCH_OPS, LPVOID, DWORD) = nullptr;
BOOL(WINAPI* o_DeleteFileW)(LPCWSTR) = nullptr;
BOOL(WINAPI* o_RemoveDirectoryW)(LPCWSTR) = nullptr;
HRESULT(WINAPI* o_SHGetKnownFolderPath)(REFKNOWNFOLDERID, DWORD, HANDLE, PWSTR*) = nullptr;
HRESULT(WINAPI* o_SHGetFolderPathW)(HWND, int, HANDLE, DWORD, LPWSTR) = nullptr;

// ---------------------------------------------------------------- redirection

// Rewrites `in` when it lies below a configured `from`. The result keeps the `to` spelling.
bool Redirect(const wchar_t* in, std::wstring& out) {
    if (!in || !*in) return false;
    std::wstring full = NormalizePath(in);
    if (full.empty()) return false;
    for (const SavePair& sp : g_cfg.saves) {
        if (!PathUnder(full, sp.from)) continue;
        out = sp.to + full.substr(sp.from.size());
        if (out.size() >= 240 && out.size() > 2 && out[1] == L':') out = L"\\\\?\\" + out;
        Bump(C_PATH_REDIRECTED);
        return true;
    }
    return false;
}

void StripLongPrefix(std::wstring& path) {
    if (path.compare(0, 4, L"\\\\?\\") == 0) path.erase(0, 4);
}

void EnsureParents(const std::wstring& path) {
    size_t pos = path.rfind(L'\\');
    if (pos == std::wstring::npos || pos < 3) return;
    std::wstring dir = path.substr(0, pos);
    if (o_GetFileAttributesW && o_GetFileAttributesW(dir.c_str()) != INVALID_FILE_ATTRIBUTES) return;
    EnsureParents(dir);
    if (o_CreateDirectoryW) o_CreateDirectoryW(dir.c_str(), nullptr);
}

// ---------------------------------------------------------------- copy on write

bool WriteIntent(DWORD access, DWORD disp) {
    constexpr DWORD kWrite = GENERIC_WRITE | GENERIC_ALL | FILE_WRITE_DATA | FILE_APPEND_DATA;
    return (access & kWrite) != 0 || disp == TRUNCATE_EXISTING || disp == CREATE_ALWAYS;
}

bool MakesFile(DWORD access, DWORD disp) {
    return WriteIntent(access, disp) || disp == CREATE_NEW || disp == OPEN_ALWAYS;
}

void CowFailed(const std::wstring& path, const char* step) {
    Bump(C_COW_FAILED);
    if (g_cow_fail_notes.fetch_add(1) < 10)
        Note(std::string("copy-on-write failed (") + step + ", error " + std::to_string(GetLastError()) + "): " + Narrow(path));
}

// Replaces the hardlink at `path` by a private file when the file has other links.
void BreakLink(const wchar_t* in, DWORD disp) {
    std::wstring full = NormalizePath(in);
    if (full.empty() || !PathUnder(full, g_cfg.cow_root)) return;
    HANDLE h = o_CreateFileW(full.c_str(), FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                             nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h == INVALID_HANDLE_VALUE) return;  // does not exist yet (a new file is private by definition)
    BY_HANDLE_FILE_INFORMATION info;
    BOOL ok = GetFileInformationByHandle(h, &info);
    CloseHandle(h);
    if (!ok || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) || info.nNumberOfLinks <= 1) return;

    EnterCriticalSection(&g_cow_lock);
    // Another thread may have replaced the link while we waited.
    h = o_CreateFileW(full.c_str(), FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
                      OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    bool still_shared = false;
    if (h != INVALID_HANDLE_VALUE) {
        still_shared = GetFileInformationByHandle(h, &info) && info.nNumberOfLinks > 1;
        CloseHandle(h);
    }
    if (still_shared) {
        std::wstring tmp = full + L".clcow" + std::to_wstring(GetCurrentProcessId()) + L"_" + std::to_wstring(GetTickCount64());
        bool made;
        if (disp == TRUNCATE_EXISTING || disp == CREATE_ALWAYS) {
            // The data is about to be thrown away, so skip copying it.
            HANDLE e = o_CreateFileW(tmp.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr);
            made = e != INVALID_HANDLE_VALUE;
            if (made) CloseHandle(e);
        } else {
            made = CopyFileW(full.c_str(), tmp.c_str(), TRUE) != FALSE;
        }
        if (!made) {
            CowFailed(full, "copy");
        } else if (!MoveFileExW(tmp.c_str(), full.c_str(), MOVEFILE_REPLACE_EXISTING)) {
            CowFailed(full, "replace");
            DeleteFileW(tmp.c_str());
        } else {
            Bump(C_COW_BROKEN);
        }
    }
    LeaveCriticalSection(&g_cow_lock);
}

// Applies redirection and copy-on-write for an open. Returns true when `out` holds the path to use instead.
bool PrepareOpen(const wchar_t* in, DWORD access, DWORD disp, std::wstring& out) {
    ReentryGuard guard;
    bool redirected = false;
    const wchar_t* eff = in;
    if (g_saves && Redirect(in, out)) {
        redirected = true;
        eff = out.c_str();
        if (MakesFile(access, disp)) EnsureParents(out);
    }
    if (g_cow && WriteIntent(access, disp)) BreakLink(eff, disp);
    return redirected;
}

inline bool Skip(const void* p) { return ReentryGuard::active() || !p || !(g_saves || g_cow); }

HANDLE WINAPI d_CreateFileW(LPCWSTR name, DWORD access, DWORD share, LPSECURITY_ATTRIBUTES sa, DWORD disp, DWORD flags,
                            HANDLE tmpl) {
    if (Skip(name)) return o_CreateFileW(name, access, share, sa, disp, flags, tmpl);
    std::wstring path;
    if (PrepareOpen(name, access, disp, path)) name = path.c_str();
    ReentryGuard call_guard;
    return o_CreateFileW(name, access, share, sa, disp, flags, tmpl);
}

HANDLE WINAPI d_CreateFileA(LPCSTR name, DWORD access, DWORD share, LPSECURITY_ATTRIBUTES sa, DWORD disp, DWORD flags,
                            HANDLE tmpl) {
    if (Skip(name)) return o_CreateFileA(name, access, share, sa, disp, flags, tmpl);
    int n = MultiByteToWideChar(CP_ACP, 0, name, -1, nullptr, 0);
    if (n <= 1) return o_CreateFileA(name, access, share, sa, disp, flags, tmpl);
    std::wstring wide(static_cast<size_t>(n), L'\0');
    MultiByteToWideChar(CP_ACP, 0, name, -1, &wide[0], n);
    wide.resize(static_cast<size_t>(n - 1));
    std::wstring path;
    bool red = PrepareOpen(wide.c_str(), access, disp, path);
    ReentryGuard call_guard;
    if (red) return o_CreateFileW(path.c_str(), access, share, sa, disp, flags, tmpl);
    return o_CreateFileA(name, access, share, sa, disp, flags, tmpl);
}

HANDLE WINAPI d_CreateFile2(LPCWSTR name, DWORD access, DWORD share, DWORD disp, LPCREATEFILE2_EXTENDED_PARAMETERS ext) {
    if (Skip(name)) return o_CreateFile2(name, access, share, disp, ext);
    std::wstring path;
    if (PrepareOpen(name, access, disp, path)) name = path.c_str();
    ReentryGuard call_guard;
    return o_CreateFile2(name, access, share, disp, ext);
}

BOOL WINAPI d_CreateDirectoryW(LPCWSTR name, LPSECURITY_ATTRIBUTES sa) {
    if (Skip(name) || !g_saves) return o_CreateDirectoryW(name, sa);
    std::wstring path;
    bool red;
    {
        ReentryGuard guard;
        red = Redirect(name, path);
        if (red) EnsureParents(path);
    }
    ReentryGuard call_guard;
    return o_CreateDirectoryW(red ? path.c_str() : name, sa);
}

DWORD WINAPI d_GetFileAttributesW(LPCWSTR name) {
    if (Skip(name) || !g_saves) return o_GetFileAttributesW(name);
    std::wstring path;
    bool red;
    {
        ReentryGuard guard;
        red = Redirect(name, path);
    }
    ReentryGuard call_guard;
    return o_GetFileAttributesW(red ? path.c_str() : name);
}

BOOL WINAPI d_GetFileAttributesExW(LPCWSTR name, GET_FILEEX_INFO_LEVELS level, LPVOID info) {
    if (Skip(name) || !g_saves) return o_GetFileAttributesExW(name, level, info);
    std::wstring path;
    bool red;
    {
        ReentryGuard guard;
        red = Redirect(name, path);
    }
    ReentryGuard call_guard;
    return o_GetFileAttributesExW(red ? path.c_str() : name, level, info);
}

HANDLE WINAPI d_FindFirstFileW(LPCWSTR name, LPWIN32_FIND_DATAW data) {
    if (Skip(name) || !g_saves) return o_FindFirstFileW(name, data);
    std::wstring path;
    bool red;
    {
        ReentryGuard guard;
        red = Redirect(name, path);
    }
    ReentryGuard call_guard;
    return o_FindFirstFileW(red ? path.c_str() : name, data);
}

HANDLE WINAPI d_FindFirstFileExW(LPCWSTR name, FINDEX_INFO_LEVELS level, LPVOID data, FINDEX_SEARCH_OPS op, LPVOID filter,
                                 DWORD flags) {
    if (Skip(name) || !g_saves) return o_FindFirstFileExW(name, level, data, op, filter, flags);
    std::wstring path;
    bool red;
    {
        ReentryGuard guard;
        red = Redirect(name, path);
    }
    ReentryGuard call_guard;
    return o_FindFirstFileExW(red ? path.c_str() : name, level, data, op, filter, flags);
}

BOOL WINAPI d_DeleteFileW(LPCWSTR name) {
    if (Skip(name) || !g_saves) return o_DeleteFileW(name);
    std::wstring path;
    bool red;
    {
        ReentryGuard guard;
        red = Redirect(name, path);
    }
    ReentryGuard call_guard;
    return o_DeleteFileW(red ? path.c_str() : name);
}

BOOL WINAPI d_RemoveDirectoryW(LPCWSTR name) {
    if (Skip(name) || !g_saves) return o_RemoveDirectoryW(name);
    std::wstring path;
    bool red;
    {
        ReentryGuard guard;
        red = Redirect(name, path);
    }
    ReentryGuard call_guard;
    return o_RemoveDirectoryW(red ? path.c_str() : name);
}

// The shell checks and creates the real folder with ordinary file calls; those must not be redirected, so the
// original runs under the guard. The returned string is rewritten afterwards.
HRESULT WINAPI d_SHGetKnownFolderPath(REFKNOWNFOLDERID id, DWORD flags, HANDLE token, PWSTR* out) {
    bool nested = ReentryGuard::active();
    HRESULT hr;
    {
        ReentryGuard guard;
        hr = o_SHGetKnownFolderPath(id, flags, token, out);
    }
    if (FAILED(hr) || !out || !*out || !g_saves || nested) return hr;
    std::wstring path;
    bool red;
    {
        ReentryGuard guard;
        red = Redirect(*out, path);
        if (red) StripLongPrefix(path);
        if (red) EnsureParents(path + L"\\_");
    }
    if (red) {
        size_t bytes = (path.size() + 1) * sizeof(wchar_t);
        wchar_t* mem = static_cast<wchar_t*>(CoTaskMemAlloc(bytes));
        if (mem) {
            memcpy(mem, path.c_str(), bytes);
            CoTaskMemFree(*out);
            *out = mem;
        }
    }
    return hr;
}

HRESULT WINAPI d_SHGetFolderPathW(HWND wnd, int csidl, HANDLE token, DWORD flags, LPWSTR buf) {
    bool nested = ReentryGuard::active();
    HRESULT hr;
    {
        ReentryGuard guard;
        hr = o_SHGetFolderPathW(wnd, csidl, token, flags, buf);
    }
    if (FAILED(hr) || !buf || !g_saves || nested) return hr;
    std::wstring path;
    bool red;
    {
        ReentryGuard guard;
        red = Redirect(buf, path);
        if (red) StripLongPrefix(path);
        if (red) EnsureParents(path + L"\\_");
    }
    if (red && path.size() < MAX_PATH) wcscpy_s(buf, MAX_PATH, path.c_str());
    return hr;
}

#define HK(mod, name) InstallHook(mod, #name, reinterpret_cast<void*>(&d_##name), reinterpret_cast<void**>(&o_##name))

}  // namespace

void InstallFiles() {
    g_cow = !g_cfg.cow_root.empty();
    g_saves = !g_cfg.saves.empty();
    if (!g_cow && !g_saves) return;
    InitializeCriticalSection(&g_cow_lock);
    HMODULE k = GetModuleHandleW(L"kernelbase.dll");
    if (!k) k = GetModuleHandleW(L"kernel32.dll");
    // The trampolines are also used by the copy-on-write code, so these are always installed.
    HK(k, CreateFileW);
    HK(k, CreateFileA);
    HK(k, CreateFile2);
    HK(k, CreateDirectoryW);
    HK(k, GetFileAttributesW);
    if (g_saves) {
        HK(k, GetFileAttributesExW);
        HK(k, FindFirstFileW);
        HK(k, FindFirstFileExW);
        HK(k, DeleteFileW);
        HK(k, RemoveDirectoryW);
        HMODULE sh = GetModuleHandleW(L"shell32.dll");
        if (!sh) sh = LoadLibraryW(L"shell32.dll");
        HK(sh, SHGetKnownFolderPath);
        HK(sh, SHGetFolderPathW);
    }
    g_features.cow = g_cow;
    g_features.saves = g_saves;
}

}  // namespace cl
