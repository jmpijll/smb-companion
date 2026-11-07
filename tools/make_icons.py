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


ROOT = pathlib.Path(__file__).resolve().parent.parent
FONTS = ROOT / "assets" / "fonts"
WHITE = (252, 250, 251, 255)  # Blade White
STEEL = (169, 157, 162, 255)  # Muted Steel


def mark(size: int) -> Image.Image:
    """Shuriken on transparent, supersampled."""
    big = size * 8
    img = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    c, r = big / 2, big * 0.48
    draw.polygon([(c + x * r, c + y * r) for x, y in outline()], fill=PINK)
    hole = r * 0.12
    draw.ellipse((c - hole, c - hole, c + hole, c + hole), fill=(0, 0, 0, 0))
    return img.resize((size, size), Image.LANCZOS)


def wizard_large(scale: int) -> Image.Image:
    """Installer side panel (164x314 at 100%): black, one slash, the mark and
    the division label. No wordmark: the custom SMB lettering is not a font."""
    from PIL import ImageFont

    w, h = 164 * scale, 314 * scale
    img = Image.new("RGBA", (w, h), VOID)
    draw = ImageDraw.Draw(img)
    # One controlled slash at the brand's 12-degree-ish diagonal.
    s = scale
    draw.polygon([(0, 248 * s), (w, 225 * s), (w, 228 * s), (0, 252 * s)], fill=PINK)
    m = mark(70 * s)
    img.alpha_composite(m, ((w - m.width) // 2, 40 * s))
    display = ImageFont.truetype(str(FONTS / "BarlowCondensed-BlackItalic.ttf"), 30 * s)
    label = ImageFont.truetype(str(FONTS / "Inter-SemiBold.ttf"), 8 * s)
    y = 126 * s
    for parts in ((("SMB ", WHITE), ("//", PINK)), (("LEAGUE OF", WHITE),), (("LEGENDS", WHITE),)):
        total = sum(draw.textlength(t, font=display) for t, _ in parts)
        x = (w - total) / 2
        for t, colour in parts:
            draw.text((x, y), t, font=display, fill=colour)
            x += draw.textlength(t, font=display)
        y += 29 * s

    def tracked(text, y):
        width = sum(draw.textlength(ch, font=label) + 1.8 * s for ch in text) - 1.8 * s
        x = (w - width) / 2
        for ch in text:
            draw.text((x, y), ch, font=label, fill=STEEL)
            x += draw.textlength(ch, font=label) + 1.8 * s

    tracked("SEXY MAAR", 262 * s)
    tracked("BESCHEIDEN", 274 * s)
    return img


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    render(64, tile=False).save(OUT / "shuriken-64.png")
    render(256, tile=True).save(OUT / "app-256.png")
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    frames = [render(s, tile=s >= 24) for s in sizes]
    frames[-1].save(OUT / "smb-lol.ico", sizes=[(s, s) for s in sizes], append_images=frames[:-1])
    print("wrote", ", ".join(p.name for p in sorted(OUT.iterdir())))

    pkg = ROOT / "packaging"
    pkg.mkdir(exist_ok=True)
    for scale in (1, 2):
        wizard_large(scale).save(pkg / f"wizard-large-{100 * scale}.png")
        mark(58 * scale).save(pkg / f"wizard-small-{100 * scale}.png")
    print("wrote packaging/wizard-*.png")


if __name__ == "__main__":
    main()
