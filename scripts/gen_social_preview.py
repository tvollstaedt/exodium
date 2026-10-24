#!/usr/bin/env python3
"""
Render the GitHub social preview (Settings -> General -> Social preview).

1280 x 640, under GitHub's 1 MB limit. The right half is a crop of the cover
wall from docs/screenshots/library.png - covers stay recognisable at the
300-500 px a link preview is shown at, and no collection counts appear that
would drift. GitHub has no API for the upload; it is done by hand.

Usage:
    python3 scripts/gen_social_preview.py [<out.png>]   (default: docs/social-preview.png)

Dependencies: Pillow, macOS SF Pro (/Library/Fonts)
"""

import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

REPO = Path(__file__).resolve().parent.parent
OUT = Path(sys.argv[1]) if len(sys.argv) > 1 else REPO / "docs/social-preview.png"
SHOT = REPO / "docs/screenshots/library.png"
SHOT_CROP = (0, 372, 1640, 931)  # the cover grid, below the collection strip
ICON = REPO / "src-tauri/icons/icon.png"
FONT = "/Library/Fonts/SF-Pro-Display-{}.otf"

W, H = 1280, 640
BG = (10, 10, 15)          # --bg-primary
GREEN = (74, 222, 128)     # the icon's phosphor
ACCENT = (124, 92, 252)    # --accent
TEXT = (240, 240, 245)
MUTED = (168, 168, 182)    # --text-secondary


def glow(center, radius, color, alpha):
    layer = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    x, y = center
    ImageDraw.Draw(layer).ellipse((x - radius, y - radius, x + radius, y + radius), fill=color + (alpha,))
    return layer.filter(ImageFilter.GaussianBlur(radius // 2))


def main():
    base = Image.new("RGBA", (W, H), BG + (255,))
    base = Image.alpha_composite(base, glow((250, 250), 260, GREEN, 46))
    base = Image.alpha_composite(base, glow((1050, 520), 320, ACCENT, 60))

    shot = Image.open(SHOT).convert("RGBA").crop(SHOT_CROP)
    sh = 540
    sw = int(shot.width * sh / shot.height)
    shot = shot.resize((sw, sh), Image.LANCZOS)
    mask = Image.new("L", (sw, sh), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, sw - 1, sh - 1), radius=18, fill=255)
    sx, sy = 640, 70
    shadow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle((sx + 6, sy + 16, sx + sw + 6, sy + sh + 16), radius=22, fill=(0, 0, 0, 170))
    base = Image.alpha_composite(base, shadow.filter(ImageFilter.GaussianBlur(18)))
    framed = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    framed.paste(shot, (sx, sy), mask)
    ImageDraw.Draw(framed).rounded_rectangle((sx, sy, sx + sw - 1, sy + sh - 1), radius=18, outline=(255, 255, 255, 34), width=2)
    base = Image.alpha_composite(base, framed)

    # Let the screenshot's left edge sink into the background.
    fade = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    fd = ImageDraw.Draw(fade)
    for i in range(90):
        fd.line([(sx + i, 0), (sx + i, H)], fill=BG + (int(150 * (1 - i / 90)),))
    base = Image.alpha_composite(base, fade)

    d = ImageDraw.Draw(base)
    base.alpha_composite(Image.open(ICON).convert("RGBA").resize((128, 128), Image.LANCZOS), (72, 132))
    d.text((72, 282), "Exodium", font=ImageFont.truetype(FONT.format("Bold"), 92), fill=TEXT)
    tag = ImageFont.truetype(FONT.format("Medium"), 31)
    d.text((76, 398), "Browse, stream and play", font=tag, fill=TEXT)
    d.text((76, 438), "the eXo game collections", font=tag, fill=TEXT)
    small = ImageFont.truetype(FONT.format("Regular"), 22)
    d.text((76, 504), "DOS  ·  Windows 3.x / 9x  ·  ScummVM  ·  Apple IIGS", font=small, fill=MUTED)
    d.text((76, 540), "Linux  ·  macOS  ·  Windows", font=small, fill=GREEN)

    base.convert("RGB").save(OUT, optimize=True)
    print(f"{OUT} ({OUT.stat().st_size // 1024} KB)")


if __name__ == "__main__":
    main()
