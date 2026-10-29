// Included by each generated proxy/<name>-<arch>.cpp (after src/proxy_forward.h, CL_PROXY_NAME and kExports).
//
// The stand-in exports one jump stub per export of the real Windows DLL (proxy/<name>-<arch>.asm). Each stub jumps
// through cl_fwd[i]; this fills the table with the real DLL's functions, loaded from the Windows folder by full
// path, so the game gets exactly what it would have gotten without us.
extern "C" void* cl_fwd[];

namespace {
// Stand-in for an export the installed Windows does not have: returns 0 instead of jumping to nothing.
int ClMissing() { return 0; }
}  // namespace

extern "C" void ClProxyForwardInit() {
    static bool done = false;
    if (done) return;
    done = true;
    wchar_t path[MAX_PATH];
    // For a 32-bit process on 64-bit Windows this is the 32-bit system folder, which is the right one.
    UINT n = GetSystemDirectoryW(path, MAX_PATH);
    if (n == 0 || n > MAX_PATH - 40) return;
    lstrcatW(path, L"\\" CL_PROXY_NAME L".dll");
    HMODULE real = LoadLibraryW(path);
    for (size_t i = 0; i < sizeof(kExports) / sizeof(kExports[0]); ++i) {
        FARPROC p = nullptr;
        if (real) p = kExports[i].name ? GetProcAddress(real, kExports[i].name) : GetProcAddress(real, MAKEINTRESOURCEA(kExports[i].ordinal));
        cl_fwd[i] = p ? reinterpret_cast<void*>(p) : reinterpret_cast<void*>(&ClMissing);
    }
}
