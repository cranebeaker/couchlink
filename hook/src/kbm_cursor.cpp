// Fake cursor overlay: a small click-through window that shows where this copy's own mouse is.
//
// The desktop has one real cursor, which belongs to whoever sits at the keyboard and mouse of the PC. A copy with its
// own mouse gets a fake cursor instead (kbm_hook.cpp keeps the position). While the game wants a visible cursor
// (menus), this overlay draws an arrow at that position; while the game hides the cursor (mouse look) the overlay is
// hidden. The idea is ProtoInput's FakeCursor (MIT, see THIRD_PARTY.md); the window is a layered, topmost,
// transparent-to-input popup owned by its own thread.
#include <algorithm>
#include <cmath>
#include <vector>

#include "kbm_internal.h"

namespace cl {
namespace {

constexpr int kSize = 32;
const wchar_t kClass[] = L"CouchlinkFakeCursor";
std::atomic<bool> g_started{false};

// Arrow outline, hotspot at the first point.
const POINT kArrow[] = {{0, 0}, {0, 20}, {5, 15}, {9, 23}, {13, 21}, {9, 14}, {16, 14}};
constexpr int kArrowN = sizeof(kArrow) / sizeof(kArrow[0]);

bool Inside(double x, double y) {
    bool in = false;
    for (int i = 0, j = kArrowN - 1; i < kArrowN; j = i++) {
        double xi = kArrow[i].x, yi = kArrow[i].y, xj = kArrow[j].x, yj = kArrow[j].y;
        if (((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi)) in = !in;
    }
    return in;
}

double EdgeDistance(double x, double y) {
    double best = 1e9;
    for (int i = 0, j = kArrowN - 1; i < kArrowN; j = i++) {
        double ax = kArrow[j].x, ay = kArrow[j].y, bx = kArrow[i].x, by = kArrow[i].y;
        double dx = bx - ax, dy = by - ay;
        double t = ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy);
        t = t < 0 ? 0 : (t > 1 ? 1 : t);
        double px = ax + t * dx - x, py = ay + t * dy - y;
        best = std::min(best, std::sqrt(px * px + py * py));
    }
    return best;
}

// White arrow with a black outline, premultiplied alpha, top-down BGRA.
void RenderArrow(uint32_t* px) {
    for (int y = 0; y < kSize; ++y) {
        for (int x = 0; x < kSize; ++x) {
            int in = 0;
            for (int sy = 0; sy < 3; ++sy)
                for (int sx = 0; sx < 3; ++sx)
                    if (Inside(x + (sx + 0.5) / 3.0, y + (sy + 0.5) / 3.0)) ++in;
            double cx = x + 0.5, cy = y + 0.5;
            double d = EdgeDistance(cx, cy);
            uint32_t argb = 0;
            if (in > 0 && d > 0.9) {
                argb = 0xFFFFFFFFu;  // fill
            } else if (d <= 1.4 && (in > 0 || d <= 1.2)) {
                argb = 0xFF000000u;  // outline
            }
            px[y * kSize + x] = argb;
        }
    }
}

struct Overlay {
    HWND wnd = nullptr;
    HDC mem = nullptr;
    HBITMAP bmp = nullptr;
};

LRESULT CALLBACK OverlayProc(HWND h, UINT m, WPARAM w, LPARAM l) {
    if (m == WM_NCHITTEST) return HTTRANSPARENT;
    if (m == WM_MOUSEACTIVATE) return MA_NOACTIVATE;
    return DefWindowProcW(h, m, w, l);
}

bool CreateOverlay(Overlay& o) {
    WNDCLASSW wc = {};
    wc.lpfnWndProc = OverlayProc;
    wc.hInstance = GetModuleHandleW(nullptr);
    wc.lpszClassName = kClass;
    RegisterClassW(&wc);
    o.wnd = CreateWindowExW(WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW, kClass,
                            L"couchlink fake cursor", WS_POPUP, 0, 0, kSize, kSize, nullptr, nullptr, wc.hInstance, nullptr);
    if (!o.wnd) return false;
    BITMAPINFO bi = {};
    bi.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    bi.bmiHeader.biWidth = kSize;
    bi.bmiHeader.biHeight = -kSize;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;
    void* bits = nullptr;
    HDC screen = GetDC(nullptr);
    o.mem = CreateCompatibleDC(screen);
    o.bmp = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, &bits, nullptr, 0);
    ReleaseDC(nullptr, screen);
    if (!o.bmp || !bits) return false;
    SelectObject(o.mem, o.bmp);
    RenderArrow(static_cast<uint32_t*>(bits));
    return true;
}

DWORD WINAPI OverlayThread(LPVOID) {
    Overlay o;
    if (!CreateOverlay(o)) {
        Note("fake cursor: cannot create the overlay window");
        return 1;
    }
    bool shown = false;
    POINT last = {-32768, -32768};
    for (;;) {
        MSG msg;
        while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) DispatchMessageW(&msg);
        KbmCursorView v = KbmCursor();
        bool want = v.visible && v.window && IsWindowVisible(v.window);
        if (want) {
            if (!shown || v.screen.x != last.x || v.screen.y != last.y) {
                POINT pos = v.screen;
                POINT src = {0, 0};
                SIZE size = {kSize, kSize};
                BLENDFUNCTION bf = {AC_SRC_OVER, 0, 255, AC_SRC_ALPHA};
                UpdateLayeredWindow(o.wnd, nullptr, &pos, &size, o.mem, &src, 0, &bf, ULW_ALPHA);
                last = v.screen;
            }
            if (!shown) {
                ShowWindow(o.wnd, SW_SHOWNOACTIVATE);
                shown = true;
            }
        } else if (shown) {
            ShowWindow(o.wnd, SW_HIDE);
            shown = false;
        }
        Sleep(8);
    }
}

}  // namespace

void StartFakeCursor() {
    if (g_started.exchange(true)) return;
    HANDLE t = CreateThread(nullptr, 0, OverlayThread, nullptr, 0, nullptr);
    if (t) CloseHandle(t);
}

}  // namespace cl
