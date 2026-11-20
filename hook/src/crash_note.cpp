// Says where a copy of a game crashes: the first access violations (and the other crashing exceptions) are written
// to the copy's own output (the log a player sends) and noted in the hook status, with the program or library and the
// offset in it, and the places on the stack that look like callers. A game that closes by itself a moment after it
// starts then leaves a trace to work from.
#include <atomic>
#include <cstdio>
#include <string>

#include "common.h"

namespace cl {
namespace {

std::atomic<int> g_noted{0};

// "name.dll+0x1234" for an address inside a loaded module.
std::string Where(void* at) {
    wchar_t path[MAX_PATH] = {};
    HMODULE mod = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, static_cast<LPCWSTR>(at), &mod) ||
        !GetModuleFileNameW(mod, path, MAX_PATH))
        return "";
    std::wstring w = path;
    size_t slash = w.find_last_of(L"\\/");
    if (slash != std::wstring::npos) w = w.substr(slash + 1);
    char name[MAX_PATH] = {};
    WideCharToMultiByte(CP_UTF8, 0, w.c_str(), -1, name, MAX_PATH, nullptr, nullptr);
    char off[40];
    snprintf(off, sizeof(off), "+0x%llx", static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(at) - reinterpret_cast<uintptr_t>(mod)));
    return std::string(name) + off;
}

bool IsCode(void* at) {
    MEMORY_BASIC_INFORMATION mbi;
    if (!VirtualQuery(at, &mbi, sizeof(mbi))) return false;
    return mbi.State == MEM_COMMIT && (mbi.Protect & (PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY));
}

// Words on the stack that point into code: a rough list of who called whom.
std::string Callers(const CONTEXT* c) {
#ifdef _WIN64
    uintptr_t* sp = reinterpret_cast<uintptr_t*>(c->Rsp);
#else
    uintptr_t* sp = reinterpret_cast<uintptr_t*>(c->Esp);
#endif
    std::string out;
    int found = 0;
    for (int i = 0; i < 512 && found < 10; ++i) {
        uintptr_t v;
        if (IsBadReadPtr(sp + i, sizeof(v))) break;
        v = sp[i];
        if (!IsCode(reinterpret_cast<void*>(v))) continue;
        std::string w = Where(reinterpret_cast<void*>(v));
        if (w.empty()) continue;
        out += (found ? ", " : "") + w;
        ++found;
    }
    return out;
}

LONG CALLBACK CrashNote(PEXCEPTION_POINTERS p) {
    if (!p || !p->ExceptionRecord) return EXCEPTION_CONTINUE_SEARCH;
    DWORD code = p->ExceptionRecord->ExceptionCode;
    if (code != EXCEPTION_ACCESS_VIOLATION && code != EXCEPTION_STACK_OVERFLOW && code != EXCEPTION_ILLEGAL_INSTRUCTION &&
        code != EXCEPTION_INT_DIVIDE_BY_ZERO && code != EXCEPTION_PRIV_INSTRUCTION)
        return EXCEPTION_CONTINUE_SEARCH;
    if (g_noted.fetch_add(1) >= 8) return EXCEPTION_CONTINUE_SEARCH;
    void* at = p->ExceptionRecord->ExceptionAddress;
    std::string where = Where(at);
    if (where.empty()) where = "an address outside every module";
    std::string what;
    char buf[64];
    if (code == EXCEPTION_ACCESS_VIOLATION && p->ExceptionRecord->NumberParameters >= 2) {
        ULONG_PTR kind = p->ExceptionRecord->ExceptionInformation[0];
        snprintf(buf, sizeof(buf), "0x%llx", static_cast<unsigned long long>(p->ExceptionRecord->ExceptionInformation[1]));
        what = std::string("access violation ") + (kind == 0 ? "reading " : kind == 1 ? "writing " : "executing ") + buf;
    } else {
        snprintf(buf, sizeof(buf), "exception 0x%08lx", static_cast<unsigned long>(code));
        what = buf;
    }
    std::string line = "[couchlink] " + what + " in " + where;
    if (p->ContextRecord) {
        std::string callers = Callers(p->ContextRecord);
        if (!callers.empty()) line += "; stack: " + callers;
    }
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
