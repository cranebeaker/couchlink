"""Renders assets/brand/couchlink-mark.svg to PNGs and the platform icon files.

Run `node tools/brand/make_mark.mjs` first. Needs Pillow and Microsoft Edge (or Chrome) for the SVG render.

    python tools/brand/make_icons.py
"""

import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
SVG = ROOT / "assets" / "brand" / "couchlink-mark.svg"
PNG_DIR = ROOT / "assets" / "brand" / "png"
APP = ROOT / "app"
BROWSERS = [
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    "msedge", "google-chrome", "chromium",
]
MASTER = 1024
SIZES = [16, 24, 32, 48, 64, 128, 256, 512, 1024]


def browser() -> str:
    for b in BROWSERS:
        if os.path.isfile(b) or shutil.which(b):
            return b
    sys.exit("No Edge or Chrome found for rendering the SVG.")


def render_master(tmp: Path) -> Image.Image:
    page = tmp / "mark.html"
    page.write_text(
        "<!doctype html><html><head><style>html,body{margin:0;background:transparent}"
        f"img{{display:block;width:{MASTER}px;height:{MASTER}px}}</style></head>"
        f'<body><img src="{SVG.as_uri()}"></body></html>',
        encoding="utf-8",
    )
    shot = tmp / "master.png"
    subprocess.run(
        [browser(), "--headless=new", "--disable-gpu", "--hide-scrollbars", "--force-device-scale-factor=1",
         f"--window-size={MASTER},{MASTER}", "--default-background-color=00000000",
         f"--user-data-dir={tmp / 'profile-master'}", f"--screenshot={shot}", page.as_uri()],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    for _ in range(150):  # the browser launcher can return before the file is written
        if shot.exists():
            break
        time.sleep(0.2)
    img = Image.open(shot).convert("RGBA")
    if img.size != (MASTER, MASTER):
        img = img.crop((0, 0, MASTER, MASTER))
    if img.getpixel((0, 0))[3] != 0:
        sys.exit("The render has no transparent background; check the browser flags.")
    return img


FONT = Path(os.environ.get("WINDIR", r"C:\Windows")) / "Fonts" / "SegUIVar.ttf"  # Segoe UI Variable


def make_lockup(master: Image.Image, ink: tuple, name: str) -> None:
    """Mark + "Couchlink" wordmark in Segoe UI Variable Display, bold, tracked tight. Transparent, 116 px tall."""
    from PIL import ImageDraw, ImageFont

    if not FONT.exists():
        print(f"skipped {name}: {FONT} not found")
        return
    size, mark, gap, track = 80, 116, 26, -0.03
    font = ImageFont.truetype(str(FONT), size)
    try:
        font.set_variation_by_axes([700, 36])  # weight, optical size (Display)
    except Exception:
        font.set_variation_by_axes([700])
    widths = [font.getlength(ch) for ch in "Couchlink"]
    text_w = int(sum(widths) + track * size * (len(widths) - 1)) + 4
    asc, desc = font.getmetrics()
    img = Image.new("RGBA", (mark + gap + text_w, mark), (0, 0, 0, 0))
    img.alpha_composite(master.resize((mark, mark), Image.LANCZOS), (0, 0))
    draw = ImageDraw.Draw(img)
    x, y = mark + gap, (mark - (asc + desc)) // 2 + 2
    for ch, w in zip("Couchlink", widths):
        draw.text((x, y), ch, font=font, fill=ink)
        x += w + track * size
    img.crop(img.getbbox()).save(ROOT / "assets" / "brand" / f"{name}.png")


def main() -> None:
    if not SVG.exists():
        sys.exit(f"{SVG} is missing; run node tools/brand/make_mark.mjs first.")
    # A scratch folder inside the repo: the Microsoft Store Python redirects its writes under AppData,
    # where the browser would not see them.
    tmp = ROOT / ".brand-tmp"
    shutil.rmtree(tmp, ignore_errors=True)
    tmp.mkdir(exist_ok=True)
    try:
        master = render_master(tmp)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    make_lockup(master, (242, 242, 245, 255), "couchlink-lockup-on-dark")
    make_lockup(master, (29, 29, 31, 255), "couchlink-lockup-on-light")

    PNG_DIR.mkdir(parents=True, exist_ok=True)
    sized = {s: master.resize((s, s), Image.LANCZOS) for s in SIZES}
    for s, im in sized.items():
        im.save(PNG_DIR / f"couchlink-{s}.png")

    # Windows: one .ico with every size the shell asks for.
    ico = APP / "windows" / "runner" / "resources" / "app_icon.ico"
    sized[256].save(ico, sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])

    # macOS: replace the template's AppIcon set (same file names).
    mac = APP / "macos" / "Runner" / "Assets.xcassets" / "AppIcon.appiconset"
    for s in (16, 32, 64, 128, 256, 512, 1024):
        sized[s].save(mac / f"app_icon_{s}.png")

    # Linux and the in-app title bar use a PNG bundled as a Flutter asset.
    assets = APP / "assets" / "brand"
    assets.mkdir(parents=True, exist_ok=True)
    sized[256].save(assets / "couchlink-256.png")
    sized[64].save(assets / "couchlink-64.png")

    print(f"wrote {len(SIZES)} PNGs to {PNG_DIR}, {ico.name}, the macOS AppIcon set and app/assets/brand")


if __name__ == "__main__":
    main()
