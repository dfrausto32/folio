#!/usr/bin/env python3
"""Generate Folio's icon set (src-tauri/icons/*).

    python3 scripts/make_icon.py                 # draw the built-in Folio icon
    python3 scripts/make_icon.py my-logo.png     # use your own square PNG (512px+ recommended)

Needs Pillow (pip install pillow). Writes: icon.png (512), 32x32.png, 128x128.png,
128x128@2x.png and icon.ico (multi-size, for Windows).
"""
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

OUT = Path(__file__).resolve().parent.parent / "src-tauri" / "icons"
S = 1024  # master canvas; everything is drawn big and scaled down for smooth edges


def lerp(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))


def gradient(size, top, bottom):
    img = Image.new("RGB", (size, size))
    px = img.load()
    for y in range(size):
        for x in range(size):
            t = (x * 0.35 + y * 0.65) / size  # slight diagonal
            px[x, y] = lerp(top, bottom, min(1.0, t))
    return img


def rounded_mask(size, radius):
    m = Image.new("L", (size, size), 0)
    ImageDraw.Draw(m).rounded_rectangle((0, 0, size - 1, size - 1), radius=radius, fill=255)
    return m


def draw_builtin():
    # Background: deep indigo -> violet rounded square
    bg = gradient(S, (49, 46, 129), (124, 58, 237)).convert("RGBA")
    bg.putalpha(rounded_mask(S, int(S * 0.225)))

    # Soft drop shadow for the page
    shadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    sd = ImageDraw.Draw(shadow)
    px0, py0, px1, py1 = 250, 150, 774, 874  # page rectangle
    fold = 190
    page_poly = [(px0, py0), (px1 - fold, py0), (px1, py0 + fold), (px1, py1), (px0, py1)]
    sd.polygon([(x + 0, y + 22) for x, y in page_poly], fill=(15, 10, 60, 120))
    shadow = shadow.filter(ImageFilter.GaussianBlur(22))
    bg.alpha_composite(shadow)

    d = ImageDraw.Draw(bg)
    # The page, with its top-right corner folded over (a "folio")
    d.polygon(page_poly, fill=(255, 255, 255, 255))
    d.polygon([(px1 - fold, py0), (px1, py0 + fold), (px1 - fold, py0 + fold)], fill=(209, 213, 255, 255))

    # Text lines
    line = (148, 152, 190, 255)
    d.rounded_rectangle((330, 400, 694, 428), radius=14, fill=line)
    d.rounded_rectangle((330, 470, 694, 498), radius=14, fill=line)
    d.rounded_rectangle((330, 540, 560, 568), radius=14, fill=line)
    d.rounded_rectangle((330, 700, 640, 728), radius=14, fill=line)

    # A highlighter stroke over the middle line: the "mark it up" gesture
    hl = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    hd = ImageDraw.Draw(hl)
    hd.rounded_rectangle((304, 452, 720, 516), radius=22, fill=(255, 196, 46, 150))
    bg.alpha_composite(hl)

    # Bold "F" monogram in the upper-left of the page
    f = (49, 46, 129, 255)
    d = ImageDraw.Draw(bg)
    d.rounded_rectangle((330, 205, 392, 345), radius=12, fill=f)   # stem
    d.rounded_rectangle((330, 205, 520, 262), radius=12, fill=f)   # top bar
    d.rounded_rectangle((330, 253, 470, 304), radius=12, fill=f)   # mid bar
    return bg


def prepare_custom(src):
    """Trim a flat background around the artwork, make the outside of the tile transparent, square it up."""
    src = src.convert("RGBA")
    bgc = src.getpixel((2, 2))[:3]
    px = src.load()
    w, h = src.size
    xs, ys = [], []
    step = 2
    for y in range(0, h, step):
        for x in range(0, w, step):
            p = px[x, y]
            if sum(abs(p[i] - bgc[i]) for i in range(3)) > 60:
                xs.append(x); ys.append(y)
    if xs:
        src = src.crop((min(xs), min(ys), max(xs) + 1, max(ys) + 1))
    side = max(src.size)
    sq = Image.new("RGBA", (side, side), bgc + (255,))
    sq.paste(src, ((side - src.width) // 2, (side - src.height) // 2))
    for corner in ((0, 0), (side - 1, 0), (0, side - 1), (side - 1, side - 1)):
        ImageDraw.floodfill(sq, corner, (0, 0, 0, 0), thresh=60)
    return sq.resize((S, S), Image.LANCZOS)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    if len(sys.argv) > 1:
        master = prepare_custom(Image.open(sys.argv[1]))
    elif (OUT.parent.parent / "assets" / "icon-source.png").exists():
        master = prepare_custom(Image.open(OUT.parent.parent / "assets" / "icon-source.png"))
    else:
        master = draw_builtin()

    def save(name, px):
        master.resize((px, px), Image.LANCZOS).save(OUT / name)

    save("icon.png", 512)
    master.resize((256, 256), Image.LANCZOS).save(OUT.parent.parent / "src" / "icon.png")  # welcome screen
    save("32x32.png", 32)
    save("128x128.png", 128)
    save("128x128@2x.png", 256)
    master.save(OUT / "icon.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
    print("Icons written to", OUT)


if __name__ == "__main__":
    main()
