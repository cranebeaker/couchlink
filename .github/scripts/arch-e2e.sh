#!/bin/bash
# Runs inside an Arch Linux container (see .github/workflows/linux.yml): the Linux integration test with real
# gamescope (mode "gamescope") or with Proton through umu-launcher (mode "proton"). Arch packages both; Ubuntu does not.
set -euo pipefail
mode="$1"
# The workspace belongs to the runner; give it back however the run ends.
owner=$(stat -c %u:%g .)
trap 'chown -R "$owner" "$PWD"' EXIT
# Temporary folders on the runner's own disk: the container's root is an overlayfs, which the kernel will not put
# another overlay on, so copies made there could not be layered.
export TMPDIR=/w/.tmp
mkdir -p "$TMPDIR"

grep -q '^\[multilib\]' /etc/pacman.conf || printf '\n[multilib]\nInclude = /etc/pacman.d/mirrorlist\n' >> /etc/pacman.conf
packages=(git rust cmake make ninja pkgconf gcc python gamescope xcursor-themes adwaita-cursors vulkan-swrast
  libx11 libxext libxrandr libxi libxfixes libxcursor libxss libxtst wayland libxkbcommon dbus alsa-lib libpulse
  systemd-libs)
if [ "$mode" = proton ]; then
  packages+=(lib32-vulkan-swrast ttf-dejavu umu-launcher mingw-w64-gcc)
fi
pacman -Syu --noconfirm --needed "${packages[@]}" >/dev/null
gamescope --version 2>&1 | head -1 || true

export CARGO_TERM_COLOR=never COUCHLINK_SKIP_INJECT_TESTS=1 COUCHLINK_REQUIRE_SANDBOX_TEST=1
export COUCHLINK_REQUIRE_GAMESCOPE_TEST=1
# gamescope's window backend needs Vulkan present-wait, which software Vulkan lacks: run it headless.
export COUCHLINK_TEST_GAMESCOPE_HEADLESS=1

if [ "$mode" = gamescope ]; then
  export XDG_RUNTIME_DIR=/tmp/xdg-runtime COUCHLINK_KBM_VIRTUAL=1
  mkdir -p -m 0700 "$XDG_RUNTIME_DIR"
  cd core
  cargo test -p couchlinkd --test integration --locked -- --nocapture
  # SteamOS and most desktops keep /tmp/.X11-unix as root's, and players are not root: gamescope must still start
  # its Xwayland inside a seat's namespace, where that folder is nobody's.
  install -d -m 1777 -o root -g root /tmp/.X11-unix
  useradd -m player
  chown -R player: /w
  chmod 0666 /dev/udmabuf 2>/dev/null || true
  install -d -m 0700 -o player /tmp/xdg-player
  exec runuser -u player -- env HOME=/home/player USER=player XDG_RUNTIME_DIR=/tmp/xdg-player TMPDIR="$TMPDIR" \
    CARGO_TERM_COLOR=never COUCHLINK_REQUIRE_X11_DIR_TEST=1 \
    cargo test -p couchlinkd --bin couchlinkd --locked gamescope_starts_its_xwayland_in_a_seat_namespace -- --nocapture
fi

# Proton: umu-launcher refuses to run as root, so the test runs as a normal user.
useradd -m player
x86_64-w64-mingw32-gcc -O2 -o /tmp/winprobe.exe core/crates/couchlinkd/tests/winprobe.c
# The cached umu runtime and Proton, mounted from the runner.
mkdir -p /home/player/.local/share
[ -d /umu-cache/umu ] && cp -a /umu-cache/umu /home/player/.local/share/umu
[ -d /umu-cache/Steam ] && cp -a /umu-cache/Steam /home/player/.local/share/Steam
chown -R player: /home/player "$PWD"
# Software Vulkan shares gamescope's buffers through udmabuf, which only root may open by default.
chmod 0666 /dev/udmabuf 2>/dev/null || true
install -d -m 0700 -o player /tmp/xdg-runtime
status=0
runuser -u player -- env HOME=/home/player USER=player XDG_RUNTIME_DIR=/tmp/xdg-runtime TMPDIR="$TMPDIR" \
  CARGO_TERM_COLOR=never COUCHLINK_SKIP_INJECT_TESTS=1 COUCHLINK_REQUIRE_SANDBOX_TEST=1 \
  COUCHLINK_REQUIRE_GAMESCOPE_TEST=1 COUCHLINK_TEST_GAMESCOPE_HEADLESS=1 \
  COUCHLINK_REQUIRE_PROTON_TEST=1 COUCHLINK_TEST_WINPROBE=/tmp/winprobe.exe \
  sh -c 'cd core && cargo test -p couchlinkd --test integration --locked -- --nocapture' || status=$?
# Keep what umu downloaded for the next run.
rm -rf /umu-cache/umu /umu-cache/Steam
[ -d /home/player/.local/share/umu ] && cp -a /home/player/.local/share/umu /umu-cache/umu
[ -d /home/player/.local/share/Steam ] && cp -a /home/player/.local/share/Steam /umu-cache/Steam
exit $status
