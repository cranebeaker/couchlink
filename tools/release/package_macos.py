"""Builds a Couchlink release for macOS: Couchlink.app in a zip (default) or a dmg.

    python3 -I tools/release/package_macos.py [--version 0.1.0] [--build-root /Users/Shared/cl-build] [--dmg]

What goes into Couchlink.app/Contents:
    MacOS/Couchlink                    the Flutter app
    MacOS/couchlinkd                   the daemon (the app starts it from next to itself)
    MacOS/libcouchlink_inject.dylib    the input library a game copy loads (docs/macos.md)
    Resources/recipes/*.json           the built-in recipes

Signing is ad-hoc only (codesign --sign -). A Developer ID certificate shows its owner's real name, so this script
never signs or notarizes with a personal Apple ID. Users open the app once through Gatekeeper (docs/macos.md).

Compilers bake the folder a program was built in into the binaries. A personal folder name must never ship, so the
build runs from a neutral folder outside the home folder, with its own Cargo and pub caches, and every file of the
result is scanned before the archive is written for the Mac user name and the user's full name. Both are read at run
time (id -un, id -F); they are never written in this script.

Output: dist/Couchlink-<version>-macos-<arch>.zip (or .dmg)
"""
import argparse
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SKIP_DIRS = ["target", "build", ".dart_tool", ".brand-tmp", "dist", ".git", "ephemeral", "Pods"]
FRAMEWORKS = ["Foundation", "AppKit", "GameController", "IOKit", "CoreGraphics"]


def run(cmd, cwd, env):
    print(">", " ".join(str(c) for c in cmd), flush=True)
    subprocess.run([str(c) for c in cmd], cwd=cwd, env=env, check=True, shell=False)


def output(cmd):
    try:
        return subprocess.run(cmd, capture_output=True, text=True, check=False).stdout.strip()
    except OSError:
        return ""


def forbidden_words():
    """The Mac user name, the home folder name and the user's full name (and each part of it), read now."""
    words = set()
    full = output(["id", "-F"])
    for v in [output(["id", "-un"]), os.environ.get("USER"), Path.home().name, full, *full.split()]:
        if v and len(v) >= 4:
            words.add(v.lower())
    return sorted(words)


def scan(root, words):
    """Every file, as bytes, for each word in ASCII and UTF-16LE, any case. Symbolic links are not followed."""
    pats = []
    for w in words:
        pats.append(w.encode("utf-8"))
        pats.append(w.encode("utf-16-le"))
    hits = []
    for f in sorted(root.rglob("*")):
        if f.is_file() and not f.is_symlink():
            low = f.read_bytes().lower()
            if any(p in low for p in pats):
                hits.append(str(f.relative_to(root)))
    return hits


def copy_sources(dst):
    """Mirror the repo into the build folder, minus build output."""
    dst.mkdir(parents=True, exist_ok=True)
    excludes = [f"--exclude={d}/" for d in SKIP_DIRS] + ["--exclude=target-*/"]
    run(["rsync", "-a", "--delete", *excludes, f"{REPO}/", f"{dst}/"], REPO, os.environ)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", default="0.1.0")
    ap.add_argument("--build-root", default="/Users/Shared/cl-build")
    ap.add_argument("--arch", default="arm64" if platform.machine() == "arm64" else "x86_64", choices=["arm64", "x86_64"])
    ap.add_argument("--dmg", action="store_true", help="write a dmg instead of a zip")
    ap.add_argument("--skip-build", action="store_true", help="reuse the last build in the build root")
    args = ap.parse_args()

    words = forbidden_words()
    if not words:
        raise SystemExit("could not read the user name to scan for")
    root = Path(args.build_root)
    if any(w in str(root).lower() for w in words) or str(root).startswith(str(Path.home())):
        raise SystemExit("the build root must not be inside the home folder")
    src = root / "couchlink"
    rust_target = "aarch64-apple-darwin" if args.arch == "arm64" else "x86_64-apple-darwin"
    env = dict(os.environ)
    env.pop("CMAKE_TOOLCHAIN_FILE", None)
    env["CARGO_HOME"] = str(root / "cargo-home")
    env["CARGO_TARGET_DIR"] = str(root / "target")
    env["PUB_CACHE"] = str(root / "pub-cache")
    env["RUSTFLAGS"] = f"--remap-path-prefix={root}=build"
    # rustup's toolchain lives in the home folder; only the compiler runs from there, nothing of it ships.
    env.setdefault("RUSTUP_HOME", str(Path.home() / ".rustup"))
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo" / "bin" / "cargo")
    flutter = shutil.which("flutter") or "flutter"
    built = root / "macos"

    if not args.skip_build:
        copy_sources(src)
        run([cargo, "build", "--release", "-p", "couchlinkd", "--locked", "--target", rust_target], src / "core", env)
        built.mkdir(parents=True, exist_ok=True)
        clang = ["clang", "-fobjc-arc", "-O2", "-mmacosx-version-min=11.0", "-arch", args.arch]
        for fw in FRAMEWORKS:
            clang += ["-framework", fw]
        run([*clang, "-dynamiclib", "-install_name", "@rpath/libcouchlink_inject.dylib", "-o", built / "libcouchlink_inject.dylib",
             src / "hook" / "macos" / "couchlink_inject.m"], src, env)
        run([flutter, "build", "macos", "--release"], src / "app", env)

    dist = REPO / "dist" / f"Couchlink-{args.version}-macos"
    if dist.exists():
        shutil.rmtree(dist)
    dist.mkdir(parents=True)
    app = dist / "Couchlink.app"
    # symlinks=True keeps the framework links the signature covers.
    shutil.copytree(src / "app" / "build" / "macos" / "Build" / "Products" / "Release" / "Couchlink.app", app, symlinks=True)
    macos_dir = app / "Contents" / "MacOS"
    shutil.copy2(root / "target" / rust_target / "release" / "couchlinkd", macos_dir / "couchlinkd")
    shutil.copy2(built / "libcouchlink_inject.dylib", macos_dir / "libcouchlink_inject.dylib")
    recipes = app / "Contents" / "Resources" / "recipes"
    recipes.mkdir(parents=True, exist_ok=True)
    for f in sorted((src / "core" / "recipes").glob("*.json")):
        if f.stem != "testgame":
            shutil.copy2(f, recipes / f.name)
    for name in ("LICENSE", "README.md"):
        if (src / name).exists():
            shutil.copy2(src / name, dist / name)
    if (src / "docs" / "macos.md").exists():
        shutil.copy2(src / "docs" / "macos.md", dist / "macOS.md")

    # Ad-hoc signature over everything, inside out (the daemon and the library first, then the bundle).
    for code in (macos_dir / "couchlinkd", macos_dir / "libcouchlink_inject.dylib"):
        run(["codesign", "--force", "--sign", "-", code], dist, env)
    run(["codesign", "--force", "--deep", "--sign", "-", app], dist, env)
    run(["codesign", "--verify", "--deep", "--strict", app], dist, env)
    # A signature must never carry a name: ad-hoc signatures have no authority at all.
    info = subprocess.run(["codesign", "-dvv", str(app)], capture_output=True, text=True).stderr
    if "Authority=" in info or "Signature=adhoc" not in info:
        print(info)
        raise SystemExit("REFUSING TO PACKAGE: the app is not ad-hoc signed")

    hits = scan(dist, words)
    if hits:
        print("REFUSING TO PACKAGE: personal name found in:", *hits, sep="\n  ")
        return 1
    out = REPO / "dist" / f"Couchlink-{args.version}-macos-{args.arch}.{'dmg' if args.dmg else 'zip'}"
    if out.exists():
        out.unlink()
    if args.dmg:
        run(["hdiutil", "create", "-volname", f"Couchlink {args.version}", "-srcfolder", dist, "-ov", "-format", "UDZO", out], REPO, env)
    else:
        # ditto keeps symbolic links and extended attributes, which zipfile would flatten.
        run(["ditto", "-c", "-k", "--keepParent", dist, out], REPO, env)
    print(f"scan clean ({len(words)} words, {sum(1 for f in dist.rglob('*') if f.is_file())} files); wrote {out} ({out.stat().st_size // 1024} KB)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
