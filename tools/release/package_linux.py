"""Builds a Couchlink release tarball for Linux.

    python3 -I tools/release/package_linux.py [--version 0.1.9] [--build-root /tmp/cl-build]
    python3 -I tools/release/package_linux.py --version 0.1.9 --daemon PATH --app-bundle DIR

Without --daemon and --app-bundle it builds both, from a neutral folder outside the user's home (with its own
Cargo and pub caches), because compilers bake the build folder into the binaries. CI passes what its jobs built.
Either way every file of the result is scanned for the current user name (and the words in
COUCHLINK_PRIVATE_WORDS, comma separated) before the tarball is written. The scan reads those at run time;
they are never written here.

Output: dist/Couchlink-<version>-linux-x64.tar.gz
"""
import argparse
import os
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SKIP_DIRS = ["target", "build", ".dart_tool", ".brand-tmp", "dist", ".git", "ephemeral", ".claude"]

DESKTOP = """[Desktop Entry]
Type=Application
Name=Couchlink
Comment=Split-screen couch co-op for any game
Exec=couchlink
Icon=couchlink
Terminal=false
Categories=Game;
"""

# What the host needs for split screen, checked by couchlinkd at session start; the README lists them for packagers.
NEEDS = """Couchlink for Linux

Run ./couchlink from this folder. couchlinkd, next to it, is started by the app.

Nothing else is needed on a normal Steam desktop: Couchlink gives each player's copy of a game a sandbox of its own
(only that player's controller is in it) and a copy of the game that takes no extra disk, by itself. Windows games run
through the Proton you already have in Steam. The Goldberg Steam emulator (gbe_fork, LGPL-3.0, licence and source
link in goldberg/) is included, so Steam-only co-op games need no download.

Optional, each for something more:

  gamescope        a window of its own for every player, and a keyboard and mouse of their own per player
                   (without it the game windows are placed side by side directly, and keyboards go to the
                   focused window)
  the input group  so Couchlink can see your keyboards and mice for keyboard players:
                   sudo usermod -aG input $USER, then log in again
  umu-launcher     (umu-run) used instead of Steam's Proton when it is installed

SteamOS Game Mode shows one window at a time. There Couchlink starts KWin (kwin_wayland, part of SteamOS's desktop)
inside Game Mode and puts every player's window in it, side by side, so add Couchlink to Steam and start it from
Game Mode as you would a game. Elsewhere it needs the kwin package for that.

Ubuntu 24.04 and later stop programs from making sandboxes unless allowed (AppArmor). Couchlink then uses bubblewrap
if it is installed, and tells you what to do if Proton cannot start inside the sandbox.

Something wrong? Couchlink keeps its logs in ~/.local/state/couchlink/logs ($XDG_STATE_HOME/couchlink/logs
when that is set): couchlinkd.log and one per game copy. Copy log in the app puts their ends on the clipboard.
"""


def run(cmd, cwd, env):
    print(">", " ".join(str(c) for c in cmd), flush=True)
    subprocess.run([str(c) for c in cmd], cwd=cwd, env=env, check=True, shell=False)


def copy_sources(dst):
    """Mirror the repo into the build folder, minus build output."""
    if dst.exists():
        shutil.rmtree(dst)

    def ignore(folder, names):
        return [n for n in names if n in SKIP_DIRS or n.startswith("target-")]

    shutil.copytree(REPO, dst, ignore=ignore, symlinks=True)


def forbidden_words():
    words = set()
    for v in (os.environ.get("USER"), os.environ.get("LOGNAME"), Path.home().name):
        if v and len(v) >= 4 and v not in ("root", "runner"):
            words.add(v.lower())
    for v in os.environ.get("COUCHLINK_PRIVATE_WORDS", "").split(","):
        if len(v.strip()) >= 4:
            words.add(v.strip().lower())
    return sorted(words)


def scan(root, words):
    """Every file, as bytes, for each word in ASCII and UTF-16LE, any case."""
    hits = []
    pats = []
    for w in words:
        pats.append(w.encode("utf-8"))
        pats.append(w.encode("utf-16-le"))
    for f in root.rglob("*"):
        if f.is_file() and not f.is_symlink():
            low = f.read_bytes().lower()
            if any(p in low for p in pats):
                hits.append(str(f.relative_to(root)))
    return hits


def build(root, words):
    """Build couchlinkd and the app in `root`; returns (daemon, bundle)."""
    if any(w in str(root).lower() for w in words):
        raise SystemExit("the build root must not be inside the user's home")
    src = root / "couchlink"
    env = dict(os.environ)
    env["CARGO_HOME"] = str(root / "cargo-home")
    env["CARGO_TARGET_DIR"] = str(root / "target")
    env["PUB_CACHE"] = str(root / "pub-cache")
    env["RUSTFLAGS"] = f"--remap-path-prefix={root}=build"
    env.setdefault("RUSTUP_HOME", str(Path.home() / ".rustup"))
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo" / "bin" / "cargo")
    flutter = shutil.which("flutter") or str(Path.home() / "flutter" / "bin" / "flutter")
    copy_sources(src)
    run([cargo, "build", "--release", "-p", "couchlinkd", "--locked"], src / "core", env)
    run([flutter, "build", "linux", "--release"], src / "app", env)
    goldberg = root / "goldberg"
    run([cargo, "run", "--release", "--locked", "-p", "couchlink-steam", "--example", "goldberg_bundle", "--", goldberg], src / "core", env)
    return root / "target" / "release" / "couchlinkd", src / "app" / "build" / "linux" / "x64" / "release" / "bundle", goldberg


def tar_filter(info):
    # No owner names from the build machine in the archive.
    info.uid = info.gid = 0
    info.uname = info.gname = ""
    return info


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", default="0.1.9")
    ap.add_argument("--build-root", default="/tmp/cl-build")
    ap.add_argument("--daemon", help="a couchlinkd that is already built")
    ap.add_argument("--app-bundle", help="a Flutter Linux bundle folder that is already built")
    ap.add_argument("--goldberg", help="a gbe_fork bundle folder (the goldberg_bundle example's output)")
    args = ap.parse_args()

    words = forbidden_words()
    if bool(args.daemon) != bool(args.app_bundle):
        raise SystemExit("pass both --daemon and --app-bundle, or neither")
    if args.daemon:
        daemon, bundle = Path(args.daemon), Path(args.app_bundle)
        if not args.goldberg:
            raise SystemExit("pass --goldberg too: the tarball ships gbe_fork")
        goldberg = Path(args.goldberg)
    else:
        daemon, bundle, goldberg = build(Path(args.build_root), words)

    name = f"Couchlink-{args.version}"
    dist = REPO / "dist" / name
    if dist.exists():
        shutil.rmtree(dist)
    shutil.copytree(bundle, dist)
    (dist / "couchlink").chmod(0o755)
    shutil.copy2(daemon, dist / "couchlinkd")
    (dist / "couchlinkd").chmod(0o755)
    (dist / "recipes").mkdir()
    for f in (REPO / "core" / "recipes").glob("*.json"):
        if f.stem != "testgame":
            shutil.copy2(f, dist / "recipes" / f.name)
    for f in ("LICENSE", "README.md"):
        if (REPO / f).exists():
            shutil.copy2(REPO / f, dist / f)
    (dist / "share").mkdir()
    (dist / "share" / "couchlink.desktop").write_text(DESKTOP)
    shutil.copy2(REPO / "assets" / "brand" / "png" / "couchlink-256.png", dist / "share" / "couchlink.png")
    (dist / "LINUX.txt").write_text(NEEDS)
    # gbe_fork (LGPL-3.0, licence and source link inside), so Steam games run with no download.
    if not (goldberg / "gbe_fork").is_dir():
        raise SystemExit(f"no gbe_fork/ in {goldberg}")
    shutil.copytree(goldberg / "gbe_fork", dist / "goldberg" / "gbe_fork")

    hits = scan(dist, words)
    if hits:
        print("REFUSING TO PACKAGE: a private word was found in:", *hits, sep="\n  ")
        return 1
    out = REPO / "dist" / f"{name}-linux-x64.tar.gz"
    with tarfile.open(out, "w:gz") as t:
        t.add(dist, arcname=name, filter=tar_filter)
    print(f"scan clean ({len(words)} words); wrote {out} ({out.stat().st_size // 1024} KB)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
