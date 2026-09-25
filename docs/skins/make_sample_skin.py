"""Make a sample skin pack: round gold A/B, pressed variants, a D-pad image
and portrait/landscape backgrounds."""
import json, zipfile, sys
from PIL import Image, ImageDraw, ImageFont, ImageFilter

out = sys.argv[1]
S = 256


def font(size):
    for p in ["/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
              "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf"]:
        try:
            return ImageFont.truetype(p, size)
        except OSError:
            pass
    return ImageFont.load_default()


def face(letter, pressed):
    im = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    top = (240, 200, 90) if not pressed else (190, 140, 40)
    bot = (180, 120, 30) if not pressed else (140, 90, 20)
    for i in range(S // 2 - 8, 0, -1):
        t = i / (S / 2)
        c = tuple(int(top[k] * (1 - t) + bot[k] * t) for k in range(3)) + (255,)
        d.ellipse([S / 2 - i, S / 2 - i, S / 2 + i, S / 2 + i], fill=c)
    d.ellipse([8, 8, S - 8, S - 8], outline=(255, 245, 210, 255), width=6)
    f = font(120)
    w = d.textlength(letter, font=f)
    d.text(((S - w) / 2, S / 2 - 72), letter, font=f, fill=(60, 35, 5, 255))
    return im


def dpad():
    im = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    a = S * 0.34
    d.rounded_rectangle([a, 12, S - a, S - 12], 22, fill=(40, 30, 20, 235), outline=(240, 200, 90, 255), width=5)
    d.rounded_rectangle([12, a, S - 12, S - a], 22, fill=(40, 30, 20, 235), outline=(240, 200, 90, 255), width=5)
    d.rectangle([a + 3, a + 3, S - a - 3, S - a - 3], fill=(40, 30, 20, 235))
    c = S / 2
    for (dx, dy) in [(0, -1), (0, 1), (-1, 0), (1, 0)]:
        tip = (c + dx * 100, c + dy * 100)
        base = (c + dx * 70, c + dy * 70)
        side = (dy * 18, -dx * 18)
        d.polygon([tip, (base[0] + side[0], base[1] + side[1]), (base[0] - side[0], base[1] - side[1])],
                  fill=(240, 200, 90, 255))
    return im


def bg(w, h):
    im = Image.new("RGB", (w, h))
    d = ImageDraw.Draw(im)
    for y in range(h):
        t = y / h
        d.line([(0, y), (w, y)], fill=(int(40 + 30 * t), int(26 + 14 * t), int(14 + 6 * t)))
    for x in range(0, w, 24):
        d.line([(x, 0), (x, h)], fill=(60, 44, 26))
    return im


def png(im):
    import io
    b = io.BytesIO()
    im.save(b, "PNG")
    return b.getvalue()


manifest = {
    "name": "Gold Rush",
    "author": "CrabBoy sample",
    "colors": {"fill": "#28200FC0", "pressed": "#F0C85AE0", "edge": "#F0C85AFF",
               "label": "#FFF0C8FF", "background": "#1E140A"},
    "images": {"a": "a.png", "a_pressed": "a_down.png", "b": "b.png", "b_pressed": "b_down.png",
               "dpad": "dpad.png", "background_portrait": "bg_portrait.png",
               "background_landscape": "bg_landscape.png"},
    "layout": {"portrait": {"a": {"scale": 1.15}, "b": {"scale": 1.15}}},
}
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("Gold Rush/skin.json", json.dumps(manifest, indent=2))
    z.writestr("Gold Rush/a.png", png(face("A", False)))
    z.writestr("Gold Rush/a_down.png", png(face("A", True)))
    z.writestr("Gold Rush/b.png", png(face("B", False)))
    z.writestr("Gold Rush/b_down.png", png(face("B", True)))
    z.writestr("Gold Rush/dpad.png", png(dpad()))
    z.writestr("Gold Rush/bg_portrait.png", png(bg(540, 1200)))
    z.writestr("Gold Rush/bg_landscape.png", png(bg(1200, 540)))
print("wrote", out)
