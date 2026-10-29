// Layout of the keyboard and mouse feed ("kbm feed"): a named shared-memory section that the daemon writes (the raw
// key and mouse events of the devices one seat owns) and every hooked process of that copy reads. The hook also
// writes a few things back (heartbeat, fake cursor position) so the daemon and the tests can see them.
// The Rust twin is core/crates/couchlink-hook/src/kbmfeed.rs; keep both in step. Little endian, naturally aligned.
//
//   0 .. 127      KbmHeader
//   128 ..        KbmEvent ring[kKbmRingCap], one writer (the daemon), any number of readers (each keeps its own
//                 read position and starts at `head` when it attaches)
#pragma once
#include <cstdint>

namespace cl {

constexpr uint32_t kKbmMagic = 0x4D4B4C43;  // "CLKM"
constexpr uint32_t kKbmVersion = 1;
constexpr uint32_t kKbmFlagConnected = 1;
constexpr uint32_t kKbmMaxDevices = 8;
constexpr uint32_t kKbmHeaderSize = 128;
constexpr uint32_t kKbmRingCap = 1024;  // events, a power of two
constexpr uint32_t kKbmEventSize = 16;
constexpr uint32_t kKbmTotalSize = kKbmHeaderSize + kKbmRingCap * kKbmEventSize;

// KbmEvent::kind
constexpr uint8_t kKbmKey = 1;
constexpr uint8_t kKbmMouse = 2;

// KbmEvent::c for keys
constexpr uint16_t kKbmKeyBreak = 1;  // key released (RI_KEY_BREAK)
constexpr uint16_t kKbmKeyE0 = 2;     // extended key (RI_KEY_E0)
constexpr uint16_t kKbmKeyE1 = 4;     // Pause (RI_KEY_E1)

struct KbmHeader {
    uint32_t magic;           //  0  kKbmMagic
    uint32_t version;         //  4  kKbmVersion
    uint32_t flags;           //  8  bit 0: seat has devices (connected)
    uint32_t head;            // 12  number of events ever written; event n lives in ring[n % kKbmRingCap]
    uint32_t capacity;        // 16  kKbmRingCap
    uint32_t device_count;    // 20  devices of the seat, indices used by KbmEvent::dev
    uint32_t game_beat;       // 24  GetTickCount() of the last time a hook of this copy drained the ring
    uint32_t host_beat;       // 28  GetTickCount() of the last daemon write
    int32_t cursor_x;         // 32  fake cursor, client coordinates of the copy's main window (written by the hook)
    int32_t cursor_y;         // 36
    uint32_t cursor_visible;  // 40  1 when the game currently wants a visible cursor (written by the hook)
    uint32_t events_seen;     // 44  events the hook has consumed (the first reader's count)
    uint32_t hook_pid;        // 48  process that wrote cursor_x/y
    uint32_t reserved0[3];    // 52
    uint64_t dev_handles[8];  // 64  raw input device handle of each device (RAWINPUTHEADER::hDevice), 0 = unknown
};

struct KbmEvent {
    uint8_t kind;  // kKbmKey or kKbmMouse
    uint8_t dev;   // index into the seat's devices
    uint16_t a;    // key: virtual key (raw VKey);            mouse: RI_MOUSE_* button flags (usButtonFlags)
    uint16_t b;    // key: scan code (MakeCode);              mouse: usButtonData (wheel delta, signed)
    uint16_t c;    // key: kKbmKey* flags;                    mouse: unused
    int32_t x;     // key: unused;                            mouse: relative X (lLastX)
    int32_t y;     // key: unused;                            mouse: relative Y (lLastY)
};

static_assert(sizeof(KbmHeader) == kKbmHeaderSize, "KbmHeader must be 128 bytes");
static_assert(sizeof(KbmEvent) == kKbmEventSize, "KbmEvent must be 16 bytes");
static_assert((kKbmRingCap & (kKbmRingCap - 1)) == 0, "ring size is a power of two");

}  // namespace cl
