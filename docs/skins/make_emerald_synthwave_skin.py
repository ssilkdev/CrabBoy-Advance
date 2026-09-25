"""Animated Emerald Synthwave skin for CrabBoy Advance.

Fuses the 80s retro-futuristic synthwave aesthetic (rolling perspective neon grid,
scanline sun, glowing wireframes, twinkling stars, and pulsing neon buttons) with
the iconic Pokémon Emerald motif:
- Electric neon emerald and cyber-jade grid lines.
- Radiant solar-gold / Rayquaza amber accents (the B button, directional arrows,
  and start/select).
- Dual-color sunset gradient on the retro sun transitioning from blazing solar gold
  into radioactive lime and electric emerald.
- Deep abyssal emerald midnight sky with twinkling cyan, lime, and gold stars.
- Celestial delta crest faintly glowing in the cosmic emerald atmosphere.
"""
import io, json, math, random, sys, zipfile
from PIL import Image, ImageDraw, ImageFilter, ImageFont, ImageChops

OUT = sys.argv[1]
PREVIEW = sys.argv[2] if len(sys.argv) > 2 else None

# ---- Palette -----------------------------------------------------------------
EMERALD_NEON = (0, 255, 168)       # #00FFA8 - Electric mint / neon emerald
EMERALD_DEEP = (0, 190, 110)       # Rich vibrant emerald
LIME_NEON = (140, 255, 60)         # Radioactive lime
CYAN_NEON = (0, 240, 255)          # Cyber cyan
GOLD_AMBER = (255, 195, 45)        # Solar Rayquaza gold / amber
GOLD_LIGHT = (255, 235, 120)       # Blazing light gold
WHITE = (255, 255, 255)

NIGHT_TOP = (4, 14, 10)            # Deep abyssal emerald black
NIGHT_MID = (10, 36, 26)           # Atmospheric dark jade
HORIZON_MIST = (18, 75, 55)        # Luminous emerald horizon glow
GROUND_TOP = (12, 32, 24)          # Dark jade obsidian ground
GROUND_BOT = (3, 10, 7)            # Deep abyss at floor bottom


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


# ---------------------------------------------------------------- background
def draw_delta_crest(d, cx, cy, size, alpha=90):
    """Subtle Rayquaza delta-crest geometric motif in the sky."""
    col = EMERALD_NEON + (alpha,)
    gold_col = GOLD_AMBER + (int(alpha * 0.9),)
    # Triangle / chevron
    s = size
    pts = [(cx, cy - s), (cx + int(s * 0.86), cy + int(s * 0.5)), (cx - int(s * 0.86), cy + int(s * 0.5))]
    d.polygon(pts, outline=col, width=max(1, s // 25))
    # Inner diamond
    isize = int(s * 0.45)
    ipts = [(cx, cy - isize), (cx + isize, cy), (cx, cy + isize), (cx - isize, cy)]
    d.polygon(ipts, outline=gold_col, width=max(1, s // 30))
    # Small center amber dot
    d.ellipse([cx - 3, cy - 3, cx + 3, cy + 3], fill=gold_col)


def background_frame(w, h, phase, stars, horizon_at=0.52, sun_size=None, sun_xs=(0.5,), with_crest=True):
    horizon = int(h * horizon_at)
    im = Image.new("RGBA", (w, h))
    d = ImageDraw.Draw(im)

    # Sky gradient: deep abyssal emerald-black to atmospheric jade to glowing mist.
    for y in range(horizon):
        t = y / horizon
        a = t * t * (3 - 2 * t)
        c = lerp(lerp(NIGHT_TOP, NIGHT_MID, a), lerp(NIGHT_MID, HORIZON_MIST, a), a)
        d.line([(0, y), (w, y)], fill=c + (255,))

    # Stars (twinkle with phase, varied emerald, cyan, and gold colors).
    for (sx, sy, sp, sz, scolor) in stars:
        a = 0.45 + 0.55 * (0.5 + 0.5 * math.sin(2 * math.pi * (phase * 2 + sp)))
        x, y = sx * w, sy * horizon * 0.85
        col = tuple(min(255, int(ch * (0.6 + 0.4 * a))) for ch in scolor) + (int(255 * a),)
        d.ellipse([x - sz, y - sz, x + sz, y + sz], fill=col)

    # Celestial Delta Crest in the upper atmosphere if enabled
    if with_crest and h > w:
        crest_layer = Image.new("RGBA", (w, h), (0, 0, 0, 0))
        cd = ImageDraw.Draw(crest_layer)
        # Sits in the upper visible area above the sun
        draw_delta_crest(cd, w // 2, int(horizon * 0.68), int(w * 0.18), alpha=75)
        im = Image.alpha_composite(im, glow(crest_layer, 4, 1))

    # Ground base gradient
    for y in range(horizon, h):
        t = (y - horizon) / (h - horizon)
        d.line([(0, y), (w, y)], fill=lerp(GROUND_TOP, GROUND_BOT, t) + (255,))

    # Retro Emerald Sun: multi-stop gradient (Gold -> Lime -> Neon Emerald) with scanline stripes
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
            # 3-stop sun gradient: Top Solar Gold -> Mid Lime -> Bottom Neon Emerald
            if t < 0.45:
                sc = lerp(GOLD_LIGHT, GOLD_AMBER, t / 0.45)
            elif t < 0.75:
                sc = lerp(GOLD_AMBER, LIME_NEON, (t - 0.45) / 0.30)
            else:
                sc = lerp(LIME_NEON, EMERALD_NEON, (t - 0.75) / 0.25)
            sd.line([(cx - half, y), (cx + half, y)], fill=sc + (255,))
        stripes(md, cx, cy, r, phase)

    sun.putalpha(ImageChops.multiply(sun.getchannel("A"), mask))
    halo = sun.filter(ImageFilter.GaussianBlur(r * 0.28))
    # Soft emerald corona flare around the sun
    corona = sun.filter(ImageFilter.GaussianBlur(r * 0.55))
    im = Image.alpha_composite(im, corona)
    im = Image.alpha_composite(im, halo)
    im = Image.alpha_composite(im, sun)

    return finish(im, w, h, horizon, phase)


def stripes(md, cx, cy, r, phase):
    band = r * 0.15
    top = cy - r * 0.15
    for k in range(-1, 10):
        base = top + (k + phase) * band
        thick = (base - top) / (r * 1.15) * band * 0.7
        if thick > 0.8:
            md.rectangle([cx - r, base, cx + r, base + thick], fill=0)


def finish(im, w, h, horizon, phase):
    # Cover lower sun edge with ground
    ground = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    gd = ImageDraw.Draw(ground)
    for y in range(horizon, h):
        t = (y - horizon) / (h - horizon)
        gd.line([(0, y), (w, y)], fill=lerp(GROUND_TOP, GROUND_BOT, t) + (255,))
    im = Image.alpha_composite(im, ground)

    # Perspective neon grid
    grid = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    g = ImageDraw.Draw(grid)
    vx, depth = w / 2, h - horizon
    lw = max(2, w // 300)

    # Vertical fanning perspective lines
    for i in range(-14, 15):
        x_bottom = vx + i * w * 0.16
        # Center guide line has a golden accent
        line_col = GOLD_AMBER + (200,) if i == 0 else EMERALD_NEON + (220,)
        g.line([(vx + i * w * 0.012, horizon), (x_bottom, h)], fill=line_col, width=lw)

    # Rolling horizontal lines (phase 0..1 = one row forwards)
    rows = 10
    for k in range(rows + 1):
        z = (k + phase) / rows
        y = horizon + depth * (z ** 2.2)
        a = int(90 + 165 * z)
        g.line([(0, y), (w, y)], fill=EMERALD_NEON + (a,), width=lw)

    # Glowing horizon beam (Electric Cyan & Mint)
    g.line([(0, horizon), (w, horizon)], fill=CYAN_NEON + (255,), width=lw + 1)
    g.line([(0, horizon - 1), (w, horizon - 1)], fill=EMERALD_NEON + (180,), width=1)

    im = Image.alpha_composite(im, glow(grid, max(3, w // 160), 2))
    return im.convert("RGB")


def background_sheet(w, h, frames, cols, seed, **scene):
    random.seed(seed)
    star_colors = [WHITE, EMERALD_NEON, CYAN_NEON, GOLD_AMBER, LIME_NEON]
    stars = [
        (
            random.random(),
            random.random(),
            random.random(),
            random.choice([1, 1, 1.4, 2.0]) * w / 540,
            random.choice(star_colors)
        )
        for _ in range(75)
    ]
    rows = -(-frames // cols)
    assert w * cols <= 2048 and h * rows <= 2048, (w * cols, h * rows)
    sheet = Image.new("RGB", (w * cols, h * rows))
    for f in range(frames):
        sheet.paste(background_frame(w, h, f / frames, stars, **scene), ((f % cols) * w, (f // cols) * h))
    return sheet


# ------------------------------------------------------------------- buttons
def neon_ring(size, color, label, pulse, pressed, shape="circle", fontsize=None, accent_color=None):
    """A neon outline with glowing text and concentric cyber details."""
    W, H = size
    pad = int(min(W, H) * 0.12)
    layer = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    lw = max(3, int(min(W, H) * (0.05 + 0.015 * pulse)))
    box = [pad, pad, W - pad, H - pad]
    fill_alpha = int(185 if pressed else 38 + 25 * pulse)
    fill = color + (fill_alpha,)

    if shape == "circle":
        d.ellipse(box, fill=fill, outline=color + (255,), width=lw)
        # Inner cyber ring for that high-tech feel
        inner_pad = pad + int(min(W, H) * 0.08)
        inner_box = [inner_pad, inner_pad, W - inner_pad, H - inner_pad]
        inner_col = (accent_color or color) + (int(70 + 60 * pulse),)
        d.ellipse(inner_box, outline=inner_col, width=max(1, lw // 2))
    else:
        d.rounded_rectangle(box, radius=(H - 2 * pad) // 2, fill=fill, outline=color + (255,), width=lw)

    if label:
        f = font(fontsize or int(min(W, H) * 0.42))
        tw = d.textlength(label, font=f)
        bb = d.textbbox((0, 0), label, font=f)
        th = bb[3] - bb[1]
        txt = (255, 255, 255, 255) if not pressed else (10, 30, 20, 255)
        d.text(((W - tw) / 2, (H - th) / 2 - bb[1]), label, font=f, fill=txt)

    strength = 3 if pressed else 1 + (pulse > 0.5)
    return fade_edges(glow(layer, max(4, int(min(W, H) * (0.07 + 0.05 * pulse))), strength))


def sheet_of(frames_list):
    w, h = frames_list[0].size
    sheet = Image.new("RGBA", (w, h * len(frames_list)), (0, 0, 0, 0))
    for i, f in enumerate(frames_list):
        sheet.paste(f, (0, i * h))
    return sheet


def pulsing(make, frames):
    return sheet_of([make(0.5 + 0.5 * math.sin(2 * math.pi * i / frames)) for i in range(frames)])


def dpad_image(size, pulse, pressed):
    """Cross D-pad with neon emerald outlines, gold arrow chevrons, and cyber center."""
    S = size
    layer = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    a = S * 0.35
    lw = max(3, int(S * (0.025 + 0.008 * pulse)))
    col = EMERALD_NEON
    fill = col + (int(160 if pressed else 30 + 20 * pulse),)

    pts = [
        (a, 14), (S - a, 14), (S - a, a), (S - 14, a), (S - 14, S - a), (S - a, S - a),
        (S - a, S - 14), (a, S - 14), (a, S - a), (14, S - a), (14, a), (a, a)
    ]
    d.polygon(pts, fill=fill, outline=col + (255,), width=lw)

    # Center cyber diamond hub
    c = S / 2
    chub = S * 0.10
    d.polygon([(c, c - chub), (c + chub, c), (c, c + chub), (c - chub, c)],
              fill=None, outline=CYAN_NEON + (int(180 + 70 * pulse),), width=2)

    # Gold arrow chevrons for directions
    arrow_col = GOLD_LIGHT if pressed else GOLD_AMBER
    for (dx, dy) in [(0, -1), (0, 1), (-1, 0), (1, 0)]:
        tip = (c + dx * S * 0.40, c + dy * S * 0.40)
        base = (c + dx * S * 0.27, c + dy * S * 0.27)
        side = (dy * S * 0.07, -dx * S * 0.07)
        d.polygon(
            [tip, (base[0] + side[0], base[1] + side[1]), (base[0] - side[0], base[1] - side[1])],
            fill=arrow_col + (255,)
        )

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

    BG = 8
    # Background portrait: 8 frames, 4 columns, 8 fps
    add("background_portrait", "bg_portrait.png",
        background_sheet(480, 1000, BG, 4, 1, horizon_at=0.80, sun_size=0.17, with_crest=True), BG, 8, 4)

    # Background landscape: 8 frames, 2 columns, 8 fps
    add("background_landscape", "bg_landscape.png",
        background_sheet(1000, 450, BG, 2, 2, horizon_at=0.55, sun_xs=(), with_crest=False), BG, 8, 2)

    # A Button: Neon Emerald with cyan inner accent
    B = 160
    add("a", "a.png", pulsing(lambda p: neon_ring((B, B), EMERALD_NEON, "A", p, False, accent_color=CYAN_NEON), F), F, 6)
    add("a_pressed", "a_down.png", neon_ring((B, B), EMERALD_NEON, "A", 1.0, True, accent_color=WHITE))

    # B Button: Solar Gold / Rayquaza Amber with lime accent
    add("b", "b.png", pulsing(lambda p: neon_ring((B, B), GOLD_AMBER, "B", p, False, accent_color=LIME_NEON), F), F, 6)
    add("b_pressed", "b_down.png", neon_ring((B, B), GOLD_AMBER, "B", 1.0, True, accent_color=WHITE))

    # D-Pad: Emerald cross with Golden Amber directional arrows and cyan hub
    D = 240
    add("dpad", "dpad.png", pulsing(lambda p: dpad_image(D, p, False), F), F, 5)
    add("dpad_pressed", "dpad_down.png", dpad_image(D, 1.0, True))

    # Shoulders L/R: Cyber Cyan with white glowing text
    PW, PH = 200, 80
    for key, label in [("l", "L"), ("r", "R")]:
        col = CYAN_NEON
        add(key, f"{key}.png", pulsing(lambda p, l=label, c=col: neon_ring((PW, PH), c, l, p, False, "pill", 44), F), F, 5)
        add(f"{key}_pressed", f"{key}_down.png", neon_ring((PW, PH), col, label, 1.0, True, "pill", 44))

    # Start & Select, and Save & Load below them: Solar Gold pills
    for key, label in [("start", "START"), ("select", "SELECT"), ("quick_save", "SAVE"), ("quick_load", "LOAD")]:
        col = GOLD_AMBER
        add(key, f"{key}.png", pulsing(lambda p, l=label, c=col: neon_ring((PW, PH), c, l, p, False, "pill", 26), F), F, 5)
        add(f"{key}_pressed", f"{key}_down.png", neon_ring((PW, PH), col, label, 1.0, True, "pill", 26))

    # Menu & Fast-forward: Cyber Cyan & Radioactive Lime pills
    M = 120
    add("menu", "menu.png", neon_ring((M, M), CYAN_NEON, "≡", 0.5, False, "pill", 60))
    add("fast", "fast.png", neon_ring((M, M), LIME_NEON, "»", 0.5, False, "pill", 64))
    add("fast_pressed", "fast_down.png", neon_ring((M, M), LIME_NEON, "»", 1.0, True, "pill", 64))

    manifest = {
        "name": "Emerald Synthwave",
        "author": "CrabBoy",
        "colors": {
            "fill": "#0A281EA0",
            "pressed": "#00FFA8D0",
            "edge": "#00FFA8E0",
            "label": "#FFFFFFFF",
            "background": "#030E0A"
        },
        "images": images,
        "animations": anims,
        "layout": {},
    }

    with zipfile.ZipFile(OUT, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("Emerald Synthwave/skin.json", json.dumps(manifest, indent=2))
        for name, data in files.items():
            z.writestr(f"Emerald Synthwave/{name}", data)

    total = sum(len(v) for v in files.values())
    print(f"wrote {OUT}: {len(files)} images, {total/1024:.0f} KB")

    if PREVIEW:
        # A 4-frame composite strip of portrait mode with buttons
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

        # Animated GIF of the background loop
        gif = [cell(f).convert("RGB").resize((270, 600)) for f in range(BG)]
        gif[0].save(PREVIEW.replace(".png", ".gif"), save_all=True, append_images=gif[1:], duration=100, loop=0)
        print(f"wrote preview to {PREVIEW} and {PREVIEW.replace('.png', '.gif')}")


if __name__ == "__main__":
    main()
