#include "common.h"

#include <cwchar>

namespace cl {

Config g_cfg;
volatile FeedBlock* g_feed = nullptr;
volatile KbmHeader* g_kbm = nullptr;
Features g_features;
const char* g_load_mode = "inject";
thread_local int ReentryGuard::depth = 0;

namespace {

const char* const kCounterNames[C_COUNT] = {
    "xinput_get_state",   "xinput_set_state", "xinput_other",   "xinput_mapped",  "xinput_blocked", "xinput_feed_reads", "xinput_feed_rumble",
    "focus_queries",      "activate_swallowed", "windows_subclassed", "names_renamed", "cow_broken",
    "cow_failed",         "paths_redirected",
    "kbm_events",         "kbm_key_messages", "kbm_char_messages", "kbm_mouse_messages", "kbm_raw_messages",
    "kbm_real_dropped",   "kbm_state_queries", "kbm_cursor_queries", "kbm_dinput_reads", "kbm_raw_reads",
    "children_hooked",    "children_failed",   "children_skipped",
    "pads_hidden",
};

std::atomic<uint64_t> g_counters[C_COUNT];

CRITICAL_SECTION g_lock;
INIT_ONCE g_lock_once = INIT_ONCE_STATIC_INIT;
std::vector<std::string> g_notes;
std::vector<std::string> g_xinput_modules;
uint16_t g_vibration[4][2];

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

}  // namespace

void Bump(Counter c, uint64_t n) { g_counters[c].fetch_add(n, std::memory_order_relaxed); }

void Note(const std::string& text) {
    Lock l;
    if (g_notes.size() >= 64) g_notes.erase(g_notes.begin());
    g_notes.push_back(text);
}

void NoteHookFailure(const char* what) { Note(std::string("hook failed: ") + what); }

void SetXInputVibration(int real_index, uint16_t left, uint16_t right) {
    if (real_index < 0 || real_index > 3) return;
    Lock l;
    g_vibration[real_index][0] = left;
    g_vibration[real_index][1] = right;
}

void AddXInputModule(const std::string& name) {
    Lock l;
    for (const auto& n : g_xinput_modules)
        if (n == name) return;
    g_xinput_modules.push_back(name);
}

std::string StatusJson() {
    Lock l;
    std::string r = "{\"ok\":true,\"version\":1,\"arch\":\"";
#ifdef _WIN64
    r += "x64";
#else
    r += "x86";
#endif
    r += std::string("\",\"load_mode\":\"") + g_load_mode + "\",\"pid\":" + std::to_string(GetCurrentProcessId());
    auto flag = [](bool v) { return v ? "true" : "false"; };
    r += std::string(",\"hooks\":{\"xinput\":") + flag(g_features.xinput) + ",\"focus\":" + flag(g_features.focus) +
         ",\"names\":" + flag(g_features.names) + ",\"cow\":" + flag(g_features.cow) +
         ",\"saves\":" + flag(g_features.saves) + ",\"feed\":" + flag(g_features.feed) +
         ",\"kbm\":" + flag(g_features.kbm) + ",\"children\":" + flag(g_features.children) +
         ",\"pads\":" + flag(g_features.pads) + "}";
    r += ",\"counters\":{";
    for (int i = 0; i < C_COUNT; ++i) {
        if (i) r += ",";
        r += std::string("\"") + kCounterNames[i] + "\":" + std::to_string(g_counters[i].load());
    }
    r += "},\"xinput_modules\":[";
    for (size_t i = 0; i < g_xinput_modules.size(); ++i) {
        if (i) r += ",";
        r += "\"" + JsonEscape(g_xinput_modules[i]) + "\"";
    }
    r += "],\"last_vibration\":[";
    for (int i = 0; i < 4; ++i) {
        if (i) r += ",";
        r += "[" + std::to_string(g_vibration[i][0]) + "," + std::to_string(g_vibration[i][1]) + "]";
    }
    r += "],\"notes\":[";
    for (size_t i = 0; i < g_notes.size(); ++i) {
        if (i) r += ",";
        r += "\"" + JsonEscape(g_notes[i]) + "\"";
    }
    r += "],\"children\":";
    r += ChildrenJson();
    r += "}";
    return r;
}

std::wstring Widen(const std::string& s) {
    if (s.empty()) return std::wstring();
    int n = MultiByteToWideChar(CP_UTF8, 0, s.data(), static_cast<int>(s.size()), nullptr, 0);
    std::wstring w(static_cast<size_t>(n), L'\0');
    MultiByteToWideChar(CP_UTF8, 0, s.data(), static_cast<int>(s.size()), &w[0], n);
    return w;
}

std::string Narrow(const std::wstring& w) {
    if (w.empty()) return std::string();
    int n = WideCharToMultiByte(CP_UTF8, 0, w.data(), static_cast<int>(w.size()), nullptr, 0, nullptr, nullptr);
    std::string s(static_cast<size_t>(n), '\0');
    WideCharToMultiByte(CP_UTF8, 0, w.data(), static_cast<int>(w.size()), &s[0], n, nullptr, nullptr);
    return s;
}

std::wstring NormalizePath(const wchar_t* path) {
    if (!path || !*path) return std::wstring();
    // Device paths, NT paths and pipes are not file system paths we manage.
    if (wcsncmp(path, L"\\\\.\\", 4) == 0 || wcsncmp(path, L"\\??\\", 4) == 0) return std::wstring();
    const wchar_t* p = path;
    if (wcsncmp(p, L"\\\\?\\UNC\\", 8) == 0) return std::wstring();
    if (wcsncmp(p, L"\\\\?\\", 4) == 0) p += 4;
    wchar_t stack_buf[MAX_PATH];
    DWORD n = GetFullPathNameW(p, MAX_PATH, stack_buf, nullptr);
    std::wstring out;
    if (n == 0) return out;
    if (n < MAX_PATH) {
        out.assign(stack_buf, n);
    } else {
        out.resize(n);
        DWORD m = GetFullPathNameW(p, n, &out[0], nullptr);
        if (m == 0 || m >= n) return std::wstring();
        out.resize(m);
    }
    while (out.size() > 3 && out.back() == L'\\') out.pop_back();
    return out;
}

bool PathUnder(const std::wstring& path, const std::wstring& prefix) {
    if (prefix.empty() || path.size() < prefix.size()) return false;
    if (CompareStringOrdinal(path.c_str(), static_cast<int>(prefix.size()), prefix.c_str(),
                             static_cast<int>(prefix.size()), TRUE) != CSTR_EQUAL)
        return false;
    if (path.size() == prefix.size()) return true;
    if (prefix.back() == L'\\') return true;
    return path[prefix.size()] == L'\\';
}

bool InstallHookAddress(void* target, void* detour, void** original, const char* label) {
    if (!target) {
        NoteHookFailure(label);
        return false;
    }
    MH_STATUS s = MH_CreateHook(target, detour, original);
    if (s == MH_ERROR_ALREADY_CREATED) return true;
    if (s != MH_OK) {
        Note(std::string("hook failed: ") + label + " (" + MH_StatusToString(s) + ")");
        return false;
    }
    if (MH_QueueEnableHook(target) != MH_OK) {
        NoteHookFailure(label);
        return false;
    }
    return true;
}

bool InstallHook(HMODULE module, const char* name, void* detour, void** original) {
    if (!module) {
        NoteHookFailure(name);
        return false;
    }
    return InstallHookAddress(reinterpret_cast<void*>(GetProcAddress(module, name)), detour, original, name);
}

void ApplyHooks() {
    MH_STATUS s = MH_ApplyQueued();
    if (s != MH_OK) Note(std::string("apply failed: ") + MH_StatusToString(s));
}

}  // namespace cl
