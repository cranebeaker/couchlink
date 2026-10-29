// Couchlink input hook entry point.
//
// The host (couchlink-hook, Rust) creates the pipe \\.\pipe\couchlink-hook-<pid> and then loads this DLL into the
// suspended copy. DllMain only starts a thread (no work under the loader lock); the thread connects to the pipe,
// waits for one {"cmd":"config", ...} line, installs the hooks, and from then on answers one-line JSON requests
// ("status", "ping") until the host closes the pipe.
#include <algorithm>

#include "common.h"

#ifdef CL_PROXY
// Fills the table the stand-in's jump stubs use with the real DLL's functions (src/proxy_forward.inl).
extern "C" void ClProxyForwardInit();
#endif

namespace cl {
namespace {

std::atomic<bool> g_configured{false};
HMODULE g_self = nullptr;

bool WriteAll(HANDLE h, const std::string& line) {
    std::string data = line + "\n";
    size_t off = 0;
    while (off < data.size()) {
        DWORD wrote = 0;
        if (!WriteFile(h, data.data() + off, static_cast<DWORD>(data.size() - off), &wrote, nullptr) || wrote == 0)
            return false;
        off += wrote;
    }
    return true;
}

bool ReadLine(HANDLE h, std::string& pending, std::string& line) {
    for (;;) {
        size_t nl = pending.find('\n');
        if (nl != std::string::npos) {
            line = pending.substr(0, nl);
            pending.erase(0, nl + 1);
            if (!line.empty() && line.back() == '\r') line.pop_back();
            return true;
        }
        char buf[4096];
        DWORD got = 0;
        if (!ReadFile(h, buf, sizeof(buf), &got, nullptr) || got == 0) return false;
        pending.append(buf, got);
    }
}

std::string Fail(const std::string& msg) { return "{\"ok\":false,\"error\":\"" + JsonEscape(msg) + "\"}"; }

void ParseTestPads(const Json& arr, Config& cfg) {
    cfg.use_test_pads = true;
    for (const Json& p : arr.a) {
        int64_t idx = p.get("index") ? p.get("index")->asInt(-1) : -1;
        if (idx < 0 || idx > 3) continue;
        TestPad& t = cfg.test_pads[idx];
        auto num = [&](const char* k, int64_t def) { return p.get(k) ? p.get(k)->asInt(def) : def; };
        t.present = true;
        t.buttons = static_cast<uint16_t>(num("buttons", 0));
        t.lt = static_cast<uint8_t>(num("lt", 0));
        t.rt = static_cast<uint8_t>(num("rt", 0));
        t.lx = static_cast<int16_t>(num("lx", 0));
        t.ly = static_cast<int16_t>(num("ly", 0));
        t.rx = static_cast<int16_t>(num("rx", 0));
        t.ry = static_cast<int16_t>(num("ry", 0));
        t.packet = static_cast<uint32_t>(num("packet", 1));
    }
}

bool ParseConfig(const Json& j, Config& cfg, std::string& err) {
    if (!j.isObj()) {
        err = "config is not an object";
        return false;
    }
    if (const Json* v = j.get("xinput")) cfg.xinput = v->asBool(true);
    if (const Json* v = j.get("pad")) cfg.pad = v->isNum() ? static_cast<int>(v->asInt(-1)) : -1;
    if (cfg.pad > 3 || cfg.pad < -1) {
        err = "pad index must be 0 to 3";
        return false;
    }
    if (const Json* v = j.get("focus")) cfg.focus = v->asBool(false);
    if (const Json* v = j.get("locks"); v && v->isArr()) {
        for (const Json& s : v->a) {
            if (!s.isStr() || s.s.empty()) continue;
            if (s.s == "*") cfg.locks_all = true;
            else cfg.locks.push_back(Widen(s.s));
        }
    }
    if (const Json* v = j.get("cow_root"); v && v->isStr() && !v->s.empty()) {
        cfg.cow_root = NormalizePath(Widen(v->s).c_str());
        if (cfg.cow_root.empty()) {
            err = "cow_root is not a usable path";
            return false;
        }
    }
    if (const Json* v = j.get("saves"); v && v->isArr()) {
        for (const Json& pair : v->a) {
            if (!pair.isArr() || pair.a.size() != 2 || !pair.a[0].isStr() || !pair.a[1].isStr()) {
                err = "saves entries must be [from, to]";
                return false;
            }
            SavePair sp;
            sp.from = NormalizePath(Widen(pair.a[0].s).c_str());
            sp.to = NormalizePath(Widen(pair.a[1].s).c_str());
            if (sp.from.empty() || sp.to.empty()) {
                err = "saves paths must be absolute";
                return false;
            }
            cfg.saves.push_back(std::move(sp));
        }
        std::stable_sort(cfg.saves.begin(), cfg.saves.end(),
                         [](const SavePair& a, const SavePair& b) { return a.from.size() > b.from.size(); });
    }
    if (const Json* v = j.get("test_pads"); v && v->isArr()) ParseTestPads(*v, cfg);
    if (const Json* v = j.get("feed"); v && v->isStr() && !v->s.empty()) cfg.feed = Widen(v->s);
    if (const Json* v = j.get("kbm_feed"); v && v->isStr() && !v->s.empty()) cfg.kbm_feed = Widen(v->s);
    if (const Json* v = j.get("fake_cursor")) cfg.fake_cursor = v->asBool(false);
    if (const Json* v = j.get("hook_children")) cfg.hook_children = v->asBool(false);
    if (const Json* v = j.get("copy_id")) cfg.copy_id = v->isNum() ? static_cast<uint32_t>(v->asInt(0)) : 0;
    if (const Json* v = j.get("depth")) cfg.depth = v->isNum() ? static_cast<uint32_t>(v->asInt(0)) : 0;
    cfg.raw = j;
    return true;
}

// Maps the pad feed section the daemon created. Returns an error text, empty on success.
std::string OpenFeed(const std::wstring& name) {
    HANDLE m = OpenFileMappingW(FILE_MAP_ALL_ACCESS, FALSE, name.c_str());
    if (!m) return "pad feed section not found (error " + std::to_string(GetLastError()) + "): " + Narrow(name);
    void* view = MapViewOfFile(m, FILE_MAP_ALL_ACCESS, 0, 0, kFeedSize);
    CloseHandle(m);  // the view keeps the section alive
    if (!view) return "cannot map the pad feed (error " + std::to_string(GetLastError()) + ")";
    const FeedBlock* b = static_cast<const FeedBlock*>(view);
    if (b->magic != kFeedMagic || b->version != kFeedVersion) {
        UnmapViewOfFile(view);
        return "pad feed has an unknown layout";
    }
    g_feed = static_cast<volatile FeedBlock*>(view);
    return std::string();
}

// Maps the keyboard and mouse feed section the daemon created. Returns an error text, empty on success.
std::string OpenKbmFeed(const std::wstring& name) {
    HANDLE m = OpenFileMappingW(FILE_MAP_ALL_ACCESS, FALSE, name.c_str());
    if (!m) return "kbm feed section not found (error " + std::to_string(GetLastError()) + "): " + Narrow(name);
    void* view = MapViewOfFile(m, FILE_MAP_ALL_ACCESS, 0, 0, kKbmTotalSize);
    CloseHandle(m);
    if (!view) return "cannot map the kbm feed (error " + std::to_string(GetLastError()) + ")";
    const KbmHeader* h = static_cast<const KbmHeader*>(view);
    if (h->magic != kKbmMagic || h->version != kKbmVersion || h->capacity != kKbmRingCap) {
        UnmapViewOfFile(view);
        return "kbm feed has an unknown layout";
    }
    g_kbm = static_cast<volatile KbmHeader*>(view);
    return std::string();
}

std::string ApplyConfig(const Json& c);

std::string Handle(const std::string& line) {
    Json req;
    std::string err;
    if (!ParseJson(line, req, err)) return Fail("bad json: " + err);
    const Json* cmd = req.get("cmd");
    if (!cmd || !cmd->isStr()) return Fail("missing cmd");
    if (cmd->s == "ping") return "{\"ok\":true}";
    if (cmd->s == "status") return StatusJson();
    if (cmd->s == "config") {
        const Json* c = req.get("config");
        if (!c) return Fail("missing config");
        return ApplyConfig(*c);
    }
    return Fail("unknown cmd");
}

// Parses a config object, maps its feeds and installs the hooks. Used by the pipe ("config" command, injection
// mode) and by the config file (stand-in DLL mode). Returns the status JSON, or a failure reply.
std::string ApplyConfig(const Json& c) {
    std::string err;
    if (g_configured.exchange(true)) return Fail("already configured");
    // One hook per process: a second copy of this DLL (a stand-in and an injected one, say) stays out of the way.
    wchar_t guard[96];
    wsprintfW(guard, L"Local\\couchlink-hook-active-%lu", GetCurrentProcessId());
    HANDLE once = CreateEventW(nullptr, TRUE, FALSE, guard);
    if (once && GetLastError() == ERROR_ALREADY_EXISTS) {
        CloseHandle(once);
        g_configured = false;
        return Fail("another copy of the hook is already active in this process");
    }
    Config cfg;
    if (!ParseConfig(c, cfg, err)) {
        g_configured = false;
        return Fail(err);
    }
    g_cfg = cfg;
    if (cfg.xinput && !cfg.feed.empty()) {
        std::string ferr = OpenFeed(cfg.feed);
        if (!ferr.empty()) {
            g_configured = false;
            return Fail(ferr);
        }
        g_features.feed = true;
    }
    if (!cfg.kbm_feed.empty()) {
        std::string kerr = OpenKbmFeed(cfg.kbm_feed);
        if (!kerr.empty()) {
            g_configured = false;
            return Fail(kerr);
        }
    }
    MH_STATUS s = MH_Initialize();
    if (s != MH_OK && s != MH_ERROR_ALREADY_INITIALIZED) return Fail(std::string("MinHook init: ") + MH_StatusToString(s));
    InstallXInput();
    InstallFocus();
    InstallNames();
    InstallFiles();
    InstallKbm();
    InstallMessages();
    InstallChildren();
    ApplyHooks();
    return StatusJson();
}

DWORD WINAPI PipeThread(LPVOID) {
    wchar_t name[96];
    wsprintfW(name, L"\\\\.\\pipe\\couchlink-hook-%lu", GetCurrentProcessId());
    HANDLE pipe = INVALID_HANDLE_VALUE;
    // The host creates the pipe before loading the DLL; retry briefly anyway.
    for (int i = 0; i < 200 && pipe == INVALID_HANDLE_VALUE; ++i) {
        pipe = CreateFileW(name, GENERIC_READ | GENERIC_WRITE, 0, nullptr, OPEN_EXISTING, 0, nullptr);
        if (pipe == INVALID_HANDLE_VALUE) {
            if (GetLastError() == ERROR_PIPE_BUSY) WaitNamedPipeW(name, 100);
            else Sleep(50);
        }
    }
    if (pipe == INVALID_HANDLE_VALUE) return 1;
    std::string pending, line;
    while (ReadLine(pipe, pending, line)) {
        if (line.empty()) continue;
        if (!WriteAll(pipe, Handle(line))) break;
    }
    CloseHandle(pipe);
    return 0;
}

#ifdef CL_PROXY
// Stand-in DLL mode: nothing connects to us, so we are the pipe server. A status client (the daemon) connects when
// it wants to know what the hook did; one client at a time, any number of times.
DWORD WINAPI ServerThread(LPVOID) {
    wchar_t name[96];
    wsprintfW(name, L"\\\\.\\pipe\\couchlink-hook-%lu", GetCurrentProcessId());
    for (;;) {
        HANDLE pipe = CreateNamedPipeW(name, PIPE_ACCESS_DUPLEX, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, 1,
                                       65536, 65536, 0, nullptr);
        if (pipe == INVALID_HANDLE_VALUE) return 1;
        if (ConnectNamedPipe(pipe, nullptr) || GetLastError() == ERROR_PIPE_CONNECTED) {
            std::string pending, line;
            while (ReadLine(pipe, pending, line)) {
                if (line.empty()) continue;
                if (!WriteAll(pipe, Handle(line))) break;
            }
            DisconnectNamedPipe(pipe);
        }
        CloseHandle(pipe);
    }
}

// Reads the whole config file named by COUCHLINK_HOOK_CONFIG. Empty when there is no such variable or file:
// then the stand-in does nothing at all and the game runs against the real DLL as if it were not here.
std::string ReadConfigFile() {
    wchar_t path[4096];
    DWORD n = GetEnvironmentVariableW(L"COUCHLINK_HOOK_CONFIG", path, 4096);
    if (n == 0 || n >= 4096) return std::string();
    HANDLE f = CreateFileW(path, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr, OPEN_EXISTING,
                           FILE_ATTRIBUTE_NORMAL, nullptr);
    if (f == INVALID_HANDLE_VALUE) return std::string();
    std::string text;
    char buf[4096];
    DWORD got = 0;
    while (text.size() < (1u << 20) && ReadFile(f, buf, sizeof(buf), &got, nullptr) && got) text.append(buf, got);
    CloseHandle(f);
    return text;
}

// Runs under the loader lock, early in the game's start. Only reads a file and patches functions of modules that
// are already loaded (kernel32, user32 ... are our own imports); it never loads a library.
void StartFromFile() {
    std::string text = ReadConfigFile();
    if (text.empty()) return;
    Json req;
    std::string err;
    if (!ParseJson(text, req, err)) return;
    const Json* cfg = req.get("config");  // the same {"cmd":"config","config":{...}} message the pipe takes
    g_load_mode = "proxy";
    ApplyConfig(cfg ? *cfg : req);
    HANDLE t = CreateThread(nullptr, 0, ServerThread, nullptr, 0, nullptr);
    if (t) CloseHandle(t);
}
#endif

}  // namespace

HMODULE SelfModule() { return g_self; }

}  // namespace cl

BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) {
        cl::g_self = module;
        DisableThreadLibraryCalls(module);
#ifdef CL_PROXY
        ClProxyForwardInit();  // first: the game may call the real DLL's functions the moment we return
        cl::StartFromFile();
#else
        HANDLE t = CreateThread(nullptr, 0, cl::PipeThread, nullptr, 0, nullptr);
        if (t) CloseHandle(t);
#endif
    }
    return TRUE;
}
