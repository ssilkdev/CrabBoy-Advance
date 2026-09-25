"""Animated synthwave skin for CrabBoy Advance.

Backgrounds: sprite sheets of a striped retro sun over a perspective neon
grid that rolls towards the viewer (seamless loop), with twinkling stars.
Buttons: neon rings/pills that pulse (sprite sheets); pressed variants are
brighter, filled flares.
"""
import io, json, math, random, sys, zipfile
from PIL import Image, ImageDraw, ImageFilter, ImageFont, ImageChops

OUT = sys.argv[1]
PREVIEW = sys.argv[2] if len(sys.argv) > 2 else None

PINK = (255, 46, 200)
CYAN = (0, 240, 255)
PURPLE = (150, 60, 255)
ORANGE = (255, 140, 40)
YELLOW = (255, 225, 80)
NIGHT_TOP = (10, 2, 30)
NIGHT_MID = (45, 8, 75)
HORIZON = (140, 20, 110)


def font(size):
    for p in ["/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
              "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
              "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf"]:
        try:
            return ImageFont.truetype(p, size)
        except OSError:
            pass
    return ImageFont.load_default()


def lerp(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(len(a)))


def glow(layer, radius, strength=2):
    """Additive bloom of an RGBA layer."""
    blur = layer.filter(ImageFilter.GaussianBlur(radius))
    out = layer.copy()
    for _ in range(strength):
        out = Image.alpha_composite(blur, out)
    return out


# ---------------------------------------------------------------- background
def background_frame(w, h, phase, stars, horizon_at=0.52, sun_size=None, sun_xs=(0.5,)):
    """`horizon_at`: horizon height as a fraction of the frame. In portrait
    the game covers the top ~46% of the screen, so the scene sits lower."""
    horizon = int(h * horizon_at)
    im = Image.new("RGBA", (w, h))
    d = ImageDraw.Draw(im)
    # Sky gradient.
    for y in range(horizon):
        t = y / horizon
        # Smooth three-stop gradient (no visible seam between stops).
        a = t * t * (3 - 2 * t)
        c = lerp(lerp(NIGHT_TOP, NIGHT_MID, a), lerp(NIGHT_MID, HORIZON, a), a)
        d.line([(0, y), (w, y)], fill=c + (255,))
    # Ground.
    for y in range(horizon, h):
        t = (y - horizon) / (h - horizon)
        d.line([(0, y), (w, y)], fill=lerp((30, 0, 45), (8, 0, 18), t) + (255,))

    # Stars (twinkle with phase).
    for (sx, sy, sp, sz) in stars:
        a = 0.45 + 0.55 * (0.5 + 0.5 * math.sin(2 * math.pi * (phase * 2 + sp)))
        v = int(255 * a)
        x, y = sx * w, sy * horizon * 0.85
        d.ellipse([x - sz, y - sz, x + sz, y + sz], fill=(v, v, min(255, v + 30), 255))

    # Retro sun: gradient disc with horizontal cut stripes that drift down.
    r = int(min(w, h) * (sun_size or (0.30 if h > w else 0.36)))
    cy = horizon - int(r * 0.62)
    sun = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    sd = ImageDraw.Draw(sun)
    mask = Image.new("L", (w, h), 255)
    md = ImageDraw.Draw(mask)
    for sx in sun_xs:
        cx = int(w * sx)
        for y in range(cy - r, cy + r):
            t = (y - (cy - r)) / (2 * r)
            half = math.sqrt(max(0, r * r - (y - cy) ** 2))
            sd.line([(cx - half, y), (cx + half, y)], fill=lerp(YELLOW, PINK, min(1, t * 1.15)) + (255,))
        stripes(md, cx, cy, r, phase)
    sun.putalpha(ImageChops.multiply(sun.getchannel("A"), mask))
    halo = sun.filter(ImageFilter.GaussianBlur(r * 0.25))
    im = Image.alpha_composite(im, halo)
    im = Image.alpha_composite(im, sun)
    return finish(im, w, h, horizon, phase)


def stripes(md, cx, cy, r, phase):
    # Stripes from just above the middle down, thicker towards the bottom,
    # drifting down one band per loop.
    band = r * 0.15
    top = cy - r * 0.15
    for k in range(-1, 10):
        base = top + (k + phase) * band
        thick = (base - top) / (r * 1.15) * band * 0.7
        if thick > 0.8:
            md.rectangle([cx - r, base, cx + r, base + thick], fill=0)


def finish(im, w, h, horizon, phase):
    # Horizon line and ground cover over the sun's lower edge.
    ground = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    gd = ImageDraw.Draw(ground)
    for y in range(horizon, h):
        t = (y - horizon) / (h - horizon)
        gd.line([(0, y), (w, y)], fill=lerp((30, 0, 45), (8, 0, 18), t) + (255,))
    im = Image.alpha_composite(im, ground)

    # Perspective grid: vertical lines fan out from the vanishing point,
    # horizontal lines roll towards the viewer (phase 0..1 = one row).
    grid = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    g = ImageDraw.Draw(grid)
    vx, depth = w / 2, h - horizon
    lw = max(2, w // 300)
    for i in range(-14, 15):
        x_bottom = vx + i * w * 0.16
        g.line([(vx + i * w * 0.012, horizon), (x_bottom, h)], fill=PINK + (230,), width=lw)
    rows = 10
    for k in range(rows + 1):
        z = (k + phase) / rows  # 0 = horizon, 1 = bottom
        y = horizon + depth * (z ** 2.2)
        a = int(90 + 165 * z)
        g.line([(0, y), (w, y)], fill=PINK + (a,), width=lw)
    g.line([(0, horizon), (w, horizon)], fill=CYAN + (255,), width=lw + 1)
    im = Image.alpha_composite(im, glow(grid, max(3, w // 160), 2))
    return im.convert("RGB")


def background_sheet(w, h, frames, cols, seed, **scene):
    """`frames` frames in a grid `cols` wide (skin images max 2048x2048)."""
    random.seed(seed)
    stars = [(random.random(), random.random(), random.random(), random.choice([1, 1, 1.5, 2]) * w / 540)
             for _ in range(70)]
    rows = -(-frames // cols)
    assert w * cols <= 2048 and h * rows <= 2048, (w * cols, h * rows)
    sheet = Image.new("RGB", (w * cols, h * rows))
    for f in range(frames):
        sheet.paste(background_frame(w, h, f / frames, stars, **scene), ((f % cols) * w, (f // cols) * h))
    return sheet


# ------------------------------------------------------------------- buttons
def neon_ring(size, color, label, pulse, pressed, shape="circle", fontsize=None):
    """A neon outline (circle or pill) with glowing text."""
    W, H = size
    pad = int(min(W, H) * 0.12)
    layer = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    lw = max(3, int(min(W, H) * (0.05 + 0.015 * pulse)))
    box = [pad, pad, W - pad, H - pad]
    fill = color + (int(170 if pressed else 40 + 25 * pulse),)
    if shape == "circle":
        d.ellipse(box, fill=fill, outline=color + (255,), width=lw)
    else:
        d.rounded_rectangle(box, radius=(H - 2 * pad) // 2, fill=fill, outline=color + (255,), width=lw)
    if label:
        f = font(fontsize or int(min(W, H) * 0.42))
        tw = d.textlength(label, font=f)
        bb = d.textbbox((0, 0), label, font=f)
        th = bb[3] - bb[1]
        txt = (255, 255, 255, 255) if not pressed else (20, 0, 30, 255)
        d.text(((W - tw) / 2, (H - th) / 2 - bb[1]), label, font=f, fill=txt)
    strength = 3 if pressed else 1 + (pulse > 0.5)
    return fade_edges(glow(layer, max(4, int(min(W, H) * (0.07 + 0.05 * pulse))), strength))


def fade_edges(im, margin=0.1):
    """Fade alpha to 0 towards the image border so a glow never ends in a
    visible hard edge where the canvas stops."""
    w, h = im.size
    mw, mh = max(1, int(w * margin)), max(1, int(h * margin))
    mask = Image.new("L", (w, h), 0)
    ImageDraw.Draw(mask).rectangle([mw, mh, w - mw, h - mh], fill=255)
    mask = mask.filter(ImageFilter.GaussianBlur(min(mw, mh) * 0.6))
    out = im.copy()
    out.putalpha(ImageChops.multiply(im.getchannel("A"), mask))
    return out


def sheet_of(frames_list):
    w, h = frames_list[0].size
    sheet = Image.new("RGBA", (w, h * len(frames_list)), (0, 0, 0, 0))
    for i, f in enumerate(frames_list):
        sheet.paste(f, (0, i * h))
    return sheet


def pulsing(make, frames):
    return sheet_of([make(0.5 + 0.5 * math.sin(2 * math.pi * i / frames)) for i in range(frames)])


def dpad_image(size, pulse, pressed):
    S = size
    layer = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    a = S * 0.35
    lw = max(3, int(S * (0.025 + 0.008 * pulse)))
    col = CYAN
    fill = col + (int(150 if pressed else 30 + 20 * pulse),)
    pts = [(a, 14), (S - a, 14), (S - a, a), (S - 14, a), (S - 14, S - a), (S - a, S - a),
           (S - a, S - 14), (a, S - 14), (a, S - a), (14, S - a), (14, a), (a, a)]
    d.polygon(pts, fill=fill, outline=col + (255,), width=lw)
    c = S / 2
    for (dx, dy) in [(0, -1), (0, 1), (-1, 0), (1, 0)]:
        tip = (c + dx * S * 0.40, c + dy * S * 0.40)
        base = (c + dx * S * 0.27, c + dy * S * 0.27)
        side = (dy * S * 0.07, -dx * S * 0.07)
        d.polygon([tip, (base[0] + side[0], base[1] + side[1]), (base[0] - side[0], base[1] - side[1])],
                  fill=(255, 255, 255, 255))
    return fade_edges(glow(layer, max(4, int(S * (0.04 + 0.03 * pulse))), 2 if pressed else 1), 0.05)


def png(im):
    b = io.BytesIO()
    im.save(b, "PNG", optimize=True)
    return b.getvalue()


def main():
    F = 8  # frames per animation
    files = {}
    anims = {}
    images = {}

    def add(key, name, im, frames=1, fps=0.0, columns=1):
        files[name] = png(im)
        images[key] = name
        if frames > 1:
            anims[key] = {"frames": frames, "fps": fps}
            if columns > 1:
                anims[key]["columns"] = columns

    # Backgrounds: 8 frames of grid motion in a 4x2 grid at 8 fps (a grid
    # row rolls past every second). Portrait 4x480 x 2x1000 = 1920x2000,
    # landscape 2x1000 x 4x450 (2 columns): both under the 2048 cap.
    BG = 8
    add("background_portrait", "bg_portrait.png",
        background_sheet(480, 1000, BG, 4, 1, horizon_at=0.80, sun_size=0.17), BG, 8, 4)
    # Landscape: the game fills the middle and the side columns are full
    # of controls, so there's no clear spot for the sun; the grid, glowing
    # horizon and stars carry the look there.
    add("background_landscape", "bg_landscape.png",
        background_sheet(1000, 450, BG, 2, 2, horizon_at=0.55, sun_xs=()), BG, 8, 2)

    B = 160
    add("a", "a.png", pulsing(lambda p: neon_ring((B, B), PINK, "A", p, False), F), F, 6)
    add("a_pressed", "a_down.png", neon_ring((B, B), PINK, "A", 1.0, True))
    add("b", "b.png", pulsing(lambda p: neon_ring((B, B), CYAN, "B", p, False), F), F, 6)
    add("b_pressed", "b_down.png", neon_ring((B, B), CYAN, "B", 1.0, True))
    D = 240
    add("dpad", "dpad.png", pulsing(lambda p: dpad_image(D, p, False), F), F, 5)
    add("dpad_pressed", "dpad_down.png", dpad_image(D, 1.0, True))

    PW, PH = 200, 80
    for key, label, col in [("l", "L", PURPLE), ("r", "R", PURPLE),
                            ("start", "START", ORANGE), ("select", "SELECT", ORANGE),
                            ("quick_save", "SAVE", ORANGE), ("quick_load", "LOAD", ORANGE)]:
        fs = 44 if len(label) == 1 else 26
        add(key, f"{key}.png", pulsing(lambda p, l=label, c=col: neon_ring((PW, PH), c, l, p, False, "pill", fs), F), F, 5)
        add(f"{key}_pressed", f"{key}_down.png", neon_ring((PW, PH), col, label, 1.0, True, "pill", fs))
    M = 120
    add("menu", "menu.png", neon_ring((M, M), CYAN, "≡", 0.5, False, "pill", 60))
    add("fast", "fast.png", neon_ring((M, M), YELLOW, "»", 0.5, False, "pill", 64))
    add("fast_pressed", "fast_down.png", neon_ring((M, M), YELLOW, "»", 1.0, True, "pill", 64))

    manifest = {
        "name": "Synthwave",
        "author": "CrabBoy",
        "colors": {"fill": "#2A0A45A0", "pressed": "#FF2EC8D0", "edge": "#00F0FFE0",
                   "label": "#FFFFFFFF", "background": "#0A021E"},
        "images": images,
        "animations": anims,
        "layout": {},
    }
    with zipfile.ZipFile(OUT, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("Synthwave/skin.json", json.dumps(manifest, indent=2))
        for name, data in files.items():
            z.writestr(f"Synthwave/{name}", data)
    total = sum(len(v) for v in files.values())
    print(f"wrote {OUT}: {len(files)} images, {total/1024:.0f} KB")

    if PREVIEW:
        # A 4-frame strip of the portrait background with buttons composited.
        bg = Image.open(io.BytesIO(files["bg_portrait.png"])).convert("RGBA")
        fw, fh = bg.width // 4, bg.height // 2
        cell = lambda f: bg.crop(((f % 4) * fw, (f // 4) * fh, (f % 4 + 1) * fw, (f // 4 + 1) * fh)).resize((360, 800))
        frames = []
        for f in [0, 2, 4, 6]:
            fr = cell(f)
            a = Image.open(io.BytesIO(files["a.png"])).crop((0, (f % 8) * B, B, (f % 8 + 1) * B)).resize((80, 80))
            b = Image.open(io.BytesIO(files["b.png"])).crop((0, (f % 8) * B, B, (f % 8 + 1) * B)).resize((80, 80))
            dp = Image.open(io.BytesIO(files["dpad.png"])).crop((0, (f % 8) * D, D, (f % 8 + 1) * D)).resize((140, 140))
            fr.alpha_composite(dp, (20, 560))
            fr.alpha_composite(a, (260, 560))
            fr.alpha_composite(b, (190, 610))
            frames.append(fr)
        strip = Image.new("RGBA", (360 * 4 + 30, 800), (0, 0, 0, 255))
        for i, fr in enumerate(frames):
            strip.paste(fr, (i * 370, 0))
        strip.convert("RGB").save(PREVIEW)
        # And an animated GIF of the background loop.
        gif = [cell(f).convert("RGB").resize((270, 600)) for f in range(BG)]
        gif[0].save(PREVIEW.replace(".png", ".gif"), save_all=True, append_images=gif[1:], duration=100, loop=0)


main()
