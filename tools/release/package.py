"""Builds a Couchlink release zip for Windows.

    python -I tools/release/package.py [--version 0.1.4] [--build-root C:\\cl-build]

Compilers bake the folder a program was built in into the binaries (panic messages, PDB paths, asset
paths). A personal folder name must never ship, so the whole build runs from a neutral folder outside the
user profile, with its own Cargo and pub caches, and every file of the result is scanned for the current
Windows user name before the zip is written. The scan reads that name at run time; it is never written here.

Output: dist/Couchlink-<version>-windows-x64.zip
"""
import argparse
import os
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SKIP_DIRS = ["target", "build", ".dart_tool", ".brand-tmp", "dist", ".git", "ephemeral"]


def run(cmd, cwd, env):
    print(">", " ".join(str(c) for c in cmd), flush=True)
    subprocess.run([str(c) for c in cmd], cwd=cwd, env=env, check=True, shell=False)


def copy_sources(dst):
    """Mirror the repo into the build folder, minus build output."""
    dst.mkdir(parents=True, exist_ok=True)
    excludes = []
    for d in REPO.rglob("*"):
        if d.is_dir() and (d.name in SKIP_DIRS or d.name.startswith("target-")):
            excludes.append(str(d))
    # robocopy exit codes below 8 mean success.
    code = subprocess.call(["robocopy", str(REPO), str(dst), "/MIR", "/NFL", "/NDL", "/NJH", "/NJS", "/NP", "/XD", *excludes])
    if code >= 8:
        raise SystemExit(f"robocopy failed ({code})")


def forbidden_words():
    words = set()
    for v in (os.environ.get("USERNAME"), Path.home().name):
        if v and len(v) >= 4:
            words.add(v.lower())
    return sorted(words)


def scan(root, words):
    """Every file, as bytes, for each word in ASCII and UTF-16LE, any case."""
    hits = []
    pats = []
    for w in words:
        for variant in {w, w.upper(), w.capitalize()}:
            pats.append(variant.encode("ascii"))
            pats.append(variant.encode("utf-16-le"))
    for f in root.rglob("*"):
        if f.is_file():
            data = f.read_bytes()
            low = data.lower()
            if any(p.lower() in low for p in pats):
                hits.append(str(f.relative_to(root)))
    return hits


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", default="0.1.4")
    ap.add_argument("--build-root", default=r"C:\cl-build")
    ap.add_argument("--skip-build", action="store_true", help="reuse the last build in the build root")
    args = ap.parse_args()

    words = forbidden_words()
    root = Path(args.build_root)
    if any(w in str(root).lower() for w in words):
        raise SystemExit("the build root must not be inside the user profile")
    src = root / "couchlink"
    env = dict(os.environ)
    env.pop("CMAKE_TOOLCHAIN_FILE", None)
    env["CARGO_HOME"] = str(root / "cargo-home")
    env["CARGO_TARGET_DIR"] = str(root / "target")
    env["PUB_CACHE"] = str(root / "pub-cache")
    env["RUSTFLAGS"] = f"--remap-path-prefix={root}=build"
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo" / "bin" / "cargo.exe")
    flutter = shutil.which("flutter") or str(Path.home() / "flutter" / "bin" / "flutter.bat")

    if not args.skip_build:
        copy_sources(src)
        # rustup's toolchain lives in the profile; only the compiler runs from there, nothing of it ships.
        env.setdefault("RUSTUP_HOME", str(Path.home() / ".rustup"))
        run([cargo, "build", "--release", "-p", "couchlinkd", "--locked"], src / "core", env)
        run(["powershell", "-NoProfile", "-File", src / "hook" / "build.ps1", "-Config", "Release"], src / "hook", env)
        run([flutter, "build", "windows", "--release"], src / "app", env)

    dist = REPO / "dist" / f"Couchlink-{args.version}"
    if dist.exists():
        shutil.rmtree(dist)
    app_out = src / "app" / "build" / "windows" / "x64" / "runner" / "Release"
    shutil.copytree(app_out, dist, ignore=shutil.ignore_patterns("*.pdb", "*.lib", "*.exp", "*.ilk"))
    shutil.copy2(root / "target" / "release" / "couchlinkd.exe", dist / "couchlinkd.exe")
    for arch in ("x64", "x86"):
        (dist / "hook" / arch).mkdir(parents=True)
        shutil.copy2(src / "hook" / "build" / arch / "couchlink_hook.dll", dist / "hook" / arch / "couchlink_hook.dll")
        # The stand-in system DLLs (version.dll, xinput1_3.dll, ...) a game loads by itself; injection is only the fallback.
        proxy_dir = dist / "hook" / arch / "proxy"
        proxy_dir.mkdir()
        stand_ins = sorted((src / "hook" / "build" / arch / "proxy").glob("*.dll"))
        if len(stand_ins) != 6:
            raise SystemExit(f"expected 6 stand-in DLLs for {arch}, found {len(stand_ins)}; build the hook first")
        for dll in stand_ins:
            shutil.copy2(dll, proxy_dir / dll.name)
    (dist / "recipes").mkdir()
    for f in (src / "core" / "recipes").glob("*.json"):
        if f.stem != "testgame":
            shutil.copy2(f, dist / "recipes" / f.name)
    for name in ("LICENSE", "README.md"):
        if (src / name).exists():
            shutil.copy2(src / name, dist / name)
    if (src / "hook" / "THIRD_PARTY.md").exists():
        shutil.copy2(src / "hook" / "THIRD_PARTY.md", dist / "THIRD_PARTY.md")

    hits = scan(dist, words)
    if hits:
        print("REFUSING TO PACKAGE: personal folder name found in:", *hits, sep="\n  ")
        return 1
    zip_path = REPO / "dist" / f"Couchlink-{args.version}-windows-x64.zip"
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as z:
        for f in sorted(dist.rglob("*")):
            if f.is_file():
                z.write(f, Path(f"Couchlink-{args.version}") / f.relative_to(dist))
    print(f"scan clean; wrote {zip_path} ({zip_path.stat().st_size // 1024} KB)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
