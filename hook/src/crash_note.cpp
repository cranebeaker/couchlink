// Says where a copy of a game crashes: the first access violations (and the other crashing exceptions) are written
// to the copy's own output (the log a player sends) and noted in the hook status, with the program or library and the
// offset in it. A game that closes by itself a moment after it starts then leaves a trace to work from.
#include <atomic>
#include <string>

#include "common.h"

namespace cl {
namespace {

std::atomic<int> g_noted{0};

LONG CALLBACK CrashNote(PEXCEPTION_POINTERS p) {
    if (!p || !p->ExceptionRecord) return EXCEPTION_CONTINUE_SEARCH;
    DWORD code = p->ExceptionRecord->ExceptionCode;
    if (code != EXCEPTION_ACCESS_VIOLATION && code != EXCEPTION_STACK_OVERFLOW && code != EXCEPTION_ILLEGAL_INSTRUCTION &&
        code != EXCEPTION_INT_DIVIDE_BY_ZERO && code != EXCEPTION_PRIV_INSTRUCTION)
        return EXCEPTION_CONTINUE_SEARCH;
    if (g_noted.fetch_add(1) >= 8) return EXCEPTION_CONTINUE_SEARCH;
    void* at = p->ExceptionRecord->ExceptionAddress;
    wchar_t path[MAX_PATH] = {};
    HMODULE mod = nullptr;
    std::string where = "unknown module";
    if (GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, static_cast<LPCWSTR>(at), &mod) &&
        GetModuleFileNameW(mod, path, MAX_PATH)) {
        std::wstring w = path;
        size_t slash = w.find_last_of(L"\/");
        if (slash != std::wstring::npos) w = w.substr(slash + 1);
        char name[MAX_PATH] = {};
        WideCharToMultiByte(CP_UTF8, 0, w.c_str(), -1, name, MAX_PATH, nullptr, nullptr);
        char off[40];
        wsprintfA(off, "+0x%llx", static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(at) - reinterpret_cast<uintptr_t>(mod)));
        where = std::string(name) + off;
    }
    std::string what;
    if (code == EXCEPTION_ACCESS_VIOLATION && p->ExceptionRecord->NumberParameters >= 2) {
        ULONG_PTR kind = p->ExceptionRecord->ExceptionInformation[0];
        char addr[40];
        wsprintfA(addr, "0x%llx", static_cast<unsigned long long>(p->ExceptionRecord->ExceptionInformation[1]));
        what = std::string("access violation ") + (kind == 0 ? "reading " : kind == 1 ? "writing " : "executing ") + addr;
    } else {
        char c[24];
        wsprintfA(c, "exception 0x%08lx", code);
        what = c;
    }
    std::string line = "[couchlink] " + what + " in " + where;
    Note(line);
    HANDLE err = GetStdHandle(STD_ERROR_HANDLE);
    if (err && err != INVALID_HANDLE_VALUE) {
        line += "\r\n";
        DWORD wrote = 0;
        WriteFile(err, line.data(), static_cast<DWORD>(line.size()), &wrote, nullptr);
    }
    return EXCEPTION_CONTINUE_SEARCH;
}

}  // namespace

void InstallCrashNotes() { AddVectoredExceptionHandler(0, CrashNote); }

}  // namespace cl
