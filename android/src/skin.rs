//! Custom skins for the on-screen controls: colour themes, per-button
//! placement and size (saved separately for portrait and landscape), and
//! importable skin packs (`.zip` with a `skin.json` and optional PNGs).
//!
//! Plain logic with no Android dependencies, so it is host-tested.
//!
//! Skin pack format (all files at the top level of the zip, or inside one
//! folder):
//!
//! ```json
//! {
//!   "name": "Glacier",
//!   "author": "someone",
//!   "colors": {
//!     "fill": "#9CC8E080", "pressed": "#E0F0FFC0",
//!     "edge": "#FFFFFFA0", "label": "#1A2A3AFF", "background": "#0B1620"
//!   },
//!   "images": {
//!     "a": "a.png", "a_pressed": "a_down.png", "dpad": "dpad.png",
//!     "background_portrait": "bg_p.png", "background_landscape": "bg_l.png"
//!   },
//!   "layout": { "portrait": { "a": { "dx": 0.0, "dy": -0.02, "scale": 1.2 } } }
//! }
//! ```
//!
//! Every field is optional. Image keys are a control name (`dpad`, `a`, `b`,
//! `l`, `r`, `start`, `select`, `menu`, `fast`), optionally with `_pressed`,
//! or `background_portrait` / `background_landscape`.
//!
//! Animation (`"animations": {"<image key>": {...}}`): an image can be a
//! sprite sheet of `frames` equal frames in a grid `columns` wide (default
//! 1: stacked top to bottom), read left to right then top to bottom and
//! played at `fps`; and/or scroll (`scroll_x`, `scroll_y`, in image
//! widths/heights per second, wrapping around).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui::{Color32, Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};

use crate::touch::Layout;

// ---- Controls ---------------------------------------------------------------

/// One on-screen control that can be moved, resized and skinned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Control {
    Dpad,
    A,
    B,
    L,
    R,
    Start,
    Select,
    Menu,
    Fast,
}

impl Control {
    pub const ALL: [Control; 9] = [
        Control::Dpad,
        Control::A,
        Control::B,
        Control::L,
        Control::R,
        Control::Start,
        Control::Select,
        Control::Menu,
        Control::Fast,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Control::Dpad => "dpad",
            Control::A => "a",
            Control::B => "b",
            Control::L => "l",
            Control::R => "r",
            Control::Start => "start",
            Control::Select => "select",
            Control::Menu => "menu",
            Control::Fast => "fast",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Control::Dpad => "D-pad",
            Control::A => "A",
            Control::B => "B",
            Control::L => "L",
            Control::R => "R",
            Control::Start => "Start",
            Control::Select => "Select",
            Control::Menu => "Menu",
            Control::Fast => "Fast-forward",
        }
    }

    fn from_key(k: &str) -> Option<Control> {
        Control::ALL.into_iter().find(|c| c.key() == k)
    }
}

// ---- Layout overrides -------------------------------------------------------

/// Where one control sits relative to the default layout. Offsets are
/// fractions of the usable screen (so they survive a resolution change);
/// `scale` multiplies the control's size.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Placement {
    pub dx: f32,
    pub dy: f32,
    pub scale: f32,
}

impl Default for Placement {
    fn default() -> Self {
        Self { dx: 0.0, dy: 0.0, scale: 1.0 }
    }
}

pub const MIN_SCALE: f32 = 0.5;
pub const MAX_SCALE: f32 = 2.0;

/// Per-orientation placements. Missing controls use the default spot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutOverrides {
    pub portrait: BTreeMap<Control, Placement>,
    pub landscape: BTreeMap<Control, Placement>,
}

impl LayoutOverrides {
    pub fn for_orientation(&self, portrait: bool) -> &BTreeMap<Control, Placement> {
        if portrait { &self.portrait } else { &self.landscape }
    }

    pub fn for_orientation_mut(&mut self, portrait: bool) -> &mut BTreeMap<Control, Placement> {
        if portrait { &mut self.portrait } else { &mut self.landscape }
    }

    pub fn is_empty(&self) -> bool {
        self.portrait.is_empty() && self.landscape.is_empty()
    }
}

/// Centre and size of a control in `layout` (the D-pad and face buttons as
/// their bounding squares).
pub fn control_rect(l: &Layout, c: Control) -> Rect {
    let circle = |(p, r): (Pos2, f32)| Rect::from_center_size(p, Vec2::splat(r * 2.0));
    match c {
        Control::Dpad => Rect::from_center_size(l.dpad_center, Vec2::splat(l.dpad_radius * 2.0)),
        Control::A => circle(l.a),
        Control::B => circle(l.b),
        Control::L => l.l,
        Control::R => l.r,
        Control::Start => l.start,
        Control::Select => l.select,
        Control::Menu => l.menu,
        Control::Fast => l.fast,
    }
}

fn set_control_rect(l: &mut Layout, c: Control, r: Rect) {
    let rad = r.width().min(r.height()) / 2.0;
    match c {
        Control::Dpad => {
            l.dpad_center = r.center();
            l.dpad_radius = rad;
        }
        Control::A => l.a = (r.center(), rad),
        Control::B => l.b = (r.center(), rad),
        Control::L => l.l = r,
        Control::R => l.r = r,
        Control::Start => l.start = r,
        Control::Select => l.select = r,
        Control::Menu => l.menu = r,
        Control::Fast => l.fast = r,
    }
}

/// Apply the global size and the per-control placements to a computed
/// layout, keeping every control inside `safe`.
pub fn apply(l: &mut Layout, safe: Rect, overrides: &BTreeMap<Control, Placement>, global_scale: f32) {
    let global = global_scale.clamp(MIN_SCALE, MAX_SCALE);
    for c in Control::ALL {
        let p = overrides.get(&c).copied().unwrap_or_default();
        let base = control_rect(l, c);
        let scale = global * p.scale.clamp(MIN_SCALE, MAX_SCALE);
        let center = base.center() + Vec2::new(p.dx * safe.width(), p.dy * safe.height());
        // Never bigger than the screen allows, whatever the scales multiply to.
        let max_side = safe.width().min(safe.height()) * 0.9;
        let size = base.size() * scale;
        let fit = (max_side / size.x.max(size.y)).min(1.0);
        let mut r = Rect::from_center_size(center, size * fit);
        r = keep_inside(r, safe);
        set_control_rect(l, c, r);
    }
}

/// Shift `r` so it lies inside `area` (or centre it if it is bigger).
fn keep_inside(r: Rect, area: Rect) -> Rect {
    let mut d = Vec2::ZERO;
    if r.width() >= area.width() {
        d.x = area.center().x - r.center().x;
    } else if r.left() < area.left() {
        d.x = area.left() - r.left();
    } else if r.right() > area.right() {
        d.x = area.right() - r.right();
    }
    if r.height() >= area.height() {
        d.y = area.center().y - r.center().y;
    } else if r.top() < area.top() {
        d.y = area.top() - r.top();
    } else if r.bottom() > area.bottom() {
        d.y = area.bottom() - r.bottom();
    }
    r.translate(d)
}

/// The control whose area contains `p`, the smallest one if several do.
pub fn control_at(l: &Layout, p: Pos2) -> Option<Control> {
    Control::ALL
        .into_iter()
        .map(|c| (c, control_rect(l, c).expand(6.0)))
        .filter(|(_, r)| r.contains(p))
        .min_by(|a, b| a.1.area().total_cmp(&b.1.area()))
        .map(|(c, _)| c)
}

/// Layout-editor drag: move control `c` by `delta` points. `base` is the
/// layout *before* overrides (the default spot), so the stored offset is
/// relative to it, and the result is clamped like `apply` would.
pub fn drag(
    overrides: &mut BTreeMap<Control, Placement>,
    c: Control,
    delta: Vec2,
    safe: Rect,
) {
    if safe.width() <= 0.0 || safe.height() <= 0.0 {
        return;
    }
    let p = overrides.entry(c).or_default();
    p.dx = (p.dx + delta.x / safe.width()).clamp(-1.0, 1.0);
    p.dy = (p.dy + delta.y / safe.height()).clamp(-1.0, 1.0);
}

/// Layout-editor resize: multiply control `c`'s size by `factor`.
pub fn resize(overrides: &mut BTreeMap<Control, Placement>, c: Control, factor: f32) {
    let p = overrides.entry(c).or_default();
    p.scale = (p.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
}

// ---- Themes -----------------------------------------------------------------

/// Colours the controls are drawn with (RGBA, straight alpha).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub fill: [u8; 4],
    pub pressed: [u8; 4],
    pub edge: [u8; 4],
    pub label: [u8; 4],
    /// Colour behind the game.
    pub background: [u8; 3],
}

impl Theme {
    /// The original look.
    pub const CLASSIC: Theme = Theme {
        fill: [40, 40, 48, 150],
        pressed: [130, 90, 60, 200],
        edge: [160, 160, 170, 140],
        label: [230, 230, 235, 230],
        background: [12, 12, 16],
    };

    /// `fill` etc. as egui colours with `opacity` (0..1) applied.
    pub fn colors(&self, opacity: f32) -> ThemeColors {
        let o = opacity.clamp(0.0, 1.0);
        let c = |[r, g, b, a]: [u8; 4]| Color32::from_rgba_unmultiplied(r, g, b, (a as f32 * o).round() as u8);
        let [r, g, b] = self.background;
        ThemeColors {
            fill: c(self.fill),
            pressed: c(self.pressed),
            edge: c(self.edge),
            label: c(self.label),
            background: Color32::from_rgb(r, g, b),
        }
    }
}

/// Ready-to-draw colours.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThemeColors {
    pub fill: Color32,
    pub pressed: Color32,
    pub edge: Color32,
    pub label: Color32,
    pub background: Color32,
}

/// A built-in skin.
pub struct BuiltIn {
    pub id: &'static str,
    pub name: &'static str,
    pub theme: Theme,
}

pub const BUILT_IN: [BuiltIn; 6] = [
    BuiltIn { id: "classic", name: "Classic", theme: Theme::CLASSIC },
    BuiltIn {
        id: "indigo",
        name: "Indigo",
        theme: Theme {
            fill: [58, 48, 120, 170],
            pressed: [120, 104, 210, 220],
            edge: [150, 140, 220, 170],
            label: [235, 232, 255, 240],
            background: [18, 14, 34],
        },
    },
    BuiltIn {
        id: "glacier",
        name: "Glacier",
        theme: Theme {
            fill: [156, 200, 224, 110],
            pressed: [224, 240, 255, 190],
            edge: [255, 255, 255, 150],
            label: [20, 40, 58, 240],
            background: [11, 22, 32],
        },
    },
    BuiltIn {
        id: "famicom",
        name: "Famicom",
        theme: Theme {
            fill: [150, 30, 36, 190],
            pressed: [215, 170, 60, 230],
            edge: [232, 220, 196, 170],
            label: [246, 238, 220, 245],
            background: [26, 20, 18],
        },
    },
    BuiltIn {
        id: "outline",
        name: "Outline",
        theme: Theme {
            fill: [0, 0, 0, 0],
            pressed: [255, 255, 255, 70],
            edge: [255, 255, 255, 170],
            label: [255, 255, 255, 210],
            background: [0, 0, 0],
        },
    },
    BuiltIn {
        id: "oled",
        name: "OLED Black",
        theme: Theme {
            fill: [0, 0, 0, 220],
            pressed: [45, 45, 50, 240],
            edge: [60, 60, 66, 200],
            label: [120, 120, 128, 230],
            background: [0, 0, 0],
        },
    },
];

pub fn built_in(id: &str) -> Option<&'static BuiltIn> {
    BUILT_IN.iter().find(|b| b.id == id)
}

/// `#RRGGBB` or `#RRGGBBAA`.
pub fn parse_color(s: &str) -> Option<[u8; 4]> {
    let h = s.trim().strip_prefix('#')?;
    if !(h.len() == 6 || h.len() == 8) || !h.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?, if h.len() == 8 { byte(6)? } else { 255 }])
}

// ---- Skin packs -------------------------------------------------------------

/// `skin.json` of an imported skin pack.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SkinManifest {
    pub name: String,
    pub author: String,
    pub colors: BTreeMap<String, String>,
    pub images: BTreeMap<String, String>,
    /// Per image key: how it animates.
    pub animations: BTreeMap<String, Animation>,
    pub layout: LayoutOverrides,
}

/// How a skin image animates. The default is a still image.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Animation {
    /// Frames in the image (1 = not a sprite sheet).
    pub frames: u32,
    /// Frames per row of the sheet; rows = frames / columns (rounded up).
    pub columns: u32,
    /// Frames per second.
    pub fps: f32,
    /// Scroll speed in image widths / heights per second (wraps around).
    pub scroll_x: f32,
    pub scroll_y: f32,
}

impl Default for Animation {
    fn default() -> Self {
        Self { frames: 1, columns: 1, fps: 12.0, scroll_x: 0.0, scroll_y: 0.0 }
    }
}

pub const MAX_FRAMES: u32 = 64;

impl Animation {
    pub fn is_animated(&self) -> bool {
        self.frames > 1 || self.scroll_x != 0.0 || self.scroll_y != 0.0
    }

    /// Part of the image (in 0..1 texture coordinates) to show `t` seconds
    /// in. Scrolling goes past 1.0; the texture repeats.
    pub fn uv(&self, t: f64) -> Rect {
        let frames = self.frames.clamp(1, MAX_FRAMES);
        let (cols, rows) = self.grid();
        let frame = if frames > 1 && self.fps > 0.0 {
            (t * self.fps as f64) as u64 % frames as u64
        } else {
            0
        } as u32;
        let (w, h) = (1.0 / cols as f32, 1.0 / rows as f32);
        let (fx, fy) = ((frame % cols) as f32 * w, (frame / cols) as f32 * h);
        let wrap = |v: f64| v.rem_euclid(1.0) as f32;
        let (sx, sy) = (wrap(t * self.scroll_x as f64) * w, wrap(t * self.scroll_y as f64) * h);
        Rect::from_min_size(Pos2::new(fx + sx, fy + sy), Vec2::new(w, h))
    }

    /// (columns, rows) of the frame grid.
    pub fn grid(&self) -> (u32, u32) {
        let frames = self.frames.clamp(1, MAX_FRAMES);
        let cols = self.columns.clamp(1, frames);
        (cols, frames.div_ceil(cols))
    }

    fn validate(&self) -> Result<(), String> {
        if self.frames == 0 || self.frames > MAX_FRAMES {
            return Err(format!("frames must be 1 to {MAX_FRAMES}"));
        }
        if self.columns == 0 || self.columns > self.frames {
            return Err("columns must be 1 to the number of frames".into());
        }
        let finite = [self.fps, self.scroll_x, self.scroll_y].iter().all(|v| v.is_finite());
        if !finite || self.fps < 0.0 || self.fps > 60.0 || self.scroll_x.abs() > 10.0 || self.scroll_y.abs() > 10.0 {
            return Err("fps must be 0-60 and scroll speeds -10 to 10".into());
        }
        Ok(())
    }
}

impl SkinManifest {
    /// Theme from `colors`, falling back to Classic for anything missing.
    pub fn theme(&self) -> Theme {
        let mut t = Theme::CLASSIC;
        let get = |k: &str| self.colors.get(k).and_then(|v| parse_color(v));
        if let Some(c) = get("fill") { t.fill = c; }
        if let Some(c) = get("pressed") { t.pressed = c; }
        if let Some(c) = get("edge") { t.edge = c; }
        if let Some(c) = get("label") { t.label = c; }
        if let Some([r, g, b, _]) = get("background") { t.background = [r, g, b]; }
        t
    }
}

/// Image a skin can provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SkinImage {
    Control(Control, bool),
    Background { portrait: bool },
}

impl SkinImage {
    pub fn parse(key: &str) -> Option<SkinImage> {
        match key {
            "background_portrait" => return Some(SkinImage::Background { portrait: true }),
            "background_landscape" => return Some(SkinImage::Background { portrait: false }),
            _ => {}
        }
        let (name, pressed) = match key.strip_suffix("_pressed") {
            Some(n) => (n, true),
            None => (key, false),
        };
        Control::from_key(name).map(|c| SkinImage::Control(c, pressed))
    }
}

/// Largest skin pack accepted, compressed and uncompressed.
pub const MAX_PACK_BYTES: usize = 16 * 1024 * 1024;
/// Largest image side accepted.
pub const MAX_IMAGE_SIDE: u32 = 2048;
const MAX_FILES: usize = 64;

/// A skin pack that has been checked and is ready to install.
#[derive(Debug)]
pub struct SkinPack {
    pub manifest: SkinManifest,
    /// File name -> contents, only files the manifest refers to.
    pub files: BTreeMap<String, Vec<u8>>,
}

/// Read and validate a `.zip` skin pack.
pub fn read_pack(zip: &[u8]) -> Result<SkinPack, String> {
    if zip.len() > MAX_PACK_BYTES {
        return Err("The skin file is larger than 16 MB".into());
    }
    let entries = zip::read(zip)?;
    // Allow everything inside one top-level folder.
    let prefix = common_folder(entries.keys());
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (name, data) in entries {
        let rel = name.strip_prefix(&prefix).unwrap_or(&name).to_string();
        if rel.is_empty() || rel.ends_with('/') || rel.starts_with("__MACOSX/") {
            continue;
        }
        files.insert(rel, data);
    }
    let json = files.get("skin.json").ok_or("No skin.json in the skin file")?;
    let manifest: SkinManifest =
        serde_json::from_slice(json).map_err(|e| format!("skin.json is not valid: {e}"))?;
    let mut out = BTreeMap::new();
    for (key, file) in &manifest.images {
        if SkinImage::parse(key).is_none() {
            return Err(format!("Unknown image \"{key}\" in skin.json"));
        }
        if !is_plain_file_name(file) || !file.to_ascii_lowercase().ends_with(".png") {
            return Err(format!("Image \"{file}\" must be a .png at the top of the skin"));
        }
        let data = files.get(file).ok_or_else(|| format!("skin.json lists \"{file}\", but it is missing"))?;
        let (w, h) = check_png(data).map_err(|e| format!("{file}: {e}"))?;
        if let Some(a) = manifest.animations.get(key) {
            a.validate().map_err(|e| format!("Animation \"{key}\": {e}"))?;
            let (cols, rows) = a.grid();
            if w % cols != 0 || h % rows != 0 {
                return Err(format!("{file}: {w}x{h} doesn't split into {cols}x{rows} equal frames"));
            }
        }
        out.insert(file.clone(), data.clone());
    }
    for key in manifest.animations.keys() {
        if !manifest.images.contains_key(key) {
            return Err(format!("Animation \"{key}\" has no image"));
        }
    }
    for (k, v) in &manifest.colors {
        if parse_color(v).is_none() {
            return Err(format!("Colour \"{k}\" must look like #RRGGBB or #RRGGBBAA"));
        }
    }
    out.insert("skin.json".into(), json.clone());
    Ok(SkinPack { manifest, files: out })
}

/// Check a PNG decodes and isn't huge. Returns its size.
pub fn check_png(data: &[u8]) -> Result<(u32, u32), String> {
    let img = image::load_from_memory_with_format(data, image::ImageFormat::Png)
        .map_err(|e| format!("not a readable PNG ({e})"))?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 || w > MAX_IMAGE_SIDE || h > MAX_IMAGE_SIDE {
        return Err(format!("image is {w}x{h}; the limit is {MAX_IMAGE_SIDE}x{MAX_IMAGE_SIDE}"));
    }
    Ok((w, h))
}

fn is_plain_file_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && !s.contains(['/', '\\', ':'])
        && s != "."
        && s != ".."
        && !s.starts_with('.')
}

fn common_folder<'a>(names: impl Iterator<Item = &'a String>) -> String {
    let mut folder: Option<String> = None;
    for n in names {
        if n.starts_with("__MACOSX/") {
            continue;
        }
        let top = match n.split_once('/') {
            Some((t, _)) => format!("{t}/"),
            None => return String::new(),
        };
        match &folder {
            None => folder = Some(top),
            Some(f) if *f == top => {}
            Some(_) => return String::new(),
        }
    }
    folder.unwrap_or_default()
}

/// Folder name for a skin: lowercase letters, digits and dashes.
pub fn skin_id(name: &str) -> String {
    let mut id: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    while id.contains("--") {
        id = id.replace("--", "-");
    }
    let id = id.trim_matches('-').chars().take(40).collect::<String>();
    if id.is_empty() { "skin".into() } else { id }
}

/// Install a checked pack under `skins_dir/<id>/`, replacing an older
/// version of the same skin. Returns the id.
pub fn install(pack: &SkinPack, skins_dir: &Path, fallback_name: &str) -> std::io::Result<String> {
    let name = if pack.manifest.name.trim().is_empty() { fallback_name } else { &pack.manifest.name };
    let id = format!("custom-{}", skin_id(name));
    let dir = skins_dir.join(&id);
    let tmp = skins_dir.join(format!(".{id}.part"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    for (file, data) in &pack.files {
        std::fs::write(tmp.join(file), data)?;
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&tmp, &dir)?;
    Ok(id)
}

/// An installed custom skin.
#[derive(Debug, Clone)]
pub struct InstalledSkin {
    pub id: String,
    pub dir: PathBuf,
    pub manifest: SkinManifest,
}

impl InstalledSkin {
    pub fn display_name(&self) -> &str {
        if self.manifest.name.trim().is_empty() { &self.id } else { &self.manifest.name }
    }

    /// Animation for each animated image.
    pub fn animations(&self) -> BTreeMap<SkinImage, Animation> {
        self.manifest
            .animations
            .iter()
            .filter_map(|(k, a)| Some((SkinImage::parse(k)?, *a)))
            .filter(|(_, a)| a.is_animated() && a.validate().is_ok())
            .collect()
    }

    /// Image files by what they're for.
    pub fn images(&self) -> BTreeMap<SkinImage, PathBuf> {
        self.manifest
            .images
            .iter()
            .filter_map(|(k, f)| Some((SkinImage::parse(k)?, self.dir.join(f))))
            .collect()
    }
}

/// Custom skins installed in `skins_dir`, sorted by name.
pub fn list_installed(skins_dir: &Path) -> Vec<InstalledSkin> {
    let mut v: Vec<InstalledSkin> = std::fs::read_dir(skins_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let id = e.file_name().to_string_lossy().to_string();
                    if !id.starts_with("custom-") {
                        return None;
                    }
                    let dir = e.path();
                    let json = std::fs::read(dir.join("skin.json")).ok()?;
                    let manifest = serde_json::from_slice(&json).ok()?;
                    Some(InstalledSkin { id, dir, manifest })
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| a.display_name().to_lowercase().cmp(&b.display_name().to_lowercase()));
    v
}

pub fn uninstall(skins_dir: &Path, id: &str) -> std::io::Result<()> {
    if !id.starts_with("custom-") || !is_plain_file_name(id) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a custom skin"));
    }
    std::fs::remove_dir_all(skins_dir.join(id))
}

// ---- Settings ---------------------------------------------------------------

/// When the touch controls are shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Hidden while a controller is being used (the menu button stays).
    #[default]
    Auto,
    Always,
    /// Only the menu button (for controller or TV play).
    MenuOnly,
}

impl Visibility {
    pub fn label(self) -> &'static str {
        match self {
            Visibility::Auto => "Touch controls: Auto",
            Visibility::Always => "Touch controls: Always",
            Visibility::MenuOnly => "Touch controls: Hidden",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Visibility::Auto => Visibility::Always,
            Visibility::Always => Visibility::MenuOnly,
            Visibility::MenuOnly => Visibility::Auto,
        }
    }
}

/// Saved in `skin.json` in the app's files directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkinSettings {
    /// A `BUILT_IN` id or an installed `custom-...` id.
    pub skin: String,
    /// 0.2..=1.0.
    pub opacity: f32,
    /// Size of all controls, 0.5..=2.0.
    pub scale: f32,
    pub visibility: Visibility,
    /// The user's own arrangement (from the layout editor). A custom skin's
    /// own layout applies underneath it.
    pub layout: LayoutOverrides,
}

impl Default for SkinSettings {
    fn default() -> Self {
        Self {
            skin: "classic".into(),
            opacity: 1.0,
            scale: 1.0,
            visibility: Visibility::Auto,
            layout: LayoutOverrides::default(),
        }
    }
}

pub const MIN_OPACITY: f32 = 0.2;

impl SkinSettings {
    /// Parse `skin.json`; anything broken falls back to the defaults.
    pub fn parse(s: &str) -> Self {
        let mut v: Self = serde_json::from_str(s).unwrap_or_default();
        v.sanitize();
        v
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn sanitize(&mut self) {
        if !self.opacity.is_finite() {
            self.opacity = 1.0;
        }
        if !self.scale.is_finite() {
            self.scale = 1.0;
        }
        self.opacity = self.opacity.clamp(MIN_OPACITY, 1.0);
        self.scale = self.scale.clamp(MIN_SCALE, MAX_SCALE);
        for map in [&mut self.layout.portrait, &mut self.layout.landscape] {
            for p in map.values_mut() {
                if !(p.dx.is_finite() && p.dy.is_finite() && p.scale.is_finite()) {
                    *p = Placement::default();
                }
                p.dx = p.dx.clamp(-1.0, 1.0);
                p.dy = p.dy.clamp(-1.0, 1.0);
                p.scale = p.scale.clamp(MIN_SCALE, MAX_SCALE);
            }
        }
        if self.skin.is_empty() {
            self.skin = "classic".into();
        }
    }
}

/// The skin's own placements with the user's on top (the user's offsets add
/// to the skin's; sizes multiply).
pub fn combined(skin: &BTreeMap<Control, Placement>, user: &BTreeMap<Control, Placement>) -> BTreeMap<Control, Placement> {
    let mut out = skin.clone();
    for (c, u) in user {
        let e = out.entry(*c).or_default();
        e.dx += u.dx;
        e.dy += u.dy;
        e.scale *= u.scale;
    }
    out
}

// ---- Minimal zip reader -----------------------------------------------------

mod zip {
    use std::collections::BTreeMap;

    fn u16le(b: &[u8], at: usize) -> Option<usize> {
        Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?) as usize)
    }
    fn u32le(b: &[u8], at: usize) -> Option<usize> {
        Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize)
    }

    /// File name -> contents for every file in the archive (stored or
    /// deflated). Rejects encrypted, zip64, oversized and path-escaping
    /// entries and checks each file's CRC.
    pub fn read(z: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, String> {
        const BAD: &str = "The skin file is not a valid .zip";
        // End of central directory: last 22+ bytes, signature PK\x05\x06.
        let min = z.len().saturating_sub(22 + 65535);
        let eocd = (min..z.len().saturating_sub(21))
            .rev()
            .find(|&i| z[i..i + 4] == [0x50, 0x4B, 0x05, 0x06])
            .ok_or(BAD)?;
        let count = u16le(z, eocd + 10).ok_or(BAD)?;
        let mut at = u32le(z, eocd + 16).ok_or(BAD)?;
        if count > super::MAX_FILES {
            return Err("The skin file has too many files".into());
        }
        let mut out = BTreeMap::new();
        let mut total = 0usize;
        for _ in 0..count {
            if z.get(at..at + 4) != Some(&[0x50, 0x4B, 0x01, 0x02]) {
                return Err(BAD.into());
            }
            let flags = u16le(z, at + 8).ok_or(BAD)?;
            let method = u16le(z, at + 10).ok_or(BAD)?;
            let crc = u32le(z, at + 16).ok_or(BAD)? as u32;
            let csize = u32le(z, at + 20).ok_or(BAD)?;
            let usize_ = u32le(z, at + 24).ok_or(BAD)?;
            let nlen = u16le(z, at + 28).ok_or(BAD)?;
            let elen = u16le(z, at + 30).ok_or(BAD)?;
            let clen = u16le(z, at + 32).ok_or(BAD)?;
            let local = u32le(z, at + 42).ok_or(BAD)?;
            let name = z.get(at + 46..at + 46 + nlen).ok_or(BAD)?;
            let name = String::from_utf8(name.to_vec()).map_err(|_| BAD)?.replace('\\', "/");
            at += 46 + nlen + elen + clen;

            if flags & 1 != 0 {
                return Err("Encrypted skin files aren't supported".into());
            }
            if name.starts_with('/') || name.split('/').any(|p| p == "..") {
                return Err(BAD.into());
            }
            if name.ends_with('/') {
                continue;
            }
            total += usize_;
            if usize_ > super::MAX_PACK_BYTES || total > super::MAX_PACK_BYTES {
                return Err("The skin is larger than 16 MB unpacked".into());
            }
            if z.get(local..local + 4) != Some(&[0x50, 0x4B, 0x03, 0x04]) {
                return Err(BAD.into());
            }
            let lnlen = u16le(z, local + 26).ok_or(BAD)?;
            let lelen = u16le(z, local + 28).ok_or(BAD)?;
            let start = local + 30 + lnlen + lelen;
            let raw = z.get(start..start + csize).ok_or(BAD)?;
            let data = match method {
                0 => raw.to_vec(),
                8 => miniz_oxide::inflate::decompress_to_vec_with_limit(raw, usize_).map_err(|_| BAD)?,
                _ => return Err(format!("\"{name}\" uses an unsupported compression method")),
            };
            if data.len() != usize_ || crc32(&data) != crc {
                return Err(format!("\"{name}\" is damaged"));
            }
            out.insert(name, data);
        }
        Ok(out)
    }

    pub fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { 0xEDB8_8320 ^ (crc >> 1) } else { crc >> 1 };
            }
        }
        !crc
    }
}

#[cfg(test)]
pub(crate) mod test_zip {
    //! Builds zip files for tests (stored or deflated entries).
    pub fn build(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data, deflate) in files {
            let crc = super::zip::crc32(data);
            let body = if *deflate { miniz_oxide::deflate::compress_to_vec(data, 6) } else { data.to_vec() };
            let offset = out.len() as u32;
            let method: u16 = if *deflate { 8 } else { 0 };
            let mut h = Vec::new();
            h.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04, 20, 0, 0, 0]);
            h.extend_from_slice(&method.to_le_bytes());
            h.extend_from_slice(&[0, 0, 0, 0]);
            h.extend_from_slice(&crc.to_le_bytes());
            h.extend_from_slice(&(body.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            h.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&h);
            out.extend_from_slice(&body);
            let mut c = Vec::new();
            c.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02, 20, 0, 20, 0, 0, 0]);
            c.extend_from_slice(&method.to_le_bytes());
            c.extend_from_slice(&[0, 0, 0, 0]);
            c.extend_from_slice(&crc.to_le_bytes());
            c.extend_from_slice(&(body.len() as u32).to_le_bytes());
            c.extend_from_slice(&(data.len() as u32).to_le_bytes());
            c.extend_from_slice(&(name.len() as u16).to_le_bytes());
            c.extend_from_slice(&[0; 12]);
            c.extend_from_slice(&offset.to_le_bytes());
            c.extend_from_slice(name.as_bytes());
            central.extend_from_slice(&c);
        }
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&[0x50, 0x4B, 0x05, 0x06, 0, 0, 0, 0]);
        let n = (files.len() as u16).to_le_bytes();
        out.extend_from_slice(&n);
        out.extend_from_slice(&n);
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    /// A small valid PNG.
    pub fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 60, 60, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::touch::{Buttons, Hand};

    fn safe() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(412.0, 915.0))
    }

    fn layout() -> Layout {
        Layout::compute_for(safe(), Some([240, 160]), Hand::Both)
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("crabboy-skin-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn moved_control_is_hit_at_its_new_place() {
        let mut l = layout();
        let before = l.a.0;
        let mut o = BTreeMap::new();
        o.insert(Control::A, Placement { dx: -0.1, dy: -0.05, scale: 1.5 });
        apply(&mut l, safe(), &o, 1.0);
        let moved = before + Vec2::new(-0.1 * 412.0, -0.05 * 915.0);
        assert!((l.a.0 - moved).length() < 0.5);
        assert_eq!(l.hit(l.a.0) & Buttons::A, Buttons::A);
        assert!(l.a.1 > 1.4 * layout().a.1, "A grew");
    }

    #[test]
    fn controls_never_leave_the_screen() {
        let mut l = layout();
        let mut o = BTreeMap::new();
        for c in Control::ALL {
            o.insert(c, Placement { dx: 1.0, dy: 1.0, scale: 2.0 });
        }
        apply(&mut l, safe(), &o, 2.0);
        for c in Control::ALL {
            let r = control_rect(&l, c);
            assert!(safe().expand(0.5).contains_rect(r), "{c:?} {r:?}");
        }
    }

    #[test]
    fn default_placement_changes_nothing() {
        let mut l = layout();
        apply(&mut l, safe(), &BTreeMap::new(), 1.0);
        let d = layout();
        for c in Control::ALL {
            let (a, b) = (control_rect(&l, c), control_rect(&d, c));
            assert!((a.center() - b.center()).length() < 0.01 && (a.size() - b.size()).length() < 0.01, "{c:?}");
        }
    }

    #[test]
    fn editor_drag_and_resize() {
        let l = layout();
        let a = control_at(&l, l.a.0);
        assert_eq!(a, Some(Control::A));
        assert_eq!(control_at(&l, l.dpad_center + Vec2::new(0.0, -l.dpad_radius * 0.7)), Some(Control::Dpad));
        assert_eq!(control_at(&l, l.screen.center()), None);

        let mut o = BTreeMap::new();
        drag(&mut o, Control::A, Vec2::new(-41.2, 91.5), safe());
        let p = o[&Control::A];
        assert!((p.dx + 0.1).abs() < 1e-5 && (p.dy - 0.1).abs() < 1e-5);
        for _ in 0..20 {
            resize(&mut o, Control::A, 1.25);
        }
        assert_eq!(o[&Control::A].scale, MAX_SCALE);
        let mut moved = layout();
        apply(&mut moved, safe(), &o, 1.0);
        assert_eq!(control_at(&moved, moved.a.0), Some(Control::A));
    }

    #[test]
    fn skin_layout_and_user_layout_combine() {
        let mut skin = BTreeMap::new();
        skin.insert(Control::B, Placement { dx: 0.1, dy: 0.0, scale: 1.5 });
        let mut user = BTreeMap::new();
        user.insert(Control::B, Placement { dx: 0.05, dy: 0.02, scale: 0.5 });
        user.insert(Control::L, Placement { dx: 0.0, dy: 0.1, scale: 1.0 });
        let c = combined(&skin, &user);
        let b = c[&Control::B];
        assert!((b.dx - 0.15).abs() < 1e-6 && (b.dy - 0.02).abs() < 1e-6 && (b.scale - 0.75).abs() < 1e-6);
        assert_eq!(c[&Control::L].dy, 0.1);
    }

    #[test]
    fn colors_and_themes() {
        assert_eq!(parse_color("#FF8000"), Some([255, 128, 0, 255]));
        assert_eq!(parse_color("#11223344"), Some([0x11, 0x22, 0x33, 0x44]));
        assert_eq!(parse_color("FF8000"), None);
        assert_eq!(parse_color("#GG0000"), None);
        assert_eq!(parse_color("#ÿÿÿ"), None);
        let half = Theme::CLASSIC.colors(0.5);
        assert_eq!(half.fill.a(), 75);
        assert!(BUILT_IN.iter().all(|b| built_in(b.id).is_some()));
        let m = SkinManifest {
            colors: [("fill".to_string(), "#10203040".to_string())].into(),
            ..Default::default()
        };
        assert_eq!(m.theme().fill, [0x10, 0x20, 0x30, 0x40]);
        assert_eq!(m.theme().edge, Theme::CLASSIC.edge, "missing colours keep Classic");
    }

    #[test]
    fn settings_round_trip_and_repair() {
        let mut s = SkinSettings { skin: "glacier".into(), opacity: 0.6, scale: 1.3, ..Default::default() };
        s.visibility = Visibility::MenuOnly;
        s.layout.landscape.insert(Control::Start, Placement { dx: 0.2, dy: -0.1, scale: 0.8 });
        assert_eq!(SkinSettings::parse(&s.to_json()), s);
        assert_eq!(SkinSettings::parse("not json"), SkinSettings::default());
        let bad = SkinSettings::parse(r#"{"opacity": 0.0, "scale": 9, "layout": {"portrait": {"a": {"dx": 5}}}}"#);
        assert_eq!((bad.opacity, bad.scale), (MIN_OPACITY, MAX_SCALE));
        assert_eq!(bad.layout.portrait[&Control::A].dx, 1.0);
        // A settings file from before this feature (missing keys) loads.
        assert_eq!(SkinSettings::parse("{}"), SkinSettings::default());
    }

    #[test]
    fn skin_pack_install_list_and_remove() {
        let manifest = br##"{"name": "Gold Rush", "author": "t",
            "colors": {"fill": "#C8A03CB0", "background": "#101010"},
            "images": {"a": "a.png", "a_pressed": "a_down.png", "background_portrait": "bg.png"},
            "layout": {"portrait": {"a": {"dx": -0.05, "scale": 1.2}}}}"##;
        let (a, ad, bg) = (test_zip::png(64, 64), test_zip::png(64, 64), test_zip::png(90, 200));
        // Inside one folder, mixed compression, with Mac junk alongside.
        let zip = test_zip::build(&[
            ("Gold Rush/skin.json", manifest, true),
            ("Gold Rush/a.png", &a, false),
            ("Gold Rush/a_down.png", &ad, true),
            ("Gold Rush/bg.png", &bg, true),
            ("__MACOSX/Gold Rush/._a.png", b"junk", false),
        ]);
        let pack = read_pack(&zip).unwrap();
        assert_eq!(pack.manifest.name, "Gold Rush");
        assert_eq!(pack.manifest.theme().fill, [0xC8, 0xA0, 0x3C, 0xB0]);
        assert_eq!(pack.files.len(), 4);

        let dir = tmpdir("install");
        let id = install(&pack, &dir, "x").unwrap();
        assert_eq!(id, "custom-gold-rush");
        let list = list_installed(&dir);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].display_name(), "Gold Rush");
        let imgs = list[0].images();
        assert_eq!(imgs.len(), 3);
        assert!(imgs[&SkinImage::Control(Control::A, true)].exists());
        assert!(imgs[&SkinImage::Background { portrait: true }].exists());
        assert_eq!(list[0].manifest.layout.portrait[&Control::A].scale, 1.2);
        // Installing again replaces it.
        install(&pack, &dir, "x").unwrap();
        assert_eq!(list_installed(&dir).len(), 1);
        uninstall(&dir, &id).unwrap();
        assert!(list_installed(&dir).is_empty());
        assert!(uninstall(&dir, "../etc").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bad_skin_packs_are_refused_with_a_reason() {
        let png = test_zip::png(8, 8);
        let big = test_zip::png(MAX_IMAGE_SIDE + 1, 4);
        let cases: Vec<(Vec<u8>, &str)> = vec![
            (b"not a zip".to_vec(), "not a valid .zip"),
            (test_zip::build(&[("a.png", &png, false)]), "No skin.json"),
            (test_zip::build(&[("skin.json", b"{nope", false)]), "not valid"),
            (test_zip::build(&[("skin.json", br#"{"images": {"a": "missing.png"}}"#, false)]), "missing"),
            (test_zip::build(&[("skin.json", br#"{"images": {"z": "a.png"}}"#, false), ("a.png", &png, false)]), "Unknown image"),
            (test_zip::build(&[("skin.json", br#"{"images": {"a": "../a.png"}}"#, false)]), "must be a .png"),
            (test_zip::build(&[("skin.json", br#"{"images": {"a": "a.png"}}"#, false), ("a.png", b"xx", false)]), "not a readable PNG"),
            (test_zip::build(&[("skin.json", br#"{"images": {"a": "a.png"}}"#, false), ("a.png", &big, false)]), "the limit"),
            (test_zip::build(&[("skin.json", br#"{"colors": {"fill": "red"}}"#, false)]), "#RRGGBB"),
            (test_zip::build(&[("../evil.json", b"{}", false)]), "not a valid .zip"),
        ];
        for (zip, want) in cases {
            let err = read_pack(&zip).unwrap_err();
            assert!(err.contains(want), "expected {want:?}, got {err:?}");
        }
        // Corrupted data is caught by the CRC.
        let mut z = test_zip::build(&[("skin.json", b"{\"name\":\"ok\"}", false)]);
        let at = z.windows(4).position(|w| w == b"name").unwrap();
        z[at] = b'N';
        assert!(read_pack(&z).unwrap_err().contains("damaged"));
    }

    #[test]
    fn skin_ids_are_safe_folder_names() {
        assert_eq!(skin_id("Gold Rush!"), "gold-rush");
        assert_eq!(skin_id("../../etc"), "etc");
        assert_eq!(skin_id("ÄÖÜ"), "skin");
        assert!(skin_id(&"x".repeat(200)).len() <= 40);
    }

    /// The sample pack made with Python's zipfile (a real-world zip writer)
    /// reads and validates, if present.
    #[test]
    fn sample_pack_from_zipfile_reads() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let p = std::path::Path::new(&home).join(".hermes/cache/scratch/GoldRush-skin.zip");
        let Ok(bytes) = std::fs::read(&p) else { return };
        let pack = read_pack(&bytes).unwrap();
        assert_eq!(pack.manifest.name, "Gold Rush");
        assert_eq!(pack.files.len(), 8);
    }

    /// The generated Synthwave pack (docs/skins/make_synthwave_skin.py)
    /// validates, with its animations, if present.
    #[test]
    fn synthwave_pack_reads() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let p = std::path::Path::new(&home).join(".hermes/cache/scratch/Synthwave-skin.zip");
        let Ok(bytes) = std::fs::read(&p) else { return };
        let pack = read_pack(&bytes).unwrap();
        assert_eq!(pack.manifest.name, "Synthwave");
        assert!(pack.manifest.animations.len() >= 9);
        assert!(pack.manifest.animations["background_portrait"].frames == 8);
        assert!(pack.manifest.animations["background_portrait"].columns > 1);
    }

    #[test]
    fn animation_frames_and_scrolling() {
        // 2x2 grid: frames go left to right, then down.
        let g = Animation { frames: 4, columns: 2, fps: 1.0, ..Default::default() };
        let at = |t: f64| (g.uv(t).min.x, g.uv(t).min.y);
        assert_eq!(at(0.0), (0.0, 0.0));
        assert_eq!(at(1.1), (0.5, 0.0));
        assert_eq!(at(2.1), (0.0, 0.5));
        assert_eq!(at(3.1), (0.5, 0.5));
        assert_eq!(g.uv(0.0).size(), Vec2::new(0.5, 0.5));
        let odd = Animation { frames: 5, columns: 2, ..Default::default() };
        assert_eq!(odd.grid(), (2, 3));
        let a = Animation { frames: 4, fps: 2.0, ..Default::default() };
        assert_eq!(a.uv(0.0).min.y, 0.0);
        assert!((a.uv(0.6).min.y - 0.25).abs() < 1e-6, "frame 1 after 0.5 s");
        assert!((a.uv(2.1).min.y - 0.0).abs() < 1e-6, "loops after 4 frames");
        assert!((a.uv(0.0).height() - 0.25).abs() < 1e-6);
        let s = Animation { scroll_x: 0.5, ..Default::default() };
        assert!((s.uv(0.5).min.x - 0.25).abs() < 1e-6);
        assert!((s.uv(3.0).min.x - 0.5).abs() < 1e-6, "wraps");
        assert!(!Animation::default().is_animated());
        let png = test_zip::png(16, 30);
        let json = br#"{"images": {"a": "a.png"}, "animations": {"a": {"frames": 3, "fps": 8}}}"#;
        assert!(read_pack(&test_zip::build(&[("skin.json", json, false), ("a.png", &png, false)])).is_ok());
        let bad = [
            (&br#"{"images": {"a": "a.png"}, "animations": {"a": {"frames": 4}}}"#[..], "equal frames"),
            (&br#"{"images": {"a": "a.png"}, "animations": {"a": {"frames": 3, "columns": 4}}}"#[..], "columns"),
            (&br#"{"images": {"a": "a.png"}, "animations": {"a": {"frames": 3, "fps": 500}}}"#[..], "fps"),
            (&br#"{"images": {"a": "a.png"}, "animations": {"b": {"frames": 3}}}"#[..], "has no image"),
        ];
        for (json, want) in bad {
            let err = read_pack(&test_zip::build(&[("skin.json", json, false), ("a.png", &png, false)])).unwrap_err();
            assert!(err.contains(want), "{want}: {err}");
        }
    }

    #[test]
    fn image_keys() {
        assert_eq!(SkinImage::parse("a"), Some(SkinImage::Control(Control::A, false)));
        assert_eq!(SkinImage::parse("dpad_pressed"), Some(SkinImage::Control(Control::Dpad, true)));
        assert_eq!(SkinImage::parse("background_landscape"), Some(SkinImage::Background { portrait: false }));
        assert_eq!(SkinImage::parse("x_pressed"), None);
    }
}
