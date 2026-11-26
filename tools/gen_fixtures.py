#!/usr/bin/env python3
"""Creates the test images in tests/fixtures (requirement QA-01).

All images are drawn by this script, so there is no license problem: the
images can be stored in the repository. The random numbers use fixed seeds,
so the script always creates the same files.

Usage: python3 tools/gen_fixtures.py
Requires: Pillow.
"""
import math
import pathlib
import random

from PIL import Image, ImageDraw, ImageFilter, ImageFont

OUT = pathlib.Path(__file__).resolve().parent.parent / "tests" / "fixtures"
OUT.mkdir(parents=True, exist_ok=True)


def supersampled(size, draw_fn, factor=4):
    """Draws at a bigger size and scales down with a box filter (area average), the way
    a rasterizer computes pixel coverage, to get smooth (anti-aliased) edges."""
    big = Image.new("RGBA", (size[0] * factor, size[1] * factor), (0, 0, 0, 0))
    draw_fn(ImageDraw.Draw(big), factor)
    return big.resize(size, Image.BOX)


def logo():
    """Flat colors: a circle, a ring, a triangle and a bar, on a transparent background."""
    def draw(d, f0):
        f = f0 * 2  # the drawing is made for 256 pixels and shown at 512
        d.ellipse([20 * f, 20 * f, 120 * f, 120 * f], fill=(0xE5, 0x39, 0x35, 255))
        d.ellipse([136 * f, 20 * f, 236 * f, 120 * f], fill=(0x1E, 0x88, 0xE5, 255))
        d.ellipse([160 * f, 44 * f, 212 * f, 96 * f], fill=(0, 0, 0, 0))
        d.polygon([(20 * f, 236 * f), (120 * f, 236 * f), (70 * f, 140 * f)], fill=(0x43, 0xA0, 0x47, 255))
        d.rectangle([136 * f, 140 * f, 236 * f, 236 * f], fill=(0xFD, 0xD8, 0x35, 255))
        d.rectangle([152 * f, 172 * f, 220 * f, 204 * f], fill=(0x21, 0x21, 0x21, 255))
    supersampled((512, 512), draw).save(OUT / "logo.png", optimize=True)


def icon():
    """An icon with a soft semi-transparent shadow and anti-aliased edges."""
    size = (256, 256)
    shadow = Image.new("RGBA", size, (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle([28, 36, 236, 244], radius=44, fill=(0, 0, 0, 150))
    shadow = shadow.filter(ImageFilter.GaussianBlur(6))

    def draw(d, f0):
        f = f0 * 2  # the drawing is made for 128 pixels and shown at 256
        d.rounded_rectangle([8 * f, 8 * f, 108 * f, 108 * f], radius=22 * f, fill=(0x5E, 0x35, 0xB1, 255))
        d.ellipse([34 * f, 30 * f, 82 * f, 78 * f], fill=(0xFF, 0xFF, 0xFF, 255))
        d.polygon([(46 * f, 44 * f), (46 * f, 64 * f), (66 * f, 54 * f)], fill=(0x5E, 0x35, 0xB1, 255))
        d.rectangle([34 * f, 86 * f, 82 * f, 92 * f], fill=(0xFF, 0xFF, 0xFF, 180))
    top = supersampled(size, draw)
    shadow.alpha_composite(top)
    shadow.save(OUT / "icon.png", optimize=True)


def text():
    """Black text on a transparent background (anti-aliased)."""
    font = ImageFont.load_default(size=88)
    small = ImageFont.load_default(size=44)
    img = Image.new("RGBA", (760, 300), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.text((20, 12), "Hello, SVG!", font=font, fill=(0x20, 0x20, 0x20, 255))
    d.text((20, 140), "png2svg converts pixels", font=small, fill=(0xC6, 0x28, 0x28, 255))
    d.text((20, 205), "to small vector shapes.", font=small, fill=(0xC6, 0x28, 0x28, 255))
    img.save(OUT / "text.png", optimize=True)


def screenshot():
    """A user interface: window, title bar, side bar, buttons and text lines."""
    k = 2  # the layout is made for 420x260 pixels and drawn twice as big
    img = Image.new("RGBA", (420 * k, 260 * k), (0xF5, 0xF5, 0xF5, 255))
    d = ImageDraw.Draw(img)

    def box(x0, y0, x1, y1):
        return [x0 * k, y0 * k, x1 * k, y1 * k]

    d.rectangle(box(0, 0, 419, 28), fill=(0x37, 0x47, 0x4F, 255))
    for i, c in enumerate([(0xFF, 0x5F, 0x56), (0xFF, 0xBD, 0x2E), (0x27, 0xC9, 0x3F)]):
        d.ellipse(box(10 + i * 18, 8, 22 + i * 18, 20), fill=c + (255,))
    d.rectangle(box(0, 29, 110, 259), fill=(0xEC, 0xEF, 0xF1, 255))
    for i in range(6):
        d.rectangle(
            box(14, 46 + i * 30, 96, 62 + i * 30),
            fill=(0xCF, 0xD8, 0xDC, 255) if i != 1 else (0x19, 0x76, 0xD2, 255),
        )
    font = ImageFont.load_default(size=14 * k)
    d.text((128 * k, 44 * k), "Settings", font=font, fill=(0x21, 0x21, 0x21, 255))
    for i in range(5):
        d.rectangle(box(128, 76 + i * 26, 128 + 220 - (i % 3) * 40, 84 + i * 26), fill=(0xB0, 0xBE, 0xC5, 255))
    d.rounded_rectangle(box(128, 214, 208, 242), radius=6 * k, fill=(0x19, 0x76, 0xD2, 255))
    d.text((146 * k, 220 * k), "Save", font=font, fill=(255, 255, 255, 255))
    d.rounded_rectangle(box(220, 214, 300, 242), radius=6 * k, outline=(0x19, 0x76, 0xD2, 255), width=2 * k)
    d.text((234 * k, 220 * k), "Cancel", font=font, fill=(0x19, 0x76, 0xD2, 255))
    img.save(OUT / "screenshot.png", optimize=True)


def pixelart():
    """A 20x20 sprite drawn with 8 colors and scaled by 4 without smoothing."""
    pal = {
        ".": (0, 0, 0, 0),
        "k": (0x1A, 0x1C, 0x2C, 255),
        "r": (0xB1, 0x3E, 0x53, 255),
        "o": (0xEF, 0x7D, 0x57, 255),
        "y": (0xFF, 0xCD, 0x75, 255),
        "g": (0x38, 0xB7, 0x64, 255),
        "b": (0x3B, 0x5D, 0xC9, 255),
        "w": (0xF4, 0xF4, 0xF4, 255),
    }
    rows = [
        "....................",
        ".......kkkkkk.......",
        ".....kkrrrrrrkk.....",
        "....krrrrooorrrk....",
        "...krrroooooorrrk...",
        "...krrooyyyyoorrk...",
        "..krrooyyyyyyoorrk..",
        "..krrooywwwwyoorrk..",
        "..krroyywkkwyyoorrk.",
        "..krroyywkkwyyoorrk.",
        "..krrooywwwwyoorrk..",
        "..krrooyyyyyyoorrk..",
        "...krrooyyyyoorrk...",
        "...krrroooooorrrk...",
        "....krrrrooorrrk....",
        ".....kkrrrrrrkk.....",
        ".......kkkkkk.......",
        "....gg.......bb.....",
        "...gggg.....bbbb....",
        "....gg.......bb.....",
    ]
    img = Image.new("RGBA", (20, 20))
    for y, row in enumerate(rows):
        for x, ch in enumerate(row):
            img.putpixel((x, y), pal[ch])
    img.resize((80, 80), Image.NEAREST).save(OUT / "pixelart.png", optimize=True)


def photo():
    """A photo-like image: smooth color fields, soft light and sensor noise."""
    rnd = random.Random(7)
    w, h = 240, 160
    base = Image.new("RGB", (w, h))
    d = ImageDraw.Draw(base)
    for y in range(h):  # sky and ground
        t = y / h
        d.line([(0, y), (w, y)], fill=(int(90 + 120 * t), int(140 + 80 * t), int(230 - 80 * t)))
    for _ in range(14):  # soft blobs of color
        x, y, r = rnd.randrange(w), rnd.randrange(h), rnd.randrange(18, 60)
        c = (rnd.randrange(30, 230), rnd.randrange(30, 230), rnd.randrange(30, 230))
        d.ellipse([x - r, y - r, x + r, y + r], fill=c)
    base = base.filter(ImageFilter.GaussianBlur(5))
    px = base.load()
    for y in range(h):
        for x in range(w):
            n = rnd.gauss(0, 2)
            r, g, b = px[x, y]
            px[x, y] = (max(0, min(255, int(r + n))), max(0, min(255, int(g + n))), max(0, min(255, int(b + n))))
    base.save(OUT / "photo.png", optimize=True)


def gradient():
    """A linear gradient with a radial highlight."""
    w, h = 256, 128
    img = Image.new("RGB", (w, h))
    px = img.load()
    for y in range(h):
        for x in range(w):
            t = x / (w - 1)
            r = int(30 + 200 * t)
            g = int(60 + 120 * (1 - t))
            b = int(200 - 140 * t)
            dist = math.hypot(x - 170, y - 50) / 70.0
            glow = max(0.0, 1.0 - dist) ** 2 * 90
            px[x, y] = (min(255, int(r + glow)), min(255, int(g + glow)), min(255, int(b + glow)))
    img.save(OUT / "gradient.png", optimize=True)


if __name__ == "__main__":
    for fn in (logo, icon, text, screenshot, pixelart, photo, gradient):
        fn()
    for p in sorted(OUT.glob("*.png")):
        print(p.name, p.stat().st_size, "bytes")
