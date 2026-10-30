// Test target for the Couchlink input hook. Behaves like a tiny game: makes a window, reads XInput, takes a
// single-instance mutex, writes a hardlinked file and a save file, then dumps what it observed to a JSON file.
// Built for x64 and x86. It does nothing harmful: all paths come from the command line.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <shlobj.h>

#define DIRECTINPUT_VERSION 0x0800
#include <dinput.h>

#include <cstdio>
#include <setupapi.h>
#include <hidsdi.h>

#include <roapi.h>
#include <windows.gaming.input.h>
#include <wrl/client.h>
#include <wrl/wrappers/corewrappers.h>

// A static import of version.dll, so a stand-in version.dll next to this exe is loaded at process start.
#pragma comment(lib, "version.lib")
#include <cstdlib>
#include <string>
#include <vector>

namespace {

struct Args {
    std::wstring out;
    std::wstring wait_file;
    std::wstring xinput = L"1_4";  // "none" skips XInput
    std::wstring link_file;        // opened for append through CreateFileW
    std::wstring link_file_a;      // opened for append through CreateFileA
    std::wstring link_trunc;       // CREATE_ALWAYS through CreateFileW
    std::wstring save_dir;
    bool known_docs = false;
    bool no_window = false;
    bool use_version = false;  // call GetFileVersionInfoSizeW so version.dll is a loaded import
    bool pads = false;         // list the game controllers DirectInput, Raw Input and HID let this process see
    bool poll_xinput = false;  // keep reading XInput and rewrite the output file whenever it changes
    bool kbm = false;          // keyboard / mouse mode: log window messages, sample key and cursor state, until the wait file
    bool inject_real = false;  // post "real looking" (unsigned) keyboard and mouse messages to the own window
    bool rawinput = false;     // register raw input for mouse and keyboard and log WM_INPUT
    bool dinput = false;       // read DirectInput keyboard and mouse
    std::wstring cmd_file;     // kbm mode: lines like "setpos 10 20" run once each (calls that touch the real cursor)
    std::wstring spawn_child;  // start the child exe with --out <this file>, sharing --wait-file
    std::wstring child_exe;    // exe to start as the child (default: this exe)
    std::wstring child_args;   // appended to the child command line
    bool launcher = false;     // with --spawn-child: do not wait for the stop file, exit after starting the child
    DWORD timeout_ms = 30000;
};

std::string Utf8(const std::wstring& w) {
    if (w.empty()) return std::string();
    int n = WideCharToMultiByte(CP_UTF8, 0, w.data(), (int)w.size(), nullptr, 0, nullptr, nullptr);
    std::string s((size_t)n, '\0');
    WideCharToMultiByte(CP_UTF8, 0, w.data(), (int)w.size(), &s[0], n, nullptr, nullptr);
    return s;
}

std::string Esc(const std::string& s) {
    std::string r;
    for (unsigned char c : s) {
        if (c == '"') r += "\\\"";
        else if (c == '\\') r += "\\\\";
        else if (c < 0x20) r += ' ';
        else r += (char)c;
    }
    return r;
}

std::string Q(const std::wstring& w) { return "\"" + Esc(Utf8(w)) + "\""; }
std::string B(bool v) { return v ? "true" : "false"; }

// window message observations
struct Seen {
    bool activate_inactive = false;
    bool activateapp_false = false;
    bool killfocus = false;
    bool ncactivate_false = false;
    int activate_total = 0;
} g_seen;

// kbm mode observations
HWND g_wnd = nullptr;
bool g_log_input = false;
bool g_dirty = false;
std::vector<std::string> g_log;
void Log(const std::string& t) {
    if (g_log.size() < 600) g_log.push_back(t);
    g_dirty = true;
}
std::string Num(long long v) { return std::to_string(v); }

void LogRawInput(LPARAM l) {
    UINT size = 0;
    if (GetRawInputData((HRAWINPUT)l, RID_INPUT, nullptr, &size, sizeof(RAWINPUTHEADER)) != 0 || size == 0) {
        Log("RAWFAIL size");
        return;
    }
    std::vector<BYTE> buf(size);
    if (GetRawInputData((HRAWINPUT)l, RID_INPUT, buf.data(), &size, sizeof(RAWINPUTHEADER)) != size) {
        Log("RAWFAIL data");
        return;
    }
    const RAWINPUT* ri = (const RAWINPUT*)buf.data();
    if (ri->header.dwType == RIM_TYPEMOUSE) {
        Log("RAWM " + Num(ri->data.mouse.lLastX) + "," + Num(ri->data.mouse.lLastY) + " f=" + Num(ri->data.mouse.usButtonFlags) +
            " d=" + Num((short)ri->data.mouse.usButtonData) + " dev=" + Num((long long)(UINT_PTR)ri->header.hDevice));
    } else if (ri->header.dwType == RIM_TYPEKEYBOARD) {
        Log("RAWK make=" + Num(ri->data.keyboard.MakeCode) + " f=" + Num(ri->data.keyboard.Flags) + " vk=" + Num(ri->data.keyboard.VKey) +
            " dev=" + Num((long long)(UINT_PTR)ri->header.hDevice));
    }
}

void LogInputMessage(UINT m, WPARAM w, LPARAM l) {
    auto key = [&](const char* name) {
        Log(std::string(name) + " vk=" + Num((long long)w) + " sc=" + Num((l >> 16) & 0xFF) + " ext=" + Num((l >> 24) & 1) +
            " prev=" + Num((l >> 30) & 1) + " ctx=" + Num((l >> 29) & 1) + " lp=" + Num((unsigned long long)(l & 0xFFFFFFFF)));
    };
    auto mouse = [&](const char* name) { Log(std::string(name) + " " + Num((short)LOWORD(l)) + "," + Num((short)HIWORD(l)) + " k=" + Num(LOWORD(w))); };
    switch (m) {
        case WM_KEYDOWN: key("KD"); break;
        case WM_KEYUP: key("KU"); break;
        case WM_SYSKEYDOWN: key("SKD"); break;
        case WM_SYSKEYUP: key("SKU"); break;
        case WM_CHAR: Log("CH " + Num((long long)w) + " lp=" + Num((unsigned long long)(l & 0xFFFFFFFF))); break;
        case WM_SYSCHAR: Log("SCH " + Num((long long)w)); break;
        case WM_DEADCHAR: Log("DCH " + Num((long long)w)); break;
        case WM_MOUSEMOVE: mouse("MM"); break;
        case WM_LBUTTONDOWN: mouse("LD"); break;
        case WM_LBUTTONUP: mouse("LU"); break;
        case WM_RBUTTONDOWN: mouse("RD"); break;
        case WM_RBUTTONUP: mouse("RU"); break;
        case WM_MBUTTONDOWN: mouse("MD"); break;
        case WM_MBUTTONUP: mouse("MU"); break;
        case WM_XBUTTONDOWN: Log("XD " + Num(HIWORD(w))); break;
        case WM_XBUTTONUP: Log("XU " + Num(HIWORD(w))); break;
        case WM_MOUSEWHEEL: Log("WH " + Num((short)HIWORD(w)) + " k=" + Num(LOWORD(w))); break;
        case WM_MOUSEHWHEEL: Log("HWH " + Num((short)HIWORD(w))); break;
        case WM_INPUT: LogRawInput(l); break;
        default: break;
    }
}

LRESULT CALLBACK TestProc(HWND h, UINT m, WPARAM w, LPARAM l) {
    if (g_log_input) LogInputMessage(m, w, l);
    switch (m) {
        case WM_ACTIVATE:
            ++g_seen.activate_total;
            if (LOWORD(w) == WA_INACTIVE) g_seen.activate_inactive = true;
            break;
        case WM_ACTIVATEAPP:
            if (w == FALSE) g_seen.activateapp_false = true;
            break;
        case WM_KILLFOCUS: g_seen.killfocus = true; break;
        case WM_NCACTIVATE:
            if (w == FALSE) g_seen.ncactivate_false = true;
            break;
        default: break;
    }
    return DefWindowProcW(h, m, w, l);
}

void Pump(DWORD ms) {
    DWORD end = GetTickCount() + ms;
    MSG msg;
    do {
        while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        Sleep(5);
    } while ((int)(end - GetTickCount()) > 0);
}

bool FileExists(const std::wstring& p) { return GetFileAttributesW(p.c_str()) != INVALID_FILE_ATTRIBUTES; }

bool AppendTo(const std::wstring& path, const char* text, bool ansi, DWORD disp, DWORD* err) {
    HANDLE h;
    if (ansi) {
        int n = WideCharToMultiByte(CP_ACP, 0, path.c_str(), -1, nullptr, 0, nullptr, nullptr);
        std::string a((size_t)n, '\0');
        WideCharToMultiByte(CP_ACP, 0, path.c_str(), -1, &a[0], n, nullptr, nullptr);
        h = CreateFileA(a.c_str(), FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, disp, FILE_ATTRIBUTE_NORMAL, nullptr);
    } else if (disp == CREATE_ALWAYS) {
        h = CreateFileW(path.c_str(), GENERIC_WRITE, FILE_SHARE_READ, nullptr, disp, FILE_ATTRIBUTE_NORMAL, nullptr);
    } else {
        h = CreateFileW(path.c_str(), FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, disp, FILE_ATTRIBUTE_NORMAL, nullptr);
    }
    if (h == INVALID_HANDLE_VALUE) {
        *err = GetLastError();
        return false;
    }
    DWORD wrote = 0;
    BOOL ok = WriteFile(h, text, (DWORD)strlen(text), &wrote, nullptr);
    *err = ok ? 0 : GetLastError();
    CloseHandle(h);
    return ok != FALSE;
}

std::wstring Arg(int argc, wchar_t** argv, int& i) {
    if (i + 1 >= argc) {
        fprintf(stderr, "missing value for %ls\n", argv[i]);
        exit(2);
    }
    return argv[++i];
}


// Poll mode: read the four XInput users every few milliseconds, answer with rumble derived from the triggers
// (left motor = lt * 257, right motor = rt * 257 on user 0, and the same attempt on user 1), and rewrite the
// output file whenever what was seen changes. Runs until the wait file appears.
void PollXInput(const Args& a) {
    std::wstring dll = L"xinput" + a.xinput + L".dll";
    HMODULE m = LoadLibraryW(dll.c_str());
    if (!m) return;
    typedef DWORD(WINAPI * Fn2)(DWORD, void*);
    auto gs = (Fn2)GetProcAddress(m, "XInputGetState");
    auto ss = (Fn2)GetProcAddress(m, "XInputSetState");
    if (!gs || !ss) return;
    struct St {
        DWORD packet;
        WORD buttons;
        BYTE lt, rt;
        SHORT lx, ly, rx, ry;
    };
    std::string last;
    unsigned long long n = 0;
    DWORD start = GetTickCount();
    while (a.wait_file.empty() || !FileExists(a.wait_file)) {
        if (GetTickCount() - start > a.timeout_ms) break;
        std::string j = "{\"pads\":[";
        for (DWORD i = 0; i < 4; ++i) {
            St st = {};
            DWORD r = gs(i, &st);
            struct {
                WORD l, r;
            } vib = {(WORD)(st.lt * 257), (WORD)(st.rt * 257)};
            DWORD rs = ss(i, &vib);
            if (i) j += ",";
            j += "{\"index\":" + std::to_string(i) + ",\"get_state\":" + std::to_string(r) + ",\"packet\":" + std::to_string(st.packet) +
                 ",\"buttons\":" + std::to_string(st.buttons) + ",\"lt\":" + std::to_string(st.lt) + ",\"rt\":" + std::to_string(st.rt) +
                 ",\"lx\":" + std::to_string(st.lx) + ",\"ly\":" + std::to_string(st.ly) + ",\"rx\":" + std::to_string(st.rx) +
                 ",\"ry\":" + std::to_string(st.ry) + ",\"set_state\":" + std::to_string(rs) + "}";
        }
        j += "]";
        if (j != last) {
            ++n;
            std::string full = j + ",\"poll_version\":" + std::to_string(n) + ",\"pid\":" + std::to_string(GetCurrentProcessId()) + "}";
            std::wstring tmp = a.out + L".poll";
            FILE* fp = _wfopen(tmp.c_str(), L"wb");
            if (fp) {
                fwrite(full.data(), 1, full.size(), fp);
                fclose(fp);
                // the reader may have the file open for a moment; try again on the next round if so
                if (MoveFileExW(tmp.c_str(), a.out.c_str(), MOVEFILE_REPLACE_EXISTING)) last = j;
            }
        }
        Pump(4);
    }
}


// ---- keyboard / mouse mode

struct DiState {
    bool loaded = false;
    IDirectInput8W* di = nullptr;
    IDirectInputDevice8W* kbd = nullptr;
    IDirectInputDevice8W* mouse = nullptr;
    std::string kbd_result = "none", mouse_result = "none";
    bool kbd_a = false;       // DIK_A down in the last GetDeviceState
    bool kbd_a_ever = false;  // DIK_A seen down in any GetDeviceState
    long long mouse_dx = 0, mouse_dy = 0, mouse_dz = 0;  // summed GetDeviceState deltas
    int mouse_buttons = 0;    // bit i = button i seen down in any GetDeviceState
    std::vector<std::string> kbd_events;    // "ofs:data" from GetDeviceData
    std::vector<std::string> mouse_events;  // "ofs:data" from GetDeviceData
} g_di;

void SetupDirectInput() {
    HMODULE m = LoadLibraryW(L"dinput8.dll");
    if (!m) return;
    typedef HRESULT(WINAPI * CreateFn)(HINSTANCE, DWORD, REFIID, LPVOID*, LPUNKNOWN);
    auto create = (CreateFn)GetProcAddress(m, "DirectInput8Create");
    if (!create) return;
    if (FAILED(create(GetModuleHandleW(nullptr), DIRECTINPUT_VERSION, IID_IDirectInput8W, (void**)&g_di.di, nullptr))) return;
    g_di.loaded = true;
    if (SUCCEEDED(g_di.di->CreateDevice(GUID_SysKeyboard, &g_di.kbd, nullptr))) {
        g_di.kbd->SetDataFormat(&c_dfDIKeyboard);
        g_di.kbd->SetCooperativeLevel(g_wnd, DISCL_BACKGROUND | DISCL_NONEXCLUSIVE);
        DIPROPDWORD pd = {};
        pd.diph.dwSize = sizeof(pd);
        pd.diph.dwHeaderSize = sizeof(DIPROPHEADER);
        pd.dwData = 64;
        g_di.kbd->SetProperty(DIPROP_BUFFERSIZE, &pd.diph);
        HRESULT hr = g_di.kbd->Acquire();
        g_di.kbd_result = "acquire=" + std::to_string((long)hr);
    }
    if (SUCCEEDED(g_di.di->CreateDevice(GUID_SysMouse, &g_di.mouse, nullptr))) {
        g_di.mouse->SetDataFormat(&c_dfDIMouse2);
        g_di.mouse->SetCooperativeLevel(g_wnd, DISCL_BACKGROUND | DISCL_NONEXCLUSIVE);
        DIPROPDWORD pd = {};
        pd.diph.dwSize = sizeof(pd);
        pd.diph.dwHeaderSize = sizeof(DIPROPHEADER);
        pd.dwData = 64;
        g_di.mouse->SetProperty(DIPROP_BUFFERSIZE, &pd.diph);
        HRESULT hr = g_di.mouse->Acquire();
        g_di.mouse_result = "acquire=" + std::to_string((long)hr);
    }
}

void PollDirectInput() {
    if (g_di.kbd) {
        BYTE keys[256] = {};
        HRESULT hr = g_di.kbd->GetDeviceState(sizeof(keys), keys);
        if (SUCCEEDED(hr)) {
            bool a = (keys[DIK_A] & 0x80) != 0;
            if (a != g_di.kbd_a) g_dirty = true;
            g_di.kbd_a = a;
            if (a && !g_di.kbd_a_ever) {
                g_di.kbd_a_ever = true;
                g_dirty = true;
            }
        }
        DIDEVICEOBJECTDATA data[32];
        DWORD n = 32;
        if (SUCCEEDED(g_di.kbd->GetDeviceData(sizeof(DIDEVICEOBJECTDATA), data, &n, 0))) {
            for (DWORD i = 0; i < n; ++i) g_di.kbd_events.push_back(std::to_string(data[i].dwOfs) + ":" + std::to_string(data[i].dwData));
            if (n) g_dirty = true;
        }
    }
    if (g_di.mouse) {
        DIMOUSESTATE2 st = {};
        HRESULT hr = g_di.mouse->GetDeviceState(sizeof(st), &st);
        if (SUCCEEDED(hr)) {
            if (st.lX || st.lY || st.lZ) g_dirty = true;
            g_di.mouse_dx += st.lX;
            g_di.mouse_dy += st.lY;
            g_di.mouse_dz += st.lZ;
            for (int i = 0; i < 8; ++i)
                if (st.rgbButtons[i] & 0x80) {
                    if (!(g_di.mouse_buttons & (1 << i))) g_dirty = true;
                    g_di.mouse_buttons |= 1 << i;
                }
        }
        DIDEVICEOBJECTDATA data[32];
        DWORD n = 32;
        if (SUCCEEDED(g_di.mouse->GetDeviceData(sizeof(DIDEVICEOBJECTDATA), data, &n, 0))) {
            for (DWORD i = 0; i < n; ++i) g_di.mouse_events.push_back(std::to_string(data[i].dwOfs) + ":" + std::to_string((int)data[i].dwData));
            if (n) g_dirty = true;
        }
    }
}

std::vector<std::wstring> ReadLines(const std::wstring& path) {
    std::vector<std::wstring> lines;
    FILE* f = _wfopen(path.c_str(), L"rb");
    if (!f) return lines;
    std::string text;
    char buf[512];
    size_t n;
    while ((n = fread(buf, 1, sizeof(buf), f)) > 0) text.append(buf, n);
    fclose(f);
    std::wstring cur;
    for (char c : text) {
        if (c == '\n') {
            lines.push_back(cur);
            cur.clear();
        } else if (c != '\r') {
            cur += (wchar_t)c;
        }
    }
    return lines;  // a last line without newline is still being written
}

size_t g_cmds_done = 0;
std::string g_cmd_results;

// Calls that would touch the real cursor without the hook run only on request from the harness, which asks after it
// has checked that the hook is installed.
void RunCommands(const std::wstring& file) {
    if (file.empty()) return;
    std::vector<std::wstring> lines = ReadLines(file);
    while (g_cmds_done < lines.size()) {
        const std::wstring& line = lines[g_cmds_done++];
        wchar_t cmd[32] = {};
        int a = 0, b = 0, c = 0, d = 0;
        int n = swscanf(line.c_str(), L"%31ls %d %d %d %d", cmd, &a, &b, &c, &d);
        std::wstring name = cmd;
        std::string res;
        if (name == L"setpos" && n >= 3) {
            POINT p = {a, b};
            ClientToScreen(g_wnd, &p);
            res = std::string("setpos=") + (SetCursorPos(p.x, p.y) ? "1" : "0");
        } else if (name == L"clip" && n >= 5) {
            POINT lt = {a, b}, rb = {c, d};
            ClientToScreen(g_wnd, &lt);
            ClientToScreen(g_wnd, &rb);
            RECT r = {lt.x, lt.y, rb.x, rb.y};
            res = std::string("clip=") + (ClipCursor(&r) ? "1" : "0");
        } else if (name == L"unclip") {
            res = std::string("unclip=") + (ClipCursor(nullptr) ? "1" : "0");
        } else if (name == L"show" && n >= 2) {
            res = "show=" + std::to_string(ShowCursor(a ? TRUE : FALSE));
        } else if (name == L"setcursor0") {
            SetCursor(nullptr);
            res = "setcursor0";
        } else {
            res = "unknown";
        }
        g_cmd_results += (g_cmd_results.empty() ? "" : ",") + std::string("\"") + Esc(res) + "\"";
        g_dirty = true;
    }
}

BOOL CALLBACK FindOverlay(HWND h, LPARAM lp) {
    DWORD pid = 0;
    GetWindowThreadProcessId(h, &pid);
    if (pid != GetCurrentProcessId()) return TRUE;
    wchar_t cls[64] = {};
    GetClassNameW(h, cls, 64);
    if (wcscmp(cls, L"CouchlinkFakeCursor") != 0) return TRUE;
    *(HWND*)lp = h;
    return FALSE;
}

std::string StateJson() {
    std::string j = "\"state\":{";
    POINT cp = {};
    GetCursorPos(&cp);
    POINT client = cp;
    ScreenToClient(g_wnd, &client);
    BYTE keys[256] = {};
    GetKeyboardState(keys);
    RECT clip = {};
    GetClipCursor(&clip);
    CURSORINFO ci = {};
    ci.cbSize = sizeof(ci);
    GetCursorInfo(&ci);
    j += "\"key_state_a\":" + Num(GetKeyState('A')) + ",\"key_state_b\":" + Num(GetKeyState('B')) +
         ",\"async_a\":" + Num(GetAsyncKeyState('A')) + ",\"async_b\":" + Num(GetAsyncKeyState('B')) +
         ",\"keyboard_state_a\":" + Num(keys['A']) + ",\"keyboard_state_b\":" + Num(keys['B']) +
         ",\"shift\":" + Num(GetKeyState(VK_SHIFT)) + ",\"lshift\":" + Num(GetKeyState(VK_LSHIFT)) + ",\"control\":" + Num(GetKeyState(VK_CONTROL)) +
         ",\"capslock\":" + Num(GetKeyState(VK_CAPITAL)) +
         ",\"lbutton\":" + Num(GetKeyState(VK_LBUTTON)) + ",\"rbutton\":" + Num(GetKeyState(VK_RBUTTON)) +
         ",\"cursor_screen\":[" + Num(cp.x) + "," + Num(cp.y) + "],\"cursor_client\":[" + Num(client.x) + "," + Num(client.y) + "]" +
         ",\"clip\":[" + Num(clip.left) + "," + Num(clip.top) + "," + Num(clip.right) + "," + Num(clip.bottom) + "]" +
         ",\"cursor_info_showing\":" + B((ci.flags & CURSOR_SHOWING) != 0) + ",\"cursor_info_pos\":[" + Num(ci.ptScreenPos.x) + "," + Num(ci.ptScreenPos.y) + "]";
    RECT cr = {};
    GetClientRect(g_wnd, &cr);
    POINT origin = {0, 0};
    ClientToScreen(g_wnd, &origin);
    j += ",\"client_size\":[" + Num(cr.right) + "," + Num(cr.bottom) + "],\"client_origin\":[" + Num(origin.x) + "," + Num(origin.y) + "]";
    HWND overlay = nullptr;
    EnumWindows(FindOverlay, (LPARAM)&overlay);
    j += ",\"overlay_exists\":" + B(overlay != nullptr);
    if (overlay) {
        RECT r = {};
        GetWindowRect(overlay, &r);
        j += ",\"overlay_visible\":" + B(IsWindowVisible(overlay) != FALSE) + ",\"overlay_pos\":[" + Num(r.left) + "," + Num(r.top) + "]";
    }
    j += "}";
    return j;
}

std::string LogJson() {
    std::string j = "\"log\":[";
    for (size_t i = 0; i < g_log.size(); ++i) {
        if (i) j += ",";
        j += "\"" + Esc(g_log[i]) + "\"";
    }
    j += "]";
    return j;
}

std::string DiJson() {
    if (!g_di.loaded) return "\"dinput\":{\"loaded\":false}";
    auto list = [](const std::vector<std::string>& v) {
        std::string r = "[";
        for (size_t i = 0; i < v.size(); ++i) {
            if (i) r += ",";
            r += "\"" + v[i] + "\"";
        }
        return r + "]";
    };
    return "\"dinput\":{\"loaded\":true,\"kbd\":\"" + g_di.kbd_result + "\",\"mouse\":\"" + g_di.mouse_result + "\",\"kbd_a\":" + B(g_di.kbd_a) +
           ",\"kbd_a_ever\":" + B(g_di.kbd_a_ever) + ",\"mouse_dx\":" + Num(g_di.mouse_dx) + ",\"mouse_dy\":" + Num(g_di.mouse_dy) +
           ",\"mouse_dz\":" + Num(g_di.mouse_dz) + ",\"mouse_buttons\":" + Num(g_di.mouse_buttons) + ",\"kbd_events\":" + list(g_di.kbd_events) +
           ",\"mouse_events\":" + list(g_di.mouse_events) + "}";
}

// Keyboard / mouse mode: runs a normal message loop (with TranslateMessage), logs every keyboard and mouse message,
// samples the key and cursor functions each round and rewrites the output file whenever something changed.
void RunKbm(const Args& a) {
    g_log_input = true;
    if (a.rawinput) {
        RAWINPUTDEVICE rid[2] = {};
        rid[0].usUsagePage = 1;
        rid[0].usUsage = 2;
        rid[0].hwndTarget = g_wnd;
        rid[1].usUsagePage = 1;
        rid[1].usUsage = 6;
        rid[1].hwndTarget = g_wnd;
        BOOL ok = RegisterRawInputDevices(rid, 2, sizeof(RAWINPUTDEVICE));
        Log(std::string("RAWREG ") + (ok ? "1" : "0"));
    }
    if (a.dinput) SetupDirectInput();
    if (a.inject_real) {
        // What the real keyboard and mouse would put into the queue: no signature, so the hook must drop them.
        PostMessageW(g_wnd, WM_KEYDOWN, 'Z', 1 | (0x2C << 16));
        PostMessageW(g_wnd, WM_CHAR, 'z', 1 | (0x2C << 16));
        PostMessageW(g_wnd, WM_KEYUP, 'Z', 1 | (0x2C << 16) | (3u << 30));
        PostMessageW(g_wnd, WM_MOUSEMOVE, 0, MAKELPARAM(5, 6));
        PostMessageW(g_wnd, WM_LBUTTONDOWN, MK_LBUTTON, MAKELPARAM(7, 8));
        PostMessageW(g_wnd, WM_MOUSEWHEEL, MAKEWPARAM(0, 120), MAKELPARAM(9, 9));
        PostMessageW(g_wnd, WM_NCMOUSEMOVE, HTCAPTION, MAKELPARAM(1, 1));
        if (a.rawinput) PostMessageW(g_wnd, WM_INPUT, RIM_INPUT, (LPARAM)0x1234);  // a real WM_INPUT has a handle the system made
        Log("INJECTED");
    }
    std::string last;
    unsigned long long version = 0;
    DWORD start = GetTickCount();
    while (a.wait_file.empty() || !FileExists(a.wait_file)) {
        if (GetTickCount() - start > a.timeout_ms) break;
        Pump(2);
        RunCommands(a.cmd_file);
        if (a.dinput) PollDirectInput();
        std::string state = StateJson();
        if (!g_dirty && state == last) continue;
        g_dirty = false;
        last = state;
        ++version;
        std::string full = "{\"kbm\":true,\"pid\":" + std::to_string(GetCurrentProcessId()) + ",\"hwnd\":" + Num((long long)(UINT_PTR)g_wnd) +
                           ",\"version\":" + std::to_string(version) + "," + state + "," + LogJson() + "," + DiJson() + ",\"commands\":[" +
                           g_cmd_results + "]}";
        std::wstring tmp = a.out + L".kbm";
        FILE* fp = _wfopen(tmp.c_str(), L"wb");
        if (fp) {
            fwrite(full.data(), 1, full.size(), fp);
            fclose(fp);
            if (!MoveFileExW(tmp.c_str(), a.out.c_str(), MOVEFILE_REPLACE_EXISTING)) g_dirty = true;
        }
    }
}

// Starts this exe again (the child writes its own output file and stops with the same wait file).
bool SpawnChild(const Args& a) {
    wchar_t self[MAX_PATH * 2];
    GetModuleFileNameW(nullptr, self, ARRAYSIZE(self));
    if (!a.child_exe.empty()) wcsncpy_s(self, a.child_exe.c_str(), _TRUNCATE);
    std::wstring cmd = L"\"" + std::wstring(self) + L"\" --out \"" + a.spawn_child + L"\" --timeout-ms " + std::to_wstring(a.timeout_ms);
    if (!a.wait_file.empty()) cmd += L" --wait-file \"" + a.wait_file + L"\"";
    if (!a.child_args.empty()) cmd += L" " + a.child_args;
    STARTUPINFOW si = {};
    si.cb = sizeof(si);
    PROCESS_INFORMATION pi = {};
    if (!CreateProcessW(self, &cmd[0], nullptr, nullptr, FALSE, 0, nullptr, nullptr, &si, &pi)) return false;
    CloseHandle(pi.hThread);
    CloseHandle(pi.hProcess);
    return true;
}

}  // namespace

int wmain(int argc, wchar_t** argv) {
    Args a;
    for (int i = 1; i < argc; ++i) {
        std::wstring k = argv[i];
        if (k == L"--out") a.out = Arg(argc, argv, i);
        else if (k == L"--wait-file") a.wait_file = Arg(argc, argv, i);
        else if (k == L"--xinput") a.xinput = Arg(argc, argv, i);
        else if (k == L"--link-file") a.link_file = Arg(argc, argv, i);
        else if (k == L"--link-file-a") a.link_file_a = Arg(argc, argv, i);
        else if (k == L"--link-trunc") a.link_trunc = Arg(argc, argv, i);
        else if (k == L"--save-dir") a.save_dir = Arg(argc, argv, i);
        else if (k == L"--known-docs") a.known_docs = true;
        else if (k == L"--no-window") a.no_window = true;
        else if (k == L"--poll-xinput") a.poll_xinput = true;
        else if (k == L"--use-version") a.use_version = true;
        else if (k == L"--pads") a.pads = true;
        else if (k == L"--kbm") a.kbm = true;
        else if (k == L"--inject-real") a.inject_real = true;
        else if (k == L"--rawinput") a.rawinput = true;
        else if (k == L"--dinput") a.dinput = true;
        else if (k == L"--cmd-file") a.cmd_file = Arg(argc, argv, i);
        else if (k == L"--spawn-child") a.spawn_child = Arg(argc, argv, i);
        else if (k == L"--child-exe") a.child_exe = Arg(argc, argv, i);
        else if (k == L"--child-args") a.child_args = Arg(argc, argv, i);
        else if (k == L"--launcher") a.launcher = true;
        else if (k == L"--timeout-ms") a.timeout_ms = (DWORD)_wtoi(Arg(argc, argv, i).c_str());
    }
    if (a.out.empty()) {
        fprintf(stderr, "usage: test_target --out file.json [options]\n");
        return 2;
    }

    std::string j = "{";
#ifdef _WIN64
    j += "\"arch\":\"x64\"";
#else
    j += "\"arch\":\"x86\"";
#endif
    j += ",\"pid\":" + std::to_string(GetCurrentProcessId());

    // ---- window and focus
    HWND wnd = nullptr;
    if (!a.no_window) {
        WNDCLASSW wc = {};
        wc.lpfnWndProc = TestProc;
        wc.hInstance = GetModuleHandleW(nullptr);
        wc.lpszClassName = L"CouchlinkTestTarget";
        RegisterClassW(&wc);
        // Visible but off screen and never activated, so it cannot take focus from whatever the owner is doing.
        wnd = CreateWindowExW(WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW, wc.lpszClassName, L"couchlink test target", WS_POPUP, -3000,
                              -3000, 64, 64, nullptr, nullptr, wc.hInstance, nullptr);
        ShowWindow(wnd, SW_SHOWNOACTIVATE);
        g_wnd = wnd;
        Pump(300);
        j += ",\"focus\":{";
        j += "\"foreground_is_self\":" + B(GetForegroundWindow() == wnd);
        j += ",\"active_is_self\":" + B(GetActiveWindow() == wnd);
        j += ",\"focus_is_self\":" + B(GetFocus() == wnd);
        g_seen = Seen();
        SendMessageW(wnd, WM_ACTIVATE, WA_INACTIVE, 0);
        SendMessageW(wnd, WM_ACTIVATEAPP, FALSE, 0);
        SendMessageW(wnd, WM_NCACTIVATE, FALSE, 0);
        SendMessageW(wnd, WM_KILLFOCUS, 0, 0);
        // Posted deactivate message (goes through the message loop filter).
        PostMessageW(wnd, WM_ACTIVATE, WA_INACTIVE, 0);
        Pump(100);
        j += ",\"saw_activate_inactive\":" + B(g_seen.activate_inactive);
        j += ",\"saw_activateapp_false\":" + B(g_seen.activateapp_false);
        j += ",\"saw_ncactivate_false\":" + B(g_seen.ncactivate_false);
        j += ",\"saw_killfocus\":" + B(g_seen.killfocus);
        j += "}";
    }

    // ---- version.dll (a static import): proves a stand-in forwards every call to the real DLL
    if (a.use_version) {
        DWORD handle = 0;
        DWORD size = GetFileVersionInfoSizeW(L"C:\\Windows\\System32\\kernel32.dll", &handle);
        j += ",\"version_dll\":{\"size_nonzero\":" + B(size > 0) + "}";
    }

    // ---- game controllers through the other APIs (own_pads hides other players' ones)
    if (a.pads) {
        auto isPad = [](USHORT page, USHORT usage) { return page == 1 && (usage == 4 || usage == 5 || usage == 8); };
        auto list = [](const std::vector<std::wstring>& v) {
            std::string r = "[";
            for (size_t i = 0; i < v.size(); ++i) r += (i ? "," : "") + Q(v[i]);
            return r + "]";
        };
        // Raw Input
        std::vector<std::wstring> raw;
        UINT n = 0;
        GetRawInputDeviceList(nullptr, &n, sizeof(RAWINPUTDEVICELIST));
        std::vector<RAWINPUTDEVICELIST> devs(n + 4);
        UINT cap = (UINT)devs.size();
        UINT got = GetRawInputDeviceList(devs.data(), &cap, sizeof(RAWINPUTDEVICELIST));
        for (UINT i = 0; got != (UINT)-1 && i < got; ++i) {
            if (devs[i].dwType != RIM_TYPEHID) continue;
            RID_DEVICE_INFO info = {};
            info.cbSize = sizeof(info);
            UINT sz = sizeof(info);
            if (GetRawInputDeviceInfoW(devs[i].hDevice, RIDI_DEVICEINFO, &info, &sz) == (UINT)-1) continue;
            if (!isPad(info.hid.usUsagePage, info.hid.usUsage)) continue;
            wchar_t name[512] = {};
            UINT chars = 512;
            GetRawInputDeviceInfoW(devs[i].hDevice, RIDI_DEVICENAME, name, &chars);
            raw.push_back(name);
        }
        // HID through SetupAPI and CreateFile
        std::vector<std::wstring> hid;
        int refused = 0;
        GUID hg;
        HidD_GetHidGuid(&hg);
        HDEVINFO set = SetupDiGetClassDevsW(&hg, nullptr, nullptr, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE);
        if (set != INVALID_HANDLE_VALUE) {
            SP_DEVICE_INTERFACE_DATA itf = {};
            itf.cbSize = sizeof(itf);
            for (DWORD i = 0; SetupDiEnumDeviceInterfaces(set, nullptr, &hg, i, &itf); ++i) {
                DWORD need = 0;
                SetupDiGetDeviceInterfaceDetailW(set, &itf, nullptr, 0, &need, nullptr);
                std::vector<BYTE> buf(need + 8);
                auto* det = reinterpret_cast<SP_DEVICE_INTERFACE_DETAIL_DATA_W*>(buf.data());
                det->cbSize = sizeof(SP_DEVICE_INTERFACE_DETAIL_DATA_W);
                if (!SetupDiGetDeviceInterfaceDetailW(set, &itf, det, need, nullptr, nullptr)) continue;
                HANDLE f = CreateFileW(det->DevicePath, 0, FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, OPEN_EXISTING, 0, nullptr);
                if (f == INVALID_HANDLE_VALUE) {
                    if (GetLastError() == ERROR_FILE_NOT_FOUND) ++refused;
                    continue;
                }
                PHIDP_PREPARSED_DATA pp = nullptr;
                if (HidD_GetPreparsedData(f, &pp)) {
                    HIDP_CAPS caps = {};
                    if (HidP_GetCaps(pp, &caps) == HIDP_STATUS_SUCCESS && isPad(caps.UsagePage, caps.Usage)) hid.push_back(det->DevicePath);
                    HidD_FreePreparsedData(pp);
                }
                CloseHandle(f);
            }
            SetupDiDestroyDeviceInfoList(set);
        }
        // DirectInput 8
        std::vector<std::wstring> di_paths;
        int di_enum = 0, di_refused = 0;
        IDirectInput8W* di = nullptr;
        if (SUCCEEDED(DirectInput8Create(GetModuleHandleW(nullptr), DIRECTINPUT_VERSION, IID_IDirectInput8W, (void**)&di, nullptr)) && di) {
            std::vector<GUID> found;
            di->EnumDevices(
                DI8DEVCLASS_GAMECTRL,
                [](LPCDIDEVICEINSTANCEW inst, LPVOID ctx) -> BOOL {
                    static_cast<std::vector<GUID>*>(ctx)->push_back(inst->guidInstance);
                    return DIENUM_CONTINUE;
                },
                &found, DIEDFL_ATTACHEDONLY);
            di_enum = (int)found.size();
            for (const GUID& g : found) {
                IDirectInputDevice8W* dev = nullptr;
                if (FAILED(di->CreateDevice(g, &dev, nullptr)) || !dev) {
                    ++di_refused;
                    continue;
                }
                DIPROPGUIDANDPATH gp = {};
                gp.diph.dwSize = sizeof(gp);
                gp.diph.dwHeaderSize = sizeof(DIPROPHEADER);
                gp.diph.dwHow = DIPH_DEVICE;
                if (SUCCEEDED(dev->GetProperty(DIPROP_GUIDANDPATH, &gp.diph))) di_paths.push_back(gp.wszPath);
                dev->Release();
            }
            di->Release();
        }
        // Windows.Gaming.Input: its lists fill in a moment after the first look, so look for up to 1.5 s.
        int wgi_pads = -1, wgi_raw = -1;
        {
            namespace wgi = ABI::Windows::Gaming::Input;
            using Microsoft::WRL::ComPtr;
            using Microsoft::WRL::Wrappers::HStringReference;
            RoInitialize(RO_INIT_MULTITHREADED);
            ComPtr<wgi::IGamepadStatics> gs;
            ComPtr<wgi::IRawGameControllerStatics> rs;
            RoGetActivationFactory(HStringReference(RuntimeClass_Windows_Gaming_Input_Gamepad).Get(), IID_PPV_ARGS(&gs));
            RoGetActivationFactory(HStringReference(RuntimeClass_Windows_Gaming_Input_RawGameController).Get(), IID_PPV_ARGS(&rs));
            for (int i = 0; i < 15 && (gs || rs); ++i) {
                unsigned n = 0;
                ComPtr<ABI::Windows::Foundation::Collections::IVectorView<wgi::Gamepad*>> pv;
                if (gs && SUCCEEDED(gs->get_Gamepads(&pv)) && SUCCEEDED(pv->get_Size(&n))) wgi_pads = (int)n;
                ComPtr<ABI::Windows::Foundation::Collections::IVectorView<wgi::RawGameController*>> rv;
                if (rs && SUCCEEDED(rs->get_RawGameControllers(&rv)) && SUCCEEDED(rv->get_Size(&n))) wgi_raw = (int)n;
                if (wgi_pads > 0 && wgi_raw > 0) break;
                Sleep(100);
            }
        }
        j += ",\"pads\":{\"wgi_gamepads\":" + std::to_string(wgi_pads) + ",\"wgi_raw\":" + std::to_string(wgi_raw) + ",\"raw\":" + list(raw) + ",\"hid\":" + list(hid) + ",\"hid_refused\":" + std::to_string(refused) +
             ",\"dinput_enumerated\":" + std::to_string(di_enum) + ",\"dinput\":" + list(di_paths) +
             ",\"dinput_refused\":" + std::to_string(di_refused) + "}";
    }

    // ---- xinput
    if (a.xinput != L"none") {
        std::wstring dll = L"xinput" + a.xinput + L".dll";
        HMODULE m = LoadLibraryW(dll.c_str());
        j += ",\"xinput\":{\"dll\":" + Q(dll) + ",\"loaded\":" + B(m != nullptr);
        if (m) {
            typedef DWORD(WINAPI * GetStateFn)(DWORD, void*);
            typedef DWORD(WINAPI * SetStateFn)(DWORD, void*);
            typedef DWORD(WINAPI * GetCapsFn)(DWORD, DWORD, void*);
            auto gs = (GetStateFn)GetProcAddress(m, "XInputGetState");
            auto ss = (SetStateFn)GetProcAddress(m, "XInputSetState");
            auto gc = (GetCapsFn)GetProcAddress(m, "XInputGetCapabilities");
            auto gsx = (GetStateFn)GetProcAddress(m, (LPCSTR)100);
            j += ",\"pads\":[";
            for (DWORD i = 0; i < 4; ++i) {
                struct {
                    DWORD packet;
                    WORD buttons;
                    BYTE lt, rt;
                    SHORT lx, ly, rx, ry;
                } st = {};
                DWORD r = gs ? gs(i, &st) : 0xFFFF;
                struct {
                    BYTE type, subtype;
                    WORD flags;
                    WORD buttons;
                    BYTE lt, rt;
                    SHORT lx, ly, rx, ry;
                    WORD vl, vr;
                } caps = {};
                DWORD rc = gc ? gc(i, 0, &caps) : 0xFFFF;
                struct {
                    WORD l, r;
                } vib = {(WORD)(1000 * (i + 1)), (WORD)(2000 * (i + 1))};
                DWORD rs = ss ? ss(i, &vib) : 0xFFFF;
                DWORD rx = 0xFFFF;
                if (gsx) {
                    decltype(st) st2 = {};
                    rx = gsx(i, &st2);
                }
                if (i) j += ",";
                j += "{\"index\":" + std::to_string(i) + ",\"get_state\":" + std::to_string(r) + ",\"packet\":" + std::to_string(st.packet) +
                     ",\"buttons\":" + std::to_string(st.buttons) + ",\"lt\":" + std::to_string(st.lt) + ",\"rt\":" + std::to_string(st.rt) +
                     ",\"lx\":" + std::to_string(st.lx) + ",\"ly\":" + std::to_string(st.ly) + ",\"rx\":" + std::to_string(st.rx) +
                     ",\"ry\":" + std::to_string(st.ry) + ",\"caps\":" + std::to_string(rc) + ",\"caps_type\":" + std::to_string(caps.type) +
                     ",\"set_state\":" + std::to_string(rs) + ",\"get_state_ex\":" + (gsx ? std::to_string(rx) : std::string("null")) + "}";
            }
            j += "]";
        }
        j += "}";
    }

    // ---- single-instance objects
    {
        HANDLE mu = CreateMutexW(nullptr, FALSE, L"TestGameMutex");
        DWORD e1 = GetLastError();
        HANDLE open = OpenMutexW(SYNCHRONIZE, FALSE, L"TestGameMutex");
        HANDLE mua = CreateMutexA(nullptr, FALSE, "TestGameMutexA");
        DWORD e2 = GetLastError();
        HANDLE ev = CreateEventW(nullptr, TRUE, FALSE, L"TestGameEvent");
        DWORD e3 = GetLastError();
        HANDLE sem = CreateSemaphoreW(nullptr, 1, 1, L"TestGameSemaphore");
        DWORD e4 = GetLastError();
        j += ",\"objects\":{\"mutex_created\":" + B(mu != nullptr) + ",\"mutex_already_exists\":" + B(e1 == ERROR_ALREADY_EXISTS) +
             ",\"mutex_reopened\":" + B(open != nullptr) + ",\"mutex_a_created\":" + B(mua != nullptr) +
             ",\"mutex_a_already_exists\":" + B(e2 == ERROR_ALREADY_EXISTS) + ",\"event_created\":" + B(ev != nullptr) +
             ",\"event_already_exists\":" + B(e3 == ERROR_ALREADY_EXISTS) + ",\"semaphore_created\":" + B(sem != nullptr) +
             ",\"semaphore_already_exists\":" + B(e4 == ERROR_ALREADY_EXISTS) + "}";
    }

    // ---- linked files
    {
        j += ",\"files\":{";
        bool first = true;
        auto add = [&](const char* key, bool ok, DWORD err) {
            if (!first) j += ",";
            first = false;
            j += std::string("\"") + key + "\":{\"ok\":" + B(ok) + ",\"error\":" + std::to_string(err) + "}";
        };
        DWORD err = 0;
        if (!a.link_file.empty()) add("append_w", AppendTo(a.link_file, "written-by-w\n", false, OPEN_EXISTING, &err), err);
        if (!a.link_file_a.empty()) add("append_a", AppendTo(a.link_file_a, "written-by-a\n", true, OPEN_EXISTING, &err), err);
        if (!a.link_trunc.empty()) add("truncate_w", AppendTo(a.link_trunc, "truncated\n", false, CREATE_ALWAYS, &err), err);
        j += "}";
    }

    // ---- saves
    if (!a.save_dir.empty()) {
        std::wstring dir = a.save_dir;
        BOOL mk = CreateDirectoryW((dir + L"\\sub").c_str(), nullptr);
        DWORD mkerr = GetLastError();
        DWORD err = 0;
        bool wrote = AppendTo(dir + L"\\slot1.sav", "save-data\n", false, CREATE_ALWAYS, &err);
        DWORD attrs = GetFileAttributesW((dir + L"\\slot1.sav").c_str());
        std::string names;
        WIN32_FIND_DATAW fd;
        HANDLE f = FindFirstFileW((dir + L"\\*").c_str(), &fd);
        if (f != INVALID_HANDLE_VALUE) {
            bool firstn = true;
            do {
                if (!wcscmp(fd.cFileName, L".") || !wcscmp(fd.cFileName, L"..")) continue;
                if (!firstn) names += ",";
                firstn = false;
                names += Q(fd.cFileName);
            } while (FindNextFileW(f, &fd));
            FindClose(f);
        }
        j += ",\"saves\":{\"mkdir_ok\":" + B(mk != FALSE) + ",\"mkdir_error\":" + std::to_string(mkerr) +
             ",\"write_ok\":" + B(wrote) + ",\"write_error\":" + std::to_string(err) + ",\"attrs_ok\":" + B(attrs != INVALID_FILE_ATTRIBUTES) +
             ",\"listing\":[" + names + "]}";
    }
    if (a.known_docs) {
        PWSTR p = nullptr;
        std::wstring docs;
        if (SUCCEEDED(SHGetKnownFolderPath(FOLDERID_Documents, 0, nullptr, &p)) && p) {
            docs = p;
            CoTaskMemFree(p);
        }
        j += ",\"known_documents\":" + Q(docs);
    }
    j += "}";

    // write atomically so the harness never reads half a file
    std::wstring tmp = a.out + L".tmp";
    FILE* fp = _wfopen(tmp.c_str(), L"wb");
    if (!fp) return 3;
    fwrite(j.data(), 1, j.size(), fp);
    fclose(fp);
    MoveFileExW(tmp.c_str(), a.out.c_str(), MOVEFILE_REPLACE_EXISTING);

    if (!a.spawn_child.empty()) {
        if (!SpawnChild(a)) return 4;
        if (a.launcher) return 0;  // a launcher exits as soon as the real program runs
    }
    if (a.kbm && g_wnd) RunKbm(a);
    if (a.poll_xinput && a.xinput != L"none") PollXInput(a);
    if (!a.wait_file.empty()) {
        DWORD start = GetTickCount();
        while (!FileExists(a.wait_file) && GetTickCount() - start < a.timeout_ms) Pump(20);
    }
    return 0;
}
