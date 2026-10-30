/* winprobe: a tiny Windows console program for the Linux Proton check (tests/integration/session_linux.rs).
 * Built with mingw in CI. Every half second it writes status.json in its working folder: that it runs, the Wine
 * version it runs under (from ntdll's wine_get_version), and its seat (COUCHLINK_SEAT). It runs until it is killed. */
#include <stdio.h>
#include <stdlib.h>
#include <windows.h>

int main(void) {
    typedef const char *(*wine_version_fn)(void);
    wine_version_fn wine_version = (wine_version_fn)(void *)GetProcAddress(GetModuleHandleA("ntdll.dll"), "wine_get_version");
    const char *wine = wine_version ? wine_version() : "";
    const char *seat = getenv("COUCHLINK_SEAT");
    for (;;) {
        FILE *f = fopen("status.json.tmp", "w");
        if (f) {
            fprintf(f, "{\"alive\":true,\"wine\":\"%s\",\"seat\":\"%s\",\"pid\":%lu}", wine, seat ? seat : "", GetCurrentProcessId());
            fclose(f);
            MoveFileExA("status.json.tmp", "status.json", MOVEFILE_REPLACE_EXISTING);
        }
        Sleep(500);
    }
    return 0;
}
