//! HD Sprite and Tile Replacement Packs (ROADMAP M7).
//!
//! Replaces native GBA tiles and sprites with high-resolution art assets,
//! keyed by deterministic 64-bit hashes of tile/sprite pixels and palettes.
//!
//! Key capabilities:
//! - Tile & composite sprite hashing (FNV-1a 64-bit) across 4bpp/8bpp and 1D/2D mapping.
//! - Pack manifest format (`manifest.json`) mapping hashes and palette variations to PNGs.
//! - Exact palette matching (e.g. Fire Mario, Shiny Pokémon) and wildcard matching.
//! - Dynamic palette recoloring (`recolor: true`) that tracks palette shifts (damage flashing, day/night cycles).
//! - Tile & sprite dump tool (`dump_tiles_and_sprites`) exporting PNGs and template `manifest.json`.
//! - High-resolution subpixel rendering and compositing with GBA hardware priority and color effects.

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, Read, Write},
    path::Path,
    sync::Arc,
};
use serde::{Deserialize, Serialize};

use super::ppu::{
    blend::{bgr555_to_rgb888, rgb888_to_bgr555},
    obj::get_sprite_size,
};

/// 64-bit FNV-1a deterministic hash implementation.
#[inline]
pub fn fnv1a_64(data: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Format a 64-bit hash as a 16-character lowercase hexadecimal string.
pub fn hash_to_hex(hash: u64) -> String {
    format!("{:016x}", hash)
}

/// Parse a hexadecimal string (with or without 0x prefix) into a 64-bit hash.
pub fn hex_to_hash(hex: &str) -> Option<u64> {
    let clean = hex.trim().trim_start_matches("0x").trim_start_matches("0X");
    u64::from_str_radix(clean, 16).ok()
}

/// Extract 64 pixel indices (8x8) for an individual tile from VRAM.
pub fn extract_tile_pixels(vram: &[u8], is_8bpp: bool, tile_addr: usize) -> [u8; 64] {
    let mut pixels = [0u8; 64];
    if is_8bpp {
        for y in 0..8 {
            for x in 0..8 {
                let addr = tile_addr + y * 8 + x;
                pixels[y * 8 + x] = if addr < vram.len() { vram[addr] } else { 0 };
            }
        }
    } else {
        for y in 0..8 {
            for x in 0..8 {
                let addr = tile_addr + y * 4 + (x / 2);
                let byte = if addr < vram.len() { vram[addr] } else { 0 };
                let idx = if x % 2 == 0 { byte & 0x0F } else { (byte >> 4) & 0x0F };
                pixels[y * 8 + x] = idx;
            }
        }
    }
    pixels
}

/// Extract palette entries (BGR555) from Palette RAM for BG or OBJ.
pub fn extract_palette(palette_ram: &[u8], pal_idx: usize, is_8bpp: bool, is_obj: bool) -> Vec<u16> {
    let base = if is_obj { 0x200 } else { 0x000 };
    if is_8bpp {
        let mut pal = Vec::with_capacity(256);
        for i in 0..256 {
            let addr = base + i * 2;
            let c = if addr + 1 < palette_ram.len() {
                (palette_ram[addr] as u16) | ((palette_ram[addr + 1] as u16) << 8)
            } else {
                0
            };
            pal.push(c);
        }
        pal
    } else {
        let mut pal = Vec::with_capacity(16);
        let pal_offset = base + pal_idx * 32;
        for i in 0..16 {
            let addr = pal_offset + i * 2;
            let c = if addr + 1 < palette_ram.len() {
                (palette_ram[addr] as u16) | ((palette_ram[addr + 1] as u16) << 8)
            } else {
                0
            };
            pal.push(c);
        }
        pal
    }
}

/// Compute a 64-bit deterministic hash for a palette.
pub fn hash_palette(palette: &[u16]) -> u64 {
    let mut bytes = Vec::with_capacity(palette.len() * 2);
    for &c in palette {
        bytes.extend_from_slice(&c.to_le_bytes());
    }
    fnv1a_64(&bytes)
}

/// Compute a 64-bit deterministic hash for an 8x8 tile.
pub fn hash_tile(vram: &[u8], is_8bpp: bool, tile_addr: usize) -> u64 {
    let pixels = extract_tile_pixels(vram, is_8bpp, tile_addr);
    fnv1a_64(&pixels)
}

/// Extracted raw GBA sprite data from OAM and VRAM.
#[derive(Debug, Clone)]
pub struct RawSpriteData {
    pub sprite_index: usize,
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
    pub palette: Vec<u16>,
    pub pal_num: usize,
    pub is_8bpp: bool,
    pub priority: u8,
    pub is_semi_trans: bool,
    pub hflip: bool,
    pub vflip: bool,
    pub is_affine: bool,
    pub sprite_x: i32,
    pub raw_y: i32,
    pub sprite_hash: u64,
    pub palette_hash: u64,
}

/// Extracts a sprite from OAM and VRAM in un-flipped sprite-local space.
pub fn extract_sprite(
    oam: &[u8],
    sprite_idx: usize,
    vram: &[u8],
    palette_ram: &[u8],
    dispcnt: u16,
) -> Option<RawSpriteData> {
    let oam_addr = sprite_idx * 8;
    if oam_addr + 6 > oam.len() {
        return None;
    }

    let attr0 = (oam[oam_addr] as u16) | ((oam[oam_addr + 1] as u16) << 8);
    let attr1 = (oam[oam_addr + 2] as u16) | ((oam[oam_addr + 3] as u16) << 8);
    let attr2 = (oam[oam_addr + 4] as u16) | ((oam[oam_addr + 5] as u16) << 8);

    let is_affine = (attr0 & (1 << 8)) != 0;
    let is_disabled = !is_affine && ((attr0 & (1 << 9)) != 0);
    if is_disabled {
        return None;
    }

    let obj_mode = (attr0 >> 10) & 3;
    let is_semi_trans = obj_mode == 1;
    let is_8bpp = (attr0 & (1 << 13)) != 0;
    let shape = (attr0 >> 14) & 3;
    let size = (attr1 >> 14) & 3;

    let (orig_w, orig_h) = get_sprite_size(shape, size);

    let raw_y = (attr0 & 0xFF) as i32;
    let raw_x = (attr1 & 0x1FF) as i32;
    let sprite_x = if raw_x >= 256 { raw_x - 512 } else { raw_x };

    let mapping_1d = (dispcnt & (1 << 6)) != 0;
    let obj_char_base = 0x10000;
    let raw_tile = (attr2 & 0x3FF) as usize;

    let tile_base = if is_8bpp && !mapping_1d {
        raw_tile & !1
    } else {
        raw_tile
    };

    let priority = ((attr2 >> 10) & 3) as u8;
    let pal_num = ((attr2 >> 12) & 0x0F) as usize;

    let (hflip, vflip) = if !is_affine {
        ((attr1 & (1 << 12)) != 0, (attr1 & (1 << 13)) != 0)
    } else {
        (false, false)
    };

    let mut pixels = Vec::with_capacity(orig_w * orig_h);

    for py in 0..orig_h {
        let tile_y = py / 8;
        let in_tile_y = py % 8;

        for px in 0..orig_w {
            let tile_x = px / 8;
            let in_tile_x = px % 8;

            let tile_offset = if mapping_1d {
                let tiles_per_row = orig_w / 8;
                if is_8bpp {
                    (tile_base + (tile_y * tiles_per_row + tile_x) * 2) & 0x3FF
                } else {
                    (tile_base + tile_y * tiles_per_row + tile_x) & 0x3FF
                }
            } else {
                if is_8bpp {
                    let tile_col = ((tile_base & 0x1F) + tile_x * 2) & 0x1F;
                    let tile_row = (((tile_base >> 5) & 0x1F) + tile_y) & 0x1F;
                    (tile_row * 32 + tile_col) & 0x3FF
                } else {
                    let tile_col = ((tile_base & 0x1F) + tile_x) & 0x1F;
                    let tile_row = (((tile_base >> 5) & 0x1F) + tile_y) & 0x1F;
                    (tile_row * 32 + tile_col) & 0x3FF
                }
            };

            let color_idx = if is_8bpp {
                let tile_addr = obj_char_base + tile_offset * 32 + in_tile_y * 8 + in_tile_x;
                if tile_addr < vram.len() {
                    vram[tile_addr]
                } else {
                    0
                }
            } else {
                let tile_addr = obj_char_base + tile_offset * 32 + in_tile_y * 4 + (in_tile_x / 2);
                if tile_addr < vram.len() {
                    let byte = vram[tile_addr];
                    if in_tile_x % 2 == 0 {
                        byte & 0x0F
                    } else {
                        (byte >> 4) & 0x0F
                    }
                } else {
                    0
                }
            };

            pixels.push(color_idx);
        }
    }

    let sprite_hash = fnv1a_64(&pixels);
    let palette = extract_palette(palette_ram, pal_num, is_8bpp, true);
    let palette_hash = hash_palette(&palette);

    Some(RawSpriteData {
        sprite_index: sprite_idx,
        width: orig_w,
        height: orig_h,
        pixels,
        palette,
        pal_num,
        is_8bpp,
        priority,
        is_semi_trans,
        hflip,
        vflip,
        is_affine,
        sprite_x,
        raw_y,
        sprite_hash,
        palette_hash,
    })
}

/// Convert raw sprite pixels and palette to 32-bit RGBA pixels (0xAABBGGRR).
pub fn sprite_to_rgba(sprite: &RawSpriteData) -> Vec<u32> {
    let mut rgba = Vec::with_capacity(sprite.width * sprite.height);
    for &idx in &sprite.pixels {
        if idx == 0 {
            rgba.push(0x0000_0000);
        } else {
            let color = if (idx as usize) < sprite.palette.len() {
                sprite.palette[idx as usize]
            } else {
                0
            };
            let (r, g, b) = bgr555_to_rgb888(color);
            rgba.push(0xFF00_0000 | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32));
        }
    }
    rgba
}

/// Convert 8x8 raw tile pixels and palette to 32-bit RGBA pixels.
pub fn tile_to_rgba(pixels: &[u8; 64], palette: &[u16]) -> [u32; 64] {
    let mut rgba = [0u32; 64];
    for i in 0..64 {
        let idx = pixels[i];
        if idx == 0 {
            rgba[i] = 0x0000_0000;
        } else {
            let color = if (idx as usize) < palette.len() {
                palette[idx as usize]
            } else {
                0
            };
            let (r, g, b) = bgr555_to_rgb888(color);
            rgba[i] = 0xFF00_0000 | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32);
        }
    }
    rgba
}

/// HD replacement pack manifest specification (`manifest.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HdPackManifest {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub game_title: Option<String>,
    #[serde(default)]
    pub game_code: Option<String>,
    #[serde(default = "default_scale")]
    pub scale: usize,
    #[serde(default)]
    pub replacements: Vec<HdReplacementEntry>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

fn default_scale() -> usize {
    4
}

/// An individual tile or sprite replacement entry in `manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HdReplacementEntry {
    /// "sprite" or "tile"
    #[serde(default = "default_entry_type")]
    pub r#type: String,
    /// 16-character hexadecimal hash of the tile or sprite pixels
    pub hash: String,
    /// 16-character hexadecimal hash of the palette, or "*" / None for wildcard
    #[serde(default)]
    pub palette_hash: Option<String>,
    /// Relative path to high-resolution PNG image
    pub file: String,
    /// Optional crop rectangle within the source image
    #[serde(default)]
    pub x: Option<u32>,
    #[serde(default)]
    pub y: Option<u32>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// Optional flip override: None = inherit from sprite, Some(bool) = explicit
    #[serde(default)]
    pub hflip: Option<bool>,
    #[serde(default)]
    pub vflip: Option<bool>,
    /// Dynamic recoloring flag
    #[serde(default)]
    pub recolor: bool,
    /// Base palette (BGR555 array) used for dynamic recoloring
    #[serde(default)]
    pub base_palette: Option<Vec<u16>>,
}

fn default_entry_type() -> String {
    "sprite".to_string()
}

/// High-resolution image representation with standard 32-bit RGBA pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct HdImage {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

impl HdImage {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width * height],
        }
    }

    pub fn from_rgba_bytes(width: usize, height: usize, bytes: &[u8]) -> Self {
        let mut pixels = Vec::with_capacity(width * height);
        for chunk in bytes.chunks_exact(4) {
            let r = chunk[0] as u32;
            let g = chunk[1] as u32;
            let b = chunk[2] as u32;
            let a = chunk[3] as u32;
            pixels.push((a << 24) | (b << 16) | (g << 8) | r);
        }
        Self { width, height, pixels }
    }

    pub fn to_rgba_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.width * self.height * 4);
        for &pix in &self.pixels {
            bytes.push((pix & 0xFF) as u8);
            bytes.push(((pix >> 8) & 0xFF) as u8);
            bytes.push(((pix >> 16) & 0xFF) as u8);
            bytes.push(((pix >> 24) & 0xFF) as u8);
        }
        bytes
    }
}

/// Loaded runtime HD replacement record.
#[derive(Clone, Debug)]
pub struct HdReplacement {
    pub is_sprite: bool,
    pub hash: u64,
    pub palette_hash: Option<u64>,
    pub image: HdImage,
    pub hflip: Option<bool>,
    pub vflip: Option<bool>,
    pub recolor: bool,
    pub base_palette: Option<Vec<u16>>,
}

/// Dynamically recolor a 32-bit RGBA pixel from `base_palette` to `current_palette`.
#[inline]
pub fn recolor_rgba(rgba: u32, base_pal: &[u16], current_pal: &[u16]) -> u32 {
    let alpha = (rgba >> 24) & 0xFF;
    if alpha == 0 || base_pal.is_empty() || current_pal.is_empty() {
        return rgba;
    }

    let r = (rgba & 0xFF) as i32;
    let g = ((rgba >> 8) & 0xFF) as i32;
    let b = ((rgba >> 16) & 0xFF) as i32;

    // 1. Exact match check
    let target_bgr = rgb888_to_bgr555(r as u8, g as u8, b as u8);
    for (k, &c_base) in base_pal.iter().enumerate().skip(1) {
        if c_base == target_bgr {
            if k < current_pal.len() {
                let (nr, ng, nb) = bgr555_to_rgb888(current_pal[k]);
                return (alpha << 24) | ((nb as u32) << 16) | ((ng as u32) << 8) | (nr as u32);
            }
        }
    }

    // 2. Nearest Euclidean distance match for anti-aliased / shaded pixels
    let mut best_idx = 1;
    let mut best_dist = i32::MAX;

    for (k, &c_base) in base_pal.iter().enumerate().skip(1) {
        let (br, bg, bb) = bgr555_to_rgb888(c_base);
        let dr = r - br as i32;
        let dg = g - bg as i32;
        let db = b - bb as i32;
        let dist = dr * dr + dg * dg + db * db;
        if dist < best_dist {
            best_dist = dist;
            best_idx = k;
        }
    }

    if best_idx < current_pal.len() {
        let (br, bg, bb) = bgr555_to_rgb888(base_pal[best_idx]);
        let (cr, cg, cb) = bgr555_to_rgb888(current_pal[best_idx]);
        let shift_r = cr as i32 - br as i32;
        let shift_g = cg as i32 - bg as i32;
        let shift_b = cb as i32 - bb as i32;

        let nr = (r + shift_r).clamp(0, 255) as u32;
        let ng = (g + shift_g).clamp(0, 255) as u32;
        let nb = (b + shift_b).clamp(0, 255) as u32;

        (alpha << 24) | (nb << 16) | (ng << 8) | nr
    } else {
        rgba
    }
}

/// Loaded in-memory HD replacement pack.
#[derive(Clone, Debug)]
pub struct HdPack {
    pub name: String,
    pub scale: usize,
    pub enabled: bool,
    exact_sprite_replacements: HashMap<(u64, u64), Arc<HdReplacement>>,
    wildcard_sprite_replacements: HashMap<u64, Arc<HdReplacement>>,
    exact_tile_replacements: HashMap<(u64, u64), Arc<HdReplacement>>,
    wildcard_tile_replacements: HashMap<u64, Arc<HdReplacement>>,
}

impl Default for HdPack {
    fn default() -> Self {
        Self::new("Default HD Pack", 4)
    }
}

impl HdPack {
    pub fn new(name: impl Into<String>, scale: usize) -> Self {
        Self {
            name: name.into(),
            scale: scale.max(1),
            enabled: true,
            exact_sprite_replacements: HashMap::new(),
            wildcard_sprite_replacements: HashMap::new(),
            exact_tile_replacements: HashMap::new(),
            wildcard_tile_replacements: HashMap::new(),
        }
    }

    /// Add an HD replacement to the pack.
    pub fn add_replacement(&mut self, replacement: HdReplacement) {
        let arc = Arc::new(replacement);
        if arc.is_sprite {
            if let Some(pal_hash) = arc.palette_hash {
                self.exact_sprite_replacements.insert((arc.hash, pal_hash), Arc::clone(&arc));
            } else {
                self.wildcard_sprite_replacements.insert(arc.hash, Arc::clone(&arc));
            }
        } else {
            if let Some(pal_hash) = arc.palette_hash {
                self.exact_tile_replacements.insert((arc.hash, pal_hash), Arc::clone(&arc));
            } else {
                self.wildcard_tile_replacements.insert(arc.hash, Arc::clone(&arc));
            }
        }
    }

    /// Total number of unique replacements loaded in the pack.
    pub fn len(&self) -> usize {
        self.exact_sprite_replacements.len()
            + self.wildcard_sprite_replacements.len()
            + self.exact_tile_replacements.len()
            + self.wildcard_tile_replacements.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of sprite replacements in the pack.
    pub fn sprite_count(&self) -> usize {
        self.exact_sprite_replacements.len() + self.wildcard_sprite_replacements.len()
    }

    /// Number of tile replacements in the pack.
    pub fn tile_count(&self) -> usize {
        self.exact_tile_replacements.len() + self.wildcard_tile_replacements.len()
    }

    /// Find an HD replacement for a sprite by its pixel hash and palette hash.
    pub fn find_sprite_replacement(&self, sprite_hash: u64, pal_hash: u64) -> Option<&Arc<HdReplacement>> {
        if let Some(rep) = self.exact_sprite_replacements.get(&(sprite_hash, pal_hash)) {
            return Some(rep);
        }
        self.wildcard_sprite_replacements.get(&sprite_hash)
    }

    /// Find an HD replacement for an 8x8 tile by its pixel hash and palette hash.
    pub fn find_tile_replacement(&self, tile_hash: u64, pal_hash: u64) -> Option<&Arc<HdReplacement>> {
        if let Some(rep) = self.exact_tile_replacements.get(&(tile_hash, pal_hash)) {
            return Some(rep);
        }
        self.wildcard_tile_replacements.get(&tile_hash)
    }

    /// Find a replacement for either sprite or tile.
    pub fn find_replacement(&self, hash: u64, pal_hash: u64) -> Option<&Arc<HdReplacement>> {
        self.find_sprite_replacement(hash, pal_hash)
            .or_else(|| self.find_tile_replacement(hash, pal_hash))
    }

    /// Load an HD replacement pack from a directory containing `manifest.json` and PNGs.
    pub fn load_from_dir<P: AsRef<Path>>(dir: P) -> io::Result<Self> {
        let dir = dir.as_ref();
        let manifest_path = dir.join("manifest.json");
        if !manifest_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("HD Pack manifest not found at '{}'", manifest_path.display()),
            ));
        }

        let mut manifest_str = String::new();
        File::open(&manifest_path)?.read_to_string(&mut manifest_str)?;
        let manifest: HdPackManifest = serde_json::from_str(&manifest_str).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("Failed to parse manifest: {}", e))
        })?;

        let mut pack = Self::new(manifest.name, manifest.scale);

        for entry in manifest.replacements {
            let hash = hex_to_hash(&entry.hash).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Invalid hex hash '{}'", entry.hash),
                )
            })?;

            let pal_hash = entry
                .palette_hash
                .as_deref()
                .and_then(|p| if p == "*" { None } else { hex_to_hash(p) });

            let png_path = dir.join(&entry.file);
            if !png_path.exists() {
                continue;
            }

            let png_bytes = fs::read(&png_path)?;
            let decoded = image::load_from_memory(&png_bytes).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Failed to decode PNG '{}': {}", png_path.display(), e),
                )
            })?;

            let rgba = decoded.to_rgba8();
            let src_w = rgba.width() as usize;
            let src_h = rgba.height() as usize;

            let crop_x = entry.x.unwrap_or(0) as usize;
            let crop_y = entry.y.unwrap_or(0) as usize;
            let crop_w = entry.width.unwrap_or(src_w as u32) as usize;
            let crop_h = entry.height.unwrap_or(src_h as u32) as usize;

            let final_w = crop_w.min(src_w.saturating_sub(crop_x));
            let final_h = crop_h.min(src_h.saturating_sub(crop_y));

            let mut pixels = Vec::with_capacity(final_w * final_h);
            for y in crop_y..(crop_y + final_h) {
                for x in crop_x..(crop_x + final_w) {
                    let p = rgba.get_pixel(x as u32, y as u32).0;
                    let r = p[0] as u32;
                    let g = p[1] as u32;
                    let b = p[2] as u32;
                    let a = p[3] as u32;
                    pixels.push((a << 24) | (b << 16) | (g << 8) | r);
                }
            }

            let is_sprite = entry.r#type.to_lowercase() == "sprite";

            pack.add_replacement(HdReplacement {
                is_sprite,
                hash,
                palette_hash: pal_hash,
                image: HdImage {
                    width: final_w,
                    height: final_h,
                    pixels,
                },
                hflip: entry.hflip,
                vflip: entry.vflip,
                recolor: entry.recolor,
                base_palette: entry.base_palette,
            });
        }

        Ok(pack)
    }

    /// Save a manifest struct to a `manifest.json` file.
    pub fn save_manifest<P: AsRef<Path>>(manifest: &HdPackManifest, path: P) -> io::Result<()> {
        let json = serde_json::to_string_pretty(manifest)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let mut file = File::create(path)?;
        file.write_all(json.as_bytes())
    }
}

/// Dumps visible tiles and sprites from VRAM, OAM, and Palette RAM into PNG files
/// and generates a template `manifest.json` for HD pack authors.
pub fn dump_tiles_and_sprites<P: AsRef<Path>>(
    vram: &[u8],
    oam: &[u8],
    palette_ram: &[u8],
    dispcnt: u16,
    output_dir: P,
) -> io::Result<HdPackManifest> {
    let output_dir = output_dir.as_ref();
    let sprites_dir = output_dir.join("sprites");
    let tiles_dir = output_dir.join("tiles");

    fs::create_dir_all(&sprites_dir)?;
    fs::create_dir_all(&tiles_dir)?;

    let mut replacements = Vec::new();
    let mut seen_sprites = HashMap::new();
    let mut seen_tiles = HashMap::new();

    // 1. Dump active sprites from OAM
    for sprite_idx in 0..128 {
        if let Some(raw_spr) = extract_sprite(oam, sprite_idx, vram, palette_ram, dispcnt) {
            let key = (raw_spr.sprite_hash, raw_spr.palette_hash);
            if seen_sprites.contains_key(&key) {
                continue;
            }
            seen_sprites.insert(key, true);

            let sprite_hex = hash_to_hex(raw_spr.sprite_hash);
            let pal_hex = hash_to_hex(raw_spr.palette_hash);
            let filename = format!("sprite_{}_{}_{}x{}.png", sprite_hex, pal_hex, raw_spr.width, raw_spr.height);
            let rel_path = format!("sprites/{}", filename);
            let full_path = sprites_dir.join(&filename);

            let rgba_words = sprite_to_rgba(&raw_spr);
            let mut raw_bytes = Vec::with_capacity(raw_spr.width * raw_spr.height * 4);
            for &pix in &rgba_words {
                raw_bytes.push((pix & 0xFF) as u8);
                raw_bytes.push(((pix >> 8) & 0xFF) as u8);
                raw_bytes.push(((pix >> 16) & 0xFF) as u8);
                raw_bytes.push(((pix >> 24) & 0xFF) as u8);
            }

            image::save_buffer(
                &full_path,
                &raw_bytes,
                raw_spr.width as u32,
                raw_spr.height as u32,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

            replacements.push(HdReplacementEntry {
                r#type: "sprite".to_string(),
                hash: sprite_hex,
                palette_hash: Some(pal_hex),
                file: rel_path,
                x: None,
                y: None,
                width: Some((raw_spr.width * 4) as u32),
                height: Some((raw_spr.height * 4) as u32),
                hflip: None,
                vflip: None,
                recolor: false,
                base_palette: Some(raw_spr.palette),
            });
        }
    }

    // 2. Dump unique non-empty 8x8 tiles from VRAM (OBJ & BG)
    // Check first 1024 tiles in BG char blocks
    let bg_palette = extract_palette(palette_ram, 0, false, false);
    let bg_pal_hex = hash_to_hex(hash_palette(&bg_palette));

    for tile_idx in 0..1024 {
        let tile_addr = tile_idx * 32;
        if tile_addr + 32 > vram.len() {
            break;
        }

        let pixels = extract_tile_pixels(vram, false, tile_addr);
        // Skip completely empty transparent tiles
        if pixels.iter().all(|&p| p == 0) {
            continue;
        }

        let tile_hash = fnv1a_64(&pixels);
        if seen_tiles.contains_key(&tile_hash) {
            continue;
        }
        seen_tiles.insert(tile_hash, true);

        let tile_hex = hash_to_hex(tile_hash);
        let filename = format!("tile_{}_{}.png", tile_hex, bg_pal_hex);
        let rel_path = format!("tiles/{}", filename);
        let full_path = tiles_dir.join(&filename);

        let rgba_words = tile_to_rgba(&pixels, &bg_palette);
        let mut raw_bytes = Vec::with_capacity(64 * 4);
        for &pix in &rgba_words {
            raw_bytes.push((pix & 0xFF) as u8);
            raw_bytes.push(((pix >> 8) & 0xFF) as u8);
            raw_bytes.push(((pix >> 16) & 0xFF) as u8);
            raw_bytes.push(((pix >> 24) & 0xFF) as u8);
        }

        image::save_buffer(
            &full_path,
            &raw_bytes,
            8,
            8,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        replacements.push(HdReplacementEntry {
            r#type: "tile".to_string(),
            hash: tile_hex,
            palette_hash: Some(bg_pal_hex.clone()),
            file: rel_path,
            x: None,
            y: None,
            width: Some(32),
            height: Some(32),
            hflip: None,
            vflip: None,
            recolor: false,
            base_palette: Some(bg_palette.clone()),
        });
    }

    let manifest = HdPackManifest {
        name: "Dumped Pack Template".to_string(),
        version: "1.0.0".to_string(),
        author: "CrabBoy-Advance Tile Dumper".to_string(),
        game_title: None,
        game_code: None,
        scale: 4,
        replacements,
    };

    let manifest_path = output_dir.join("manifest.json");
    HdPack::save_manifest(&manifest, manifest_path)?;

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fnv1a_hashing_and_hex() {
        let data1 = b"CrabBoyAdvanceTileData";
        let data2 = b"CrabBoyAdvanceTileData2";

        let hash1 = fnv1a_64(data1);
        let hash2 = fnv1a_64(data2);
        assert_ne!(hash1, hash2);

        let hex1 = hash_to_hex(hash1);
        assert_eq!(hex1.len(), 16);

        let parsed1 = hex_to_hash(&hex1);
        assert_eq!(parsed1, Some(hash1));

        let parsed_prefixed = hex_to_hash(&format!("0x{}", hex1));
        assert_eq!(parsed_prefixed, Some(hash1));
    }

    #[test]
    fn test_recolor_rgba_exact_and_shifted() {
        // Base palette: 0 = transparent, 1 = red (31, 0, 0)
        let base_pal = vec![0x0000, 0x001F, 0x03E0]; // Black/trans, Red, Green
        // Current palette: 1 = blue (0, 0, 31)
        let curr_pal = vec![0x0000, 0x7C00, 0x001F]; // Black/trans, Blue, Red

        // RGBA for Red (0xFF0000FF)
        let (r, g, b) = bgr555_to_rgb888(0x001F);
        let orig_pixel = 0xFF00_0000 | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32);

        let recolored = recolor_rgba(orig_pixel, &base_pal, &curr_pal);
        let rec_r = (recolored & 0xFF) as u8;
        let rec_g = ((recolored >> 8) & 0xFF) as u8;
        let rec_b = ((recolored >> 16) & 0xFF) as u8;
        let rec_a = ((recolored >> 24) & 0xFF) as u8;

        assert_eq!(rec_a, 255);
        let (expected_r, expected_g, expected_b) = bgr555_to_rgb888(0x7C00); // Blue
        assert_eq!(rec_r, expected_r);
        assert_eq!(rec_g, expected_g);
        assert_eq!(rec_b, expected_b);
    }
}
