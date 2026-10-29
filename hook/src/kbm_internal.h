// Internal interface between the keyboard / mouse feed state (kbm_hook.cpp) and the parts that read it
// (kbm_dinput.cpp, kbm_cursor.cpp).
#pragma once
#include "common.h"

namespace cl {

struct DiEvent {
    uint32_t ofs;   // DirectInput object offset (keyboard: DIK_* code, mouse: DIMOFS_*)
    uint32_t data;  // keyboard: 0x80 pressed / 0 released, mouse axes: delta, mouse buttons: 0x80 / 0
    uint32_t time;  // GetTickCount()
};

// Switches the DirectInput queues on (they stay empty until a game creates such a device).
void KbmDiEnable(bool keyboard, bool mouse);
// Buffered data since the last call (peek = leave it queued). Returns the number of events copied to `out`.
size_t KbmDiTakeKeys(DiEvent* out, size_t max, bool peek);
size_t KbmDiTakeMouse(DiEvent* out, size_t max, bool peek);
// Immediate state: DIK-indexed keyboard bytes (0x80 = down); mouse movement since the last consuming call and
// the first 8 buttons.
void KbmDiKeyState(uint8_t out[256]);
void KbmDiMouseState(int32_t* x, int32_t* y, int32_t* z, uint8_t buttons[8], bool consume);

// Fake cursor as the overlay needs it.
struct KbmCursorView {
    bool visible = false;  // the game wants a cursor and the feed is attached
    POINT screen = {0, 0};
    HCURSOR shape = nullptr;
    HWND window = nullptr;  // main window the cursor lives in
};
KbmCursorView KbmCursor();
void StartFakeCursor();

}  // namespace cl
