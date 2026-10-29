// Child processes: when a hooked process starts another process (a launcher exe that starts the real game), the child
// is created suspended, run to its entry point, given this DLL with the same configuration, and then let go.
//
// The mechanism mirrors the host side (core/crates/couchlink-hook/src/sys.rs): the child's main thread is let run
// until it sits on its exe's entry point (a `jmp $` is written there for that moment), so every static import is
// loaded and none of the child's own code has run; LoadLibraryW is started in the child with CreateRemoteThread;
// this process then plays the host: it serves the pipe \\.\pipe\couchlink-hook-<child pid>, sends the config line and
// reads the reply. The parent keeps the pipe so its own status report can include the child's status.
//
// Config handed to the child = the config this process received, with `copy_id` (the pid of the first hooked process
// of the copy, so renamed locks agree between launcher and game) and `depth` + 1. Everything else repeats, because
// the child belongs to the same seat: same pad feed, same kbm feed, same save redirection.
// Not covered: a 32-bit parent starting a 64-bit child, processes started by a kernel driver or by another process
// (services, shell), and programs under %SystemRoot% (cmd.exe, werfault, ...) which are left alone on purpose.
#include <cstring>

#include "common.h"

#include <tlhelp32.h>

namespace cl {

HMODULE SelfModule();  // dllmain.cpp

namespace {

#ifdef _WIN64
constexpr bool kSelf64 = true;
#else
constexpr bool kSelf64 = false;
#endif

struct Child {
    DWORD pid = 0;
    bool hooked = false;
    std::string arch;
    std::string error;
    HANDLE pipe = INVALID_HANDLE_VALUE;
};

CRITICAL_SECTION g_lock;
INIT_ONCE g_lock_once = INIT_ONCE_STATIC_INIT;
std::vector<Child> g_children;
thread_local bool t_in_create = false;

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

BOOL(WINAPI* o_CreateProcessW)(LPCWSTR, LPWSTR, LPSECURITY_ATTRIBUTES, LPSECURITY_ATTRIBUTES, BOOL, DWORD, LPVOID, LPCWSTR,
                               LPSTARTUPINFOW, LPPROCESS_INFORMATION) = nullptr;
BOOL(WINAPI* o_CreateProcessA)(LPCSTR, LPSTR, LPSECURITY_ATTRIBUTES, LPSECURITY_ATTRIBUTES, BOOL, DWORD, LPVOID, LPCSTR,
                               LPSTARTUPINFOA, LPPROCESS_INFORMATION) = nullptr;

typedef LONG(NTAPI* NtQueryInformationProcessFn)(HANDLE, ULONG, PVOID, ULONG, PULONG);

struct ProcBasicInfo {
    PVOID exit_status;
    PVOID peb;
    PVOID affinity;
    PVOID priority;
    PVOID pid;
    PVOID parent;
};

bool ReadMem(HANDLE p, uintptr_t addr, void* buf, size_t n) {
    SIZE_T got = 0;
    return ReadProcessMemory(p, reinterpret_cast<LPCVOID>(addr), buf, n, &got) && got == n;
}
bool WriteMem(HANDLE p, uintptr_t addr, const void* buf, size_t n) {
    SIZE_T put = 0;
    return WriteProcessMemory(p, reinterpret_cast<LPVOID>(addr), buf, n, &put) && put == n;
}
uint32_t Le32(const uint8_t* b) { return b[0] | (b[1] << 8) | (b[2] << 16) | (static_cast<uint32_t>(b[3]) << 24); }

// Entry point of the child's exe. Empty error on success.
std::string EntryPoint(HANDLE p, bool child32, uintptr_t& entry) {
    static NtQueryInformationProcessFn nt =
        reinterpret_cast<NtQueryInformationProcessFn>(GetProcAddress(GetModuleHandleW(L"ntdll.dll"), "NtQueryInformationProcess"));
    if (!nt) return "NtQueryInformationProcess is missing";
    uintptr_t base = 0;
    if (child32) {
        ULONG_PTR peb32 = 0;
        LONG st = nt(p, 26 /* ProcessWow64Information */, &peb32, sizeof(peb32), nullptr);
        if (st != 0 || !peb32) return "no 32-bit PEB (status " + std::to_string(st) + ")";
        uint32_t b = 0;
        if (!ReadMem(p, peb32 + 8, &b, 4)) return "cannot read the 32-bit PEB";
        base = b;
    } else {
        ProcBasicInfo pbi = {};
        LONG st = nt(p, 0 /* ProcessBasicInformation */, &pbi, sizeof(pbi), nullptr);
        if (st != 0 || !pbi.peb) return "no PEB (status " + std::to_string(st) + ")";
        uint64_t b = 0;
        if (!ReadMem(p, reinterpret_cast<uintptr_t>(pbi.peb) + 0x10, &b, 8)) return "cannot read the PEB";
        base = static_cast<uintptr_t>(b);
    }
    uint8_t dos[0x40];
    if (!ReadMem(p, base, dos, sizeof(dos)) || dos[0] != 'M' || dos[1] != 'Z') return "no MZ header";
    uint8_t hdr[0x30];
    if (!ReadMem(p, base + Le32(dos + 0x3c), hdr, sizeof(hdr)) || memcmp(hdr, "PE\0\0", 4) != 0) return "no PE header";
    uint32_t rva = Le32(hdr + 24 + 16);
    if (!rva) return "no entry point";
    entry = base + rva;
    return std::string();
}

bool ThreadIp(HANDLE thread, bool child32, uintptr_t& ip) {
#ifdef _WIN64
    if (child32) {
        WOW64_CONTEXT ctx = {};
        ctx.ContextFlags = WOW64_CONTEXT_CONTROL;
        if (!Wow64GetThreadContext(thread, &ctx)) return false;
        ip = ctx.Eip;
        return true;
    }
    CONTEXT ctx = {};
    ctx.ContextFlags = CONTEXT_CONTROL;
    if (!GetThreadContext(thread, &ctx)) return false;
    ip = static_cast<uintptr_t>(ctx.Rip);
    return true;
#else
    (void)child32;
    CONTEXT ctx = {};
    ctx.ContextFlags = CONTEXT_CONTROL;
    if (!GetThreadContext(thread, &ctx)) return false;
    ip = ctx.Eip;
    return true;
#endif
}

// Runs the main thread to the entry point and leaves it suspended there (suspend count 1 again).
std::string RunToEntry(HANDLE p, HANDLE thread, bool child32) {
    uintptr_t entry = 0;
    std::string err = EntryPoint(p, child32, entry);
    if (!err.empty()) return err;
    uint8_t original[2];
    if (!ReadMem(p, entry, original, 2)) return "cannot read the entry point";
    DWORD old = 0;
    if (!VirtualProtectEx(p, reinterpret_cast<LPVOID>(entry), 2, PAGE_EXECUTE_READWRITE, &old)) return "cannot unprotect the entry point";
    const uint8_t spin[2] = {0xEB, 0xFE};  // jmp $
    if (!WriteMem(p, entry, spin, 2)) return "cannot patch the entry point";
    FlushInstructionCache(p, reinterpret_cast<LPCVOID>(entry), 2);
    if (ResumeThread(thread) == static_cast<DWORD>(-1)) return "ResumeThread failed";
    ULONGLONG deadline = GetTickCount64() + 20000;
    std::string result;
    for (;;) {
        Sleep(3);
        if (WaitForSingleObject(p, 0) == WAIT_OBJECT_0) {
            result = "the child exited before reaching its entry point";
            break;
        }
        if (SuspendThread(thread) == static_cast<DWORD>(-1)) {
            result = "SuspendThread failed";
            break;
        }
        uintptr_t ip = 0;
        if (ThreadIp(thread, child32, ip) && ip == entry) break;
        ResumeThread(thread);
        if (GetTickCount64() > deadline) {
            result = "the child did not reach its entry point";
            break;
        }
    }
    WriteMem(p, entry, original, 2);
    DWORD ignored = 0;
    VirtualProtectEx(p, reinterpret_cast<LPVOID>(entry), 2, old, &ignored);
    FlushInstructionCache(p, reinterpret_cast<LPCVOID>(entry), 2);
    return result;
}

// Address of LoadLibraryW in the child. Same bitness: ours. Otherwise read the export table of the child's kernel32.
uintptr_t RemoteLoadLibrary(HANDLE p, DWORD pid, bool child32) {
    if (child32 == !kSelf64) return reinterpret_cast<uintptr_t>(GetProcAddress(GetModuleHandleW(L"kernel32.dll"), "LoadLibraryW"));
    uintptr_t base = 0;
    for (int tries = 0; tries < 100 && !base; ++tries) {
        HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
        if (snap == INVALID_HANDLE_VALUE) {
            Sleep(10);
            continue;
        }
        MODULEENTRY32W me;
        me.dwSize = sizeof(me);
        for (BOOL ok = Module32FirstW(snap, &me); ok; ok = Module32NextW(snap, &me)) {
            if (_wcsicmp(me.szModule, L"kernel32.dll") == 0) base = reinterpret_cast<uintptr_t>(me.modBaseAddr);
        }
        CloseHandle(snap);
        if (!base) Sleep(10);
    }
    if (!base) return 0;
    std::vector<uint8_t> head(0x1000);
    if (!ReadMem(p, base, head.data(), head.size())) return 0;
    uint32_t nt = Le32(head.data() + 0x3c);
    if (nt + 0x90 > head.size() || memcmp(head.data() + nt, "PE\0\0", 4) != 0) return 0;
    uint32_t opt = nt + 24;
    uint16_t magic = static_cast<uint16_t>(head[opt] | (head[opt + 1] << 8));
    uint32_t dd = magic == 0x20b ? opt + 112 : opt + 96;
    uint32_t exp_rva = Le32(head.data() + dd), exp_size = Le32(head.data() + dd + 4);
    if (!exp_rva || exp_size < 40) return 0;
    std::vector<uint8_t> exp(exp_size);
    if (!ReadMem(p, base + exp_rva, exp.data(), exp.size())) return 0;
    uint32_t n_names = Le32(exp.data() + 24), funcs = Le32(exp.data() + 28), names = Le32(exp.data() + 32), ords = Le32(exp.data() + 36);
    auto rd = [&](uint32_t rva, void* out, size_t len) {
        if (rva >= exp_rva && rva + len <= static_cast<size_t>(exp_rva) + exp_size) {
            memcpy(out, exp.data() + (rva - exp_rva), len);
            return true;
        }
        return ReadMem(p, base + rva, out, len);
    };
    const char want[] = "LoadLibraryW";
    for (uint32_t i = 0; i < n_names; ++i) {
        uint8_t name_rva_b[4];
        if (!rd(names + i * 4, name_rva_b, 4)) continue;
        char buf[sizeof(want)];
        if (!rd(Le32(name_rva_b), buf, sizeof(buf)) || memcmp(buf, want, sizeof(want)) != 0) continue;
        uint8_t ob[2], fb[4];
        if (!rd(ords + i * 2, ob, 2)) return 0;
        uint16_t ord = static_cast<uint16_t>(ob[0] | (ob[1] << 8));
        if (!rd(funcs + ord * 4, fb, 4)) return 0;
        uint32_t f = Le32(fb);
        if (f >= exp_rva && f < exp_rva + exp_size) return 0;  // forwarded export
        return base + f;
    }
    return 0;
}

std::wstring DllPathFor(bool child32) {
    wchar_t self[MAX_PATH * 2];
    DWORD n = GetModuleFileNameW(SelfModule(), self, ARRAYSIZE(self));
    if (n == 0 || n >= ARRAYSIZE(self)) return std::wstring();
    std::wstring path(self, n);
    if (child32 == !kSelf64) return path;
    // The other architecture lives in the sibling folder (hook/build/x64 <-> hook/build/x86).
    size_t slash = path.find_last_of(L'\\');
    if (slash == std::wstring::npos) return std::wstring();
    size_t slash2 = path.find_last_of(L'\\', slash - 1);
    if (slash2 == std::wstring::npos) return std::wstring();
    std::wstring other = path.substr(0, slash2 + 1) + (child32 ? L"x86" : L"x64") + path.substr(slash);
    return GetFileAttributesW(other.c_str()) == INVALID_FILE_ATTRIBUTES ? std::wstring() : other;
}

std::string LoadInChild(HANDLE p, DWORD pid, bool child32, const std::wstring& dll) {
    uintptr_t load = RemoteLoadLibrary(p, pid, child32);
    if (!load) return "cannot find LoadLibraryW in the child";
    size_t bytes = (dll.size() + 1) * sizeof(wchar_t);
    LPVOID remote = VirtualAllocEx(p, nullptr, bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    if (!remote) return "VirtualAllocEx failed";
    std::string err;
    if (!WriteMem(p, reinterpret_cast<uintptr_t>(remote), dll.c_str(), bytes)) {
        err = "cannot write the DLL path";
    } else {
        HANDLE t = CreateRemoteThread(p, nullptr, 0, reinterpret_cast<LPTHREAD_START_ROUTINE>(load), remote, 0, nullptr);
        if (!t) {
            err = "CreateRemoteThread failed (error " + std::to_string(GetLastError()) + ")";
        } else {
            if (WaitForSingleObject(t, 15000) == WAIT_TIMEOUT) err = "LoadLibrary in the child did not return";
            CloseHandle(t);
        }
    }
    // After a timeout the child may still be reading the path: leave the page alone.
    if (err.find("did not return") == std::string::npos) VirtualFreeEx(p, remote, 0, MEM_RELEASE);
    return err;
}

bool PipeWrite(HANDLE h, const std::string& line) {
    std::string data = line + "\n";
    size_t off = 0;
    while (off < data.size()) {
        DWORD wrote = 0;
        if (!WriteFile(h, data.data() + off, static_cast<DWORD>(data.size() - off), &wrote, nullptr) || wrote == 0) return false;
        off += wrote;
    }
    return true;
}

// Reads one line; false on timeout or a broken pipe.
bool PipeReadLine(HANDLE h, std::string& line, DWORD timeout_ms) {
    line.clear();
    ULONGLONG deadline = GetTickCount64() + timeout_ms;
    for (;;) {
        DWORD avail = 0;
        if (!PeekNamedPipe(h, nullptr, 0, nullptr, &avail, nullptr)) return false;
        while (avail > 0) {
            char c;
            DWORD got = 0;
            if (!ReadFile(h, &c, 1, &got, nullptr) || got == 0) return false;
            --avail;
            if (c == '\n') {
                if (!line.empty() && line.back() == '\r') line.pop_back();
                return true;
            }
            line += c;
        }
        if (GetTickCount64() > deadline) return false;
        Sleep(2);
    }
}

// Serves the config on the child's pipe. Returns an error text, empty on success; `pipe_out` is the open pipe.
std::string Configure(DWORD pid, HANDLE& pipe_out) {
    wchar_t name[96];
    wsprintfW(name, L"\\\\.\\pipe\\couchlink-hook-%lu", pid);
    HANDLE pipe = CreateNamedPipeW(name, PIPE_ACCESS_DUPLEX, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | 8 /* reject remote */, 1,
                                   64 * 1024, 64 * 1024, 0, nullptr);
    if (pipe == INVALID_HANDLE_VALUE) return "cannot create the child's pipe";
    pipe_out = pipe;
    return std::string();
}

std::string Handshake(HANDLE p, HANDLE pipe, DWORD timeout_ms) {
    ULONGLONG deadline = GetTickCount64() + timeout_ms;
    for (;;) {
        BOOL ok = ConnectNamedPipe(pipe, nullptr);
        if (ok || GetLastError() == ERROR_PIPE_CONNECTED) break;
        if (WaitForSingleObject(p, 0) == WAIT_OBJECT_0) return "the child exited while the hook was loading";
        if (GetTickCount64() > deadline) return "the hook DLL did not connect in the child";
        Sleep(5);
    }
    DWORD mode = PIPE_WAIT | PIPE_READMODE_BYTE;
    SetNamedPipeHandleState(pipe, &mode, nullptr, nullptr);
    if (!PipeWrite(pipe, ChildConfigLine())) return "cannot send the config to the child";
    std::string reply;
    if (!PipeReadLine(pipe, reply, 15000)) return "the child's hook did not answer";
    Json j;
    std::string perr;
    if (!ParseJson(reply, j, perr)) return "bad reply from the child's hook";
    const Json* ok = j.get("ok");
    if (!ok || !ok->asBool(false)) {
        const Json* e = j.get("error");
        return "the child's hook rejected the config: " + (e && e->isStr() ? e->s : reply);
    }
    return std::string();
}

bool UnderWindowsDir(HANDLE p) {
    wchar_t image[MAX_PATH * 2];
    DWORD n = ARRAYSIZE(image);
    if (!QueryFullProcessImageNameW(p, 0, image, &n)) return false;
    wchar_t win[MAX_PATH];
    UINT w = GetWindowsDirectoryW(win, ARRAYSIZE(win));
    if (w == 0 || w >= ARRAYSIZE(win)) return false;
    return PathUnder(std::wstring(image, n), std::wstring(win, w));
}

void Record(Child c) {
    Lock l;
    if (g_children.size() >= 64) {
        if (g_children.front().pipe != INVALID_HANDLE_VALUE) CloseHandle(g_children.front().pipe);
        g_children.erase(g_children.begin());
    }
    g_children.push_back(std::move(c));
}

// Hooks one freshly created (suspended) child. `caller_suspended`: the caller asked for CREATE_SUSPENDED itself.
void HookChild(const PROCESS_INFORMATION& pi, bool caller_suspended) {
    Child rec;
    rec.pid = pi.dwProcessId;
    std::string err;
    BOOL wow = FALSE;
    IsWow64Process(pi.hProcess, &wow);
    bool skip = false;
    bool child32;
    if (kSelf64) {
        child32 = wow != FALSE;
    } else {
        BOOL self_wow = FALSE;
        IsWow64Process(GetCurrentProcess(), &self_wow);
        child32 = !self_wow || wow;
        if (self_wow && !wow) err = "a 32-bit process cannot hook a 64-bit child";
    }
    rec.arch = child32 ? "x86" : "x64";
    if (err.empty() && UnderWindowsDir(pi.hProcess)) {
        skip = true;
        Bump(C_CHILDREN_SKIPPED);
    }
    HANDLE pipe = INVALID_HANDLE_VALUE;
    if (err.empty() && !skip) {
        std::wstring dll = DllPathFor(child32);
        if (dll.empty()) err = "the " + rec.arch + " hook DLL was not found next to this one";
        if (err.empty()) err = Configure(pi.dwProcessId, pipe);
        if (err.empty()) err = RunToEntry(pi.hProcess, pi.hThread, child32);
        if (err.empty()) err = LoadInChild(pi.hProcess, pi.dwProcessId, child32, dll);
        if (err.empty()) err = Handshake(pi.hProcess, pipe, 15000);
        if (err.empty()) {
            rec.hooked = true;
            rec.pipe = pipe;
            Bump(C_CHILDREN_HOOKED);
        } else {
            if (pipe != INVALID_HANDLE_VALUE) CloseHandle(pipe);
            Bump(C_CHILDREN_FAILED);
            Note("child " + std::to_string(pi.dwProcessId) + " not hooked: " + err);
        }
    } else if (!skip) {
        Bump(C_CHILDREN_FAILED);
        Note("child " + std::to_string(pi.dwProcessId) + " not hooked: " + err);
    }
    rec.error = err;
    if (!skip) Record(std::move(rec));
    // The process was started suspended by us: a caller that did not ask for that gets it running now. When we ran
    // the main thread to its entry point the suspend count is 1 again, otherwise it never left 1.
    if (!caller_suspended) ResumeThread(pi.hThread);
}

BOOL WINAPI d_CreateProcessW(LPCWSTR app, LPWSTR cmd, LPSECURITY_ATTRIBUTES pa, LPSECURITY_ATTRIBUTES ta, BOOL inherit, DWORD flags,
                             LPVOID env, LPCWSTR cwd, LPSTARTUPINFOW si, LPPROCESS_INFORMATION pi) {
    if (!g_cfg.hook_children || t_in_create || (flags & (DEBUG_PROCESS | DEBUG_ONLY_THIS_PROCESS)))
        return o_CreateProcessW(app, cmd, pa, ta, inherit, flags, env, cwd, si, pi);
    t_in_create = true;
    PROCESS_INFORMATION local = {};
    LPPROCESS_INFORMATION out = pi ? pi : &local;
    BOOL ok = o_CreateProcessW(app, cmd, pa, ta, inherit, flags | CREATE_SUSPENDED, env, cwd, si, out);
    if (ok) {
        HookChild(*out, (flags & CREATE_SUSPENDED) != 0);
        if (!pi) {
            CloseHandle(local.hProcess);
            CloseHandle(local.hThread);
        }
    }
    t_in_create = false;
    return ok;
}

BOOL WINAPI d_CreateProcessA(LPCSTR app, LPSTR cmd, LPSECURITY_ATTRIBUTES pa, LPSECURITY_ATTRIBUTES ta, BOOL inherit, DWORD flags,
                             LPVOID env, LPCSTR cwd, LPSTARTUPINFOA si, LPPROCESS_INFORMATION pi) {
    if (!g_cfg.hook_children || t_in_create || (flags & (DEBUG_PROCESS | DEBUG_ONLY_THIS_PROCESS)))
        return o_CreateProcessA(app, cmd, pa, ta, inherit, flags, env, cwd, si, pi);
    t_in_create = true;
    PROCESS_INFORMATION local = {};
    LPPROCESS_INFORMATION out = pi ? pi : &local;
    BOOL ok = o_CreateProcessA(app, cmd, pa, ta, inherit, flags | CREATE_SUSPENDED, env, cwd, si, out);
    if (ok) {
        HookChild(*out, (flags & CREATE_SUSPENDED) != 0);
        if (!pi) {
            CloseHandle(local.hProcess);
            CloseHandle(local.hThread);
        }
    }
    t_in_create = false;
    return ok;
}

}  // namespace

uint32_t CopyId() { return g_cfg.copy_id ? g_cfg.copy_id : GetCurrentProcessId(); }

std::string ChildConfigLine() {
    Json cfg = g_cfg.raw;
    cfg.set("copy_id", Json::number(CopyId()));
    cfg.set("depth", Json::number(g_cfg.depth + 1));
    Json msg;
    msg.set("cmd", Json::string("config"));
    msg.set("config", cfg);
    return JsonWrite(msg);
}

std::string ChildrenJson() {
    if (!g_features.children && g_children.empty()) return "[]";
    Lock l;
    std::string r = "[";
    for (size_t i = 0; i < g_children.size(); ++i) {
        Child& c = g_children[i];
        if (i) r += ",";
        r += "{\"pid\":" + std::to_string(c.pid) + ",\"hooked\":" + (c.hooked ? "true" : "false") + ",\"arch\":\"" + c.arch + "\"";
        if (!c.error.empty()) r += ",\"error\":\"" + JsonEscape(c.error) + "\"";
        if (c.hooked && c.pipe != INVALID_HANDLE_VALUE) {
            // Ask only children that are still running (an exited child's pipe is dead, and a stuck one costs a timeout).
            HANDLE proc = OpenProcess(SYNCHRONIZE, FALSE, c.pid);
            bool alive = proc && WaitForSingleObject(proc, 0) == WAIT_TIMEOUT;
            if (proc) CloseHandle(proc);
            r += std::string(",\"running\":") + (alive ? "true" : "false");
            std::string reply;
            if (alive && PipeWrite(c.pipe, "{\"cmd\":\"status\"}") && PipeReadLine(c.pipe, reply, 1500) && !reply.empty() && reply[0] == '{') {
                r += ",\"status\":" + reply;
            } else {
                r += ",\"status\":null";
            }
        }
        r += "}";
    }
    return r + "]";
}

void InstallChildren() {
    if (!g_cfg.hook_children) return;
    HMODULE k = GetModuleHandleW(L"kernelbase.dll");
    if (!k) k = GetModuleHandleW(L"kernel32.dll");
    InstallHook(k, "CreateProcessW", reinterpret_cast<void*>(&d_CreateProcessW), reinterpret_cast<void**>(&o_CreateProcessW));
    InstallHook(k, "CreateProcessA", reinterpret_cast<void*>(&d_CreateProcessA), reinterpret_cast<void**>(&o_CreateProcessA));
    g_features.children = true;
}

}  // namespace cl
