// Shared by the generated proxy/<name>-<arch>.cpp files.
#pragma once
#define WIN32_LEAN_AND_MEAN
#include <windows.h>

struct ClFwdExport {
    const char* name;  // null for an export that has only an ordinal
    unsigned short ordinal;
};
