"""Render the provisional SMB shuriken mark to PNG and ICO.

The geometry matches `shuriken_outline` in src/theme.rs. Run with:

    python -I tools/make_icons.py

Requires Pillow. Output goes to assets/icon/.
"""

import math
import pathlib

from PIL import Image, ImageDraw

PINK = (249, 16, 121, 255)  # SMB Neon Pink
VOID = (5, 3, 5, 255)  # Void Black
OUT = pathlib.Path(__file__).resolve().parent.parent / "assets" / "icon"


def outline(steps: int = 12):
    """Unit-radius outline (x, y): four X-oriented blades, each with a straight
    leading edge and an inward-hooked trailing edge."""
    def polar(angle_deg, r):
        a = math.radians(angle_deg)
        return (math.cos(a) * r, math.sin(a) * r)

    points = []
    for k in range(4):
        base = 45 + 90 * k
        valley_prev = polar(base - 45, 0.24)
        tip = polar(base, 1.0)
        valley_next = polar(base + 45, 0.24)
        for i in range(steps):  # leading edge: straight
            t = i / steps
            points.append(lerp(valley_prev, tip, t))
        for i in range(steps):  # trailing edge: pulled toward the centre
            t = i / steps
            x, y = lerp(tip, valley_next, t)
            pull = 1 - 0.28 * math.sin(math.pi * t)
            points.append((x * pull, y * pull))
    return points


def lerp(a, b, t):
    return (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t)


def render(size: int, tile: bool) -> Image.Image:
    scale = 8
    big = size * scale
    img = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    c = big / 2
    if tile:
        radius = max(1, int(big * 0.06))  # brand: 0-6 px radius equivalent
        draw.rounded_rectangle((0, 0, big - 1, big - 1), radius=radius, fill=VOID)
        r = big * 0.40
    else:
        r = big * 0.48
    poly = [(c + x * r, c + y * r) for x, y in outline()]
    draw.polygon(poly, fill=PINK)
    hole = r * 0.12
    draw.ellipse(
        (c - hole, c - hole, c + hole, c + hole),
        fill=VOID if tile else (0, 0, 0, 0),
    )
    return img.resize((size, size), Image.LANCZOS)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    render(64, tile=False).save(OUT / "shuriken-64.png")
    render(256, tile=True).save(OUT / "app-256.png")
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    frames = [render(s, tile=s >= 24) for s in sizes]
    frames[-1].save(OUT / "smb-lol.ico", sizes=[(s, s) for s in sizes], append_images=frames[:-1])
    print("wrote", ", ".join(p.name for p in sorted(OUT.iterdir())))


if __name__ == "__main__":
    main()
