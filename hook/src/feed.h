// Layout of the pad feed: a named shared-memory section the daemon writes (pad state) and the hook reads, and the
// hook writes (rumble, game heartbeat) and the daemon reads. The Rust twin is core/crates/couchlink-hook/src/feed.rs;
// keep both in step. Little endian, 64 bytes, every field naturally aligned.
#pragma once
#include <cstdint>

namespace cl {

constexpr uint32_t kFeedMagic = 0x46504C43;  // "CLPF"
constexpr uint32_t kFeedVersion = 1;
constexpr uint32_t kFeedFlagConnected = 1;
constexpr uint32_t kFeedSize = 64;

struct FeedBlock {
    uint32_t magic;          //  0  kFeedMagic
    uint32_t version;        //  4  kFeedVersion
    uint32_t seq;            //  8  seqlock: odd while the daemon is writing the state fields
    uint32_t packet;         // 12  XINPUT_STATE.dwPacketNumber
    uint16_t buttons;        // 16  XINPUT_GAMEPAD.wButtons (0x0400 = guide, only shown by XInputGetStateEx)
    uint8_t lt;              // 18  trigger 0..255
    uint8_t rt;              // 19
    int16_t lx;              // 20  sticks in XInput orientation (Y up)
    int16_t ly;              // 22
    int16_t rx;              // 24
    int16_t ry;              // 26
    uint32_t flags;          // 28  bit 0: pad connected
    uint16_t rumble_left;    // 32  written by the hook (XInputSetState)
    uint16_t rumble_right;   // 34
    uint32_t rumble_seq;     // 36  incremented by the hook after each XInputSetState
    uint32_t game_beat;      // 40  GetTickCount() of the last state read by the game
    uint32_t host_beat;      // 44  GetTickCount() of the last daemon write
    uint32_t reserved[4];    // 48
};

static_assert(sizeof(FeedBlock) == kFeedSize, "FeedBlock must be 64 bytes");

}  // namespace cl
