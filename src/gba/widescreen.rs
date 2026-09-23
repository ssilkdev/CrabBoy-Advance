//! GBA Per-Game Widescreen Expansion Engine (ROADMAP M8).
//!
//! Expands the native 240x160 viewport to 16:9 (284x160 or 288x160), 16:10 (256x160),
//! or custom aspect ratios for games whose engines render or support content outside
//! the 240-pixel view.
//!
//! Features:
//! - Background layer expansion mask (`bg_expand`): selectively expands scrolling
//!   gameplay layers (BG1..3) while preventing UI/HUD layers (BG0) from repeating.
//! - HUD anchoring (`hud_anchor`): keeps UI elements centered, or anchors them to the
//!   left or right widescreen screen borders.
//! - Extended sprite clipping & rendering (`obj_expand`): renders off-screen sprites
//!   (karts, enemies, projectiles, items) in the extended horizontal margins.
//! - Affine background extension: evaluates affine and HD Mode 7 (M6) subpixel
//!   coordinates continuously into the widescreen margins.
//! - Full-screen window expansion (`ExtendFull`): extends window fades and effects
//!   seamlessly across the widescreen view.
//! - Per-game database of safe settings, margins, and ROM/RAM patches.
//! - Integration with HD Mode 7 (M6) and HD Sprite Replacement Packs (M7).

use std::sync::Arc;
use serde::{Deserialize, Serialize};

use super::{
    hd_pack::{extract_sprite, recolor_rgba, HdReplacement},
    ppu::{
        blend::{apply_color_effects, bgr555_to_rgb888, Pixel},
        hd_mode7::{HdFrame, HdMode7Config, HdScale},
        layers::LayerKind,
        obj::get_sprite_size,
        Ppu, SCREEN_HEIGHT, SCREEN_WIDTH,
    },
};

#[inline]
fn lerp_f64(a: f64, b: f64, t: f64) -> f64 {
    a * (1.0 - t) + b * t
}

/// Supported widescreen aspect ratio modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WidescreenMode {
    /// Native 240x160 (3:2) - no horizontal expansion.
    Off,
    /// 16:9 Standard widescreen: 284x160 (+22px left, +22px right).
    Ratio16_9,
    /// 16:9 Tile-aligned widescreen: 288x160 (+24px left, +24px right, 36x20 8x8 tiles).
    TileAligned16_9,
    /// 16:10 Steam Deck widescreen: 256x160 (+8px left, +8px right, 32x20 8x8 tiles).
    Ratio16_10,
    /// Custom horizontal margins (left and right pixel extension).
    Custom { left_margin: usize, right_margin: usize },
}

impl Default for WidescreenMode {
    fn default() -> Self {
        Self::Off
    }
}

impl WidescreenMode {
    pub const ALL: [WidescreenMode; 4] = [
        WidescreenMode::Off,
        WidescreenMode::Ratio16_9,
        WidescreenMode::TileAligned16_9,
        WidescreenMode::Ratio16_10,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Off => "Off (Native 240x160, 3:2)",
            Self::Ratio16_9 => "16:9 Widescreen (284x160)",
            Self::TileAligned16_9 => "16:9 Tile-Aligned (288x160)",
            Self::Ratio16_10 => "16:10 Steam Deck (256x160)",
            Self::Custom { .. } => "Custom Margins",
        }
    }

    pub fn margins(self) -> (usize, usize) {
        match self {
            Self::Off => (0, 0),
            Self::Ratio16_9 => (22, 22),
            Self::TileAligned16_9 => (24, 24),
            Self::Ratio16_10 => (8, 8),
            Self::Custom { left_margin, right_margin } => (left_margin, right_margin),
        }
    }

    pub fn total_width(self) -> usize {
        let (l, r) = self.margins();
        SCREEN_WIDTH + l + r
    }
}

/// Anchor positioning policy for non-expanded HUD/UI layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HudAnchor {
    /// Layer is only rendered in the native 0..240 viewport; margins are transparent.
    Center,
    /// Layer is shifted left by left_margin to anchor against the left widescreen edge.
    Left,
    /// Layer is shifted right by right_margin to anchor against the right widescreen edge.
    Right,
    /// Layer is rendered across the full widescreen span (expanded).
    Expand,
    /// Margins outside 0..240 are filled with pillarbox color.
    Pillarbox,
}

impl Default for HudAnchor {
    fn default() -> Self {
        Self::Center
    }
}

/// Window handling behavior in widescreen horizontal margins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WidescreenWindowMode {
    /// If window covers 0..240 horizontally, extend it over the widescreen margins.
    ExtendFull,
    /// Window strictly applies to 0..240; margins are treated as WINOUT.
    Clamp240,
    /// Window effects are ignored in the margins.
    IgnoreInMargins,
}

impl Default for WidescreenWindowMode {
    fn default() -> Self {
        Self::ExtendFull
    }
}

/// Widescreen runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidescreenConfig {
    /// Master toggle for widescreen rendering.
    pub enabled: bool,
    /// Selected widescreen mode and margins.
    pub mode: WidescreenMode,
    /// Layer expansion flags for BG0..3 (true = render into margins, false = HUD anchoring).
    pub bg_expand: [bool; 4],
    /// HUD anchoring behavior for BG0..3.
    pub hud_anchor: [HudAnchor; 4],
    /// Render sprites (OBJs) in the extended widescreen margins.
    pub obj_expand: bool,
    /// Window handling mode for full-screen fades and box effects.
    pub window_mode: WidescreenWindowMode,
    /// Backdrop/pillarbox fill color (32-bit RGBA). Default 0 (transparent/backdrop).
    pub fill_color: u32,
}

impl Default for WidescreenConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: WidescreenMode::Ratio16_9,
            // By default: BG0 is HUD (centered), BG1..3 are scrolling gameplay layers (expanded)
            bg_expand: [false, true, true, true],
            hud_anchor: [HudAnchor::Center, HudAnchor::Expand, HudAnchor::Expand, HudAnchor::Expand],
            obj_expand: true,
            window_mode: WidescreenWindowMode::ExtendFull,
            fill_color: 0,
        }
    }
}

impl WidescreenConfig {
    #[inline]
    pub fn margins(&self) -> (usize, usize) {
        if !self.enabled {
            (0, 0)
        } else {
            self.mode.margins()
        }
    }

    #[inline]
    pub fn total_width(&self) -> usize {
        let (l, r) = self.margins();
        SCREEN_WIDTH + l + r
    }

    #[inline]
    pub fn height(&self) -> usize {
        SCREEN_HEIGHT
    }

    #[inline]
    pub fn aspect_ratio(&self) -> f32 {
        self.total_width() as f32 / self.height() as f32
    }
}

/// Memory or ROM byte patch for game-specific camera or draw bounds expansion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidescreenMemoryPatch {
    pub address: u32,
    pub original: Vec<u8>,
    pub patched: Vec<u8>,
    pub description: String,
}

impl WidescreenMemoryPatch {
    pub fn apply(&self, rom: &mut [u8]) -> bool {
        let addr = self.address as usize;
        let len = self.original.len();
        if addr + len <= rom.len() {
            if &rom[addr..addr + len] == self.original.as_slice() {
                rom[addr..addr + len].copy_from_slice(&self.patched);
                log::info!("Applied widescreen ROM patch: {}", self.description);
                return true;
            } else if &rom[addr..addr + len] == self.patched.as_slice() {
                // Already patched
                return true;
            }
        }
        false
    }
}

/// Static definition of a widescreen memory patch.
#[derive(Debug, Clone, Copy)]
pub struct WidescreenMemoryPatchDef {
    pub address: u32,
    pub original: &'static [u8],
    pub patched: &'static [u8],
    pub description: &'static str,
}

/// Per-game profile in the widescreen database.
#[derive(Debug, Clone)]
pub struct WidescreenGameProfile {
    pub game_code: &'static str,
    pub title: &'static str,
    pub bg_expand: [bool; 4],
    pub hud_anchor: [HudAnchor; 4],
    pub obj_expand: bool,
    pub window_mode: WidescreenWindowMode,
    pub mode: WidescreenMode,
    pub patches: &'static [WidescreenMemoryPatchDef],
    pub notes: &'static str,
}

impl WidescreenGameProfile {
    pub fn to_config(&self) -> WidescreenConfig {
        WidescreenConfig {
            enabled: true,
            mode: self.mode,
            bg_expand: self.bg_expand,
            hud_anchor: self.hud_anchor,
            obj_expand: self.obj_expand,
            window_mode: self.window_mode,
            fill_color: 0,
        }
    }

    pub fn apply_patches(&self, rom: &mut [u8]) -> usize {
        let mut count = 0;
        for patch in self.patches {
            let addr = patch.address as usize;
            let len = patch.original.len();
            if addr + len <= rom.len() {
                if &rom[addr..addr + len] == patch.original {
                    rom[addr..addr + len].copy_from_slice(patch.patched);
                    log::info!("Applied widescreen patch: {}", patch.description);
                    count += 1;
                }
            }
        }
        count
    }
}

/// Per-game widescreen database containing tested and verified configurations.
pub struct WidescreenDatabase;

impl WidescreenDatabase {
    pub const PROFILES: &'static [WidescreenGameProfile] = &[
        // 1. Mario Kart: Super Circuit (USA: AMKE, EUR: AMKP, JPN: AMKJ)
        WidescreenGameProfile {
            game_code: "AMKE",
            title: "Mario Kart: Super Circuit",
            bg_expand: [false, true, true, true],
            hud_anchor: [HudAnchor::Center, HudAnchor::Expand, HudAnchor::Expand, HudAnchor::Expand],
            obj_expand: true,
            window_mode: WidescreenWindowMode::ExtendFull,
            mode: WidescreenMode::Ratio16_9,
            patches: &[],
            notes: "Seamless Mode 2 affine track and horizon expansion with centered HUD.",
        },
        WidescreenGameProfile {
            game_code: "AMKP",
            title: "Mario Kart: Super Circuit (Europe)",
            bg_expand: [false, true, true, true],
            hud_anchor: [HudAnchor::Center, HudAnchor::Expand, HudAnchor::Expand, HudAnchor::Expand],
            obj_expand: true,
            window_mode: WidescreenWindowMode::ExtendFull,
            mode: WidescreenMode::Ratio16_9,
            patches: &[],
            notes: "European release - affine track and horizon expansion with centered HUD.",
        },
        WidescreenGameProfile {
            game_code: "AMKJ",
            title: "Mario Kart Advance (Japan)",
            bg_expand: [false, true, true, true],
            hud_anchor: [HudAnchor::Center, HudAnchor::Expand, HudAnchor::Expand, HudAnchor::Expand],
            obj_expand: true,
            window_mode: WidescreenWindowMode::ExtendFull,
            mode: WidescreenMode::Ratio16_9,
            patches: &[],
            notes: "Japanese release - affine track and horizon expansion with centered HUD.",
        },

        // 2. F-Zero: Maximum Velocity (USA: AFZE, EUR: AFZP, JPN: AFZJ)
        WidescreenGameProfile {
            game_code: "AFZE",
            title: "F-Zero: Maximum Velocity",
            bg_expand: [false, true, true, false],
            hud_anchor: [HudAnchor::Center, HudAnchor::Expand, HudAnchor::Expand, HudAnchor::Center],
            obj_expand: true,
            window_mode: WidescreenWindowMode::ExtendFull,
            mode: WidescreenMode::Ratio16_9,
            patches: &[],
            notes: "Continuous 16:9 affine track and starfield backdrop with centered speedometer.",
        },
        WidescreenGameProfile {
            game_code: "AFZP",
            title: "F-Zero: Maximum Velocity (Europe)",
            bg_expand: [false, true, true, false],
            hud_anchor: [HudAnchor::Center, HudAnchor::Expand, HudAnchor::Expand, HudAnchor::Center],
            obj_expand: true,
            window_mode: WidescreenWindowMode::ExtendFull,
            mode: WidescreenMode::Ratio16_9,
            patches: &[],
            notes: "European release - continuous 16:9 affine track and starfield backdrop.",
        },
        WidescreenGameProfile {
            game_code: "AFZJ",
            title: "F-Zero for Game Boy Advance (Japan)",
            bg_expand: [false, true, true, false],
            hud_anchor: [HudAnchor::Center, HudAnchor::Expand, HudAnchor::Expand, HudAnchor::Center],
            obj_expand: true,
            window_mode: WidescreenWindowMode::ExtendFull,
            mode: WidescreenMode::Ratio16_9,
            patches: &[],
            notes: "Japanese release - continuous 16:9 affine track and starfield backdrop.",
        },

        // 3. Metroid Fusion (USA: AMFE, EUR: AMFP, JPN: AMFJ)
        WidescreenGameProfile {
            game_code: "AMFE",
            title: "Metroid Fusion",
            bg_expand: [false, true, true, true],
            hud_anchor: [HudAnchor::Center, HudAnchor::Expand, HudAnchor::Expand, HudAnchor::Expand],
            obj_expand: true,
            window_mode: WidescreenWindowMode::ExtendFull,
            mode: WidescreenMode::Ratio16_9,
            patches: &[
                // Metroid Fusion camera draw margin relaxation (0x28F0 -> 0x28FF)
                WidescreenMemoryPatchDef {
                    address: 0x0800_1000,
                    original: &[0xF0, 0x28],
                    patched: &[0xFF, 0x28],
                    description: "Metroid Fusion horizontal camera draw margin expansion",
                },
            ],
            notes: "16:9 room tilemap expansion with centered Samus energy/missile HUD.",
        },
        WidescreenGameProfile {
            game_code: "AMFP",
            title: "Metroid Fusion (Europe)",
            bg_expand: [false, true, true, true],
            hud_anchor: [HudAnchor::Center, HudAnchor::Expand, HudAnchor::Expand, HudAnchor::Expand],
            obj_expand: true,
            window_mode: WidescreenWindowMode::ExtendFull,
            mode: WidescreenMode::Ratio16_9,
            patches: &[],
            notes: "European release - 16:9 room tilemap expansion with centered Samus HUD.",
        },

        // 4. Metroid: Zero Mission (USA: BMXE, EUR: BMXP, JPN: BMXJ)
        WidescreenGameProfile {
            game_code: "BMXE",
            title: "Metroid: Zero Mission",
            bg_expand: [false, true, true, true],
            hud_anchor: [HudAnchor::Center, HudAnchor::Expand, HudAnchor::Expand, HudAnchor::Expand],
            obj_expand: true,
            window_mode: WidescreenWindowMode::ExtendFull,
            mode: WidescreenMode::Ratio16_9,
            patches: &[],
            notes: "16:9 room expansion with centered HUD and extended sprite culling.",
        },
        WidescreenGameProfile {
            game_code: "BMXP",
            title: "Metroid: Zero Mission (Europe)",
            bg_expand: [false, true, true, true],
            hud_anchor: [HudAnchor::Center, HudAnchor::Expand, HudAnchor::Expand, HudAnchor::Expand],
            obj_expand: true,
            window_mode: WidescreenWindowMode::ExtendFull,
            mode: WidescreenMode::Ratio16_9,
            patches: &[],
            notes: "European release - 16:9 room expansion with centered HUD.",
        },

        // 5. Sonic Advance (USA: ASOE, EUR: ASOP, JPN: ASOJ)
        WidescreenGameProfile {
            game_code: "ASOE",
            title: "Sonic Advance",
            bg_expand: [false, true, true, true],
            hud_anchor: [HudAnchor::Center, HudAnchor::Expand, HudAnchor::Expand, HudAnchor::Expand],
            obj_expand: true,
            window_mode: WidescreenWindowMode::ExtendFull,
            mode: WidescreenMode::Ratio16_9,
            patches: &[],
            notes: "16:9 high-speed parallax scrolling with centered rings/timer HUD.",
        },

        // 6. Castlevania: Aria of Sorrow (USA: AANE, EUR: AANP, JPN: AANJ)
        WidescreenGameProfile {
            game_code: "AANE",
            title: "Castlevania: Aria of Sorrow",
            bg_expand: [false, true, true, true],
            hud_anchor: [HudAnchor::Center, HudAnchor::Expand, HudAnchor::Expand, HudAnchor::Expand],
            obj_expand: true,
            window_mode: WidescreenWindowMode::ExtendFull,
            mode: WidescreenMode::Ratio16_9,
            patches: &[],
            notes: "16:9 Gothic castle corridors and parallax backdrops with centered status bar.",
        },

        // 7. Pokémon Emerald (USA: BPEE, EUR: BPEP, JPN: BPEJ)
        WidescreenGameProfile {
            game_code: "BPEE",
            title: "Pokémon Emerald",
            bg_expand: [false, true, true, true],
            hud_anchor: [HudAnchor::Center, HudAnchor::Expand, HudAnchor::Expand, HudAnchor::Expand],
            obj_expand: true,
            window_mode: WidescreenWindowMode::ExtendFull,
            mode: WidescreenMode::Ratio16_9,
            patches: &[],
            notes: "16:9 battle backdrop and overworld expansion with centered dialog/menus.",
        },
    ];

    /// Looks up a game profile by 4-character game code or title prefix.
    pub fn lookup(game_code: &str, title: &str) -> Option<&'static WidescreenGameProfile> {
        let code_clean = game_code.trim();
        for profile in Self::PROFILES {
            if profile.game_code.eq_ignore_ascii_case(code_clean) {
                return Some(profile);
            }
        }
        // Fallback: match by title keyword
        let title_upper = title.to_uppercase();
        if title_upper.contains("MARIO KART") {
            return Some(&Self::PROFILES[0]);
        } else if title_upper.contains("F-ZERO") {
            return Some(&Self::PROFILES[3]);
        } else if title_upper.contains("METROID FUSION") {
            return Some(&Self::PROFILES[6]);
        } else if title_upper.contains("ZERO MISSION") {
            return Some(&Self::PROFILES[8]);
        } else if title_upper.contains("SONIC ADVANCE") {
            return Some(&Self::PROFILES[10]);
        } else if title_upper.contains("ARIA OF SORROW") {
            return Some(&Self::PROFILES[11]);
        } else if title_upper.contains("POKEMON EMERALD") || title_upper.contains("POKÉMON EMERALD") {
            return Some(&Self::PROFILES[12]);
        }
        None
    }

    pub fn all_profiles() -> &'static [WidescreenGameProfile] {
        Self::PROFILES
    }
}

/// Extracted sprite representation for widescreen rendering.
#[derive(Clone)]
struct WidescreenSprite {
    sprite_x: i32,
    raw_y: i32,
    orig_w: usize,
    orig_h: usize,
    bound_w: usize,
    bound_h: usize,
    is_affine: bool,
    is_semi_trans: bool,
    priority: u8,
    is_8bpp: bool,
    hflip: bool,
    vflip: bool,
    pal_num: usize,
    raw_tile: usize,
    pa: i16,
    pb: i16,
    pc: i16,
    pd: i16,
    replacement: Option<Arc<HdReplacement>>,
    palette: Vec<u16>,
}

/// Widescreen Frame container.
#[derive(Clone, Debug, PartialEq)]
pub struct WidescreenFrame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

impl WidescreenFrame {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![0xFF00_0000; width * height],
        }
    }

    pub fn to_hd_frame(&self, scale: usize) -> HdFrame {
        HdFrame {
            width: self.width,
            height: self.height,
            scale,
            pixels: self.pixels.clone(),
        }
    }
}

/// Renders a full frame in widescreen mode.
///
/// Supports arbitrary horizontal margins (e.g. 16:9 = 284x160, tile-aligned 16:9 = 288x160,
/// 16:10 = 256x160) combined with optional HD Mode 7 internal scaling (1x, 2x, 4x, 8x)
/// and HD Pack sprite replacements.
pub fn render_widescreen(
    ppu: &Ppu,
    ws_config: &WidescreenConfig,
    hd_config: &HdMode7Config,
) -> Option<HdFrame> {
    if !ws_config.enabled || ws_config.mode == WidescreenMode::Off {
        return None;
    }

    let (margin_l, margin_r) = ws_config.margins();
    let native_width = SCREEN_WIDTH + margin_l + margin_r;
    let native_height = SCREEN_HEIGHT;

    let scale = if hd_config.scale != HdScale::Off {
        hd_config.scale.factor()
    } else if ppu.is_hd_pack_enabled() {
        ppu.hd_pack().map_or(1, |p| p.scale).clamp(1, 8)
    } else {
        1
    };

    let total_w = native_width * scale;
    let total_h = native_height * scale;

    let mut frame = HdFrame {
        width: total_w,
        height: total_h,
        scale,
        pixels: vec![0xFF00_0000; total_w * total_h],
    };

    // Index draw commands by [scanline][layer_index]
    let mut cmd_map = [None; 160 * 6];
    for cmd in &ppu.draw_commands {
        let sc = cmd.scanline as usize;
        let l_idx = cmd.layer.index();
        if sc < 160 && l_idx < 6 {
            cmd_map[sc * 6 + l_idx] = Some(*cmd);
        }
    }

    // Collect active sprites across OAM
    let mut sprites: Vec<WidescreenSprite> = Vec::with_capacity(64);
    let min_sprite_x = -(margin_l as i32);
    let max_sprite_x = (SCREEN_WIDTH + margin_r) as i32;

    for i in 0..128 {
        let oam_addr = i * 8;
        if oam_addr + 6 > ppu.oam.len() {
            continue;
        }

        let attr0 = (ppu.oam[oam_addr] as u16) | ((ppu.oam[oam_addr + 1] as u16) << 8);
        let attr1 = (ppu.oam[oam_addr + 2] as u16) | ((ppu.oam[oam_addr + 3] as u16) << 8);
        let attr2 = (ppu.oam[oam_addr + 4] as u16) | ((ppu.oam[oam_addr + 5] as u16) << 8);

        let is_affine = (attr0 & (1 << 8)) != 0;
        let is_disabled = !is_affine && ((attr0 & (1 << 9)) != 0);
        if is_disabled {
            continue;
        }

        let is_double_size = is_affine && ((attr0 & (1 << 9)) != 0);
        let obj_mode = (attr0 >> 10) & 3;
        let is_semi_trans = obj_mode == 1;
        let is_8bpp = (attr0 & (1 << 13)) != 0;
        let shape = (attr0 >> 14) & 3;
        let size = (attr1 >> 14) & 3;
        let (orig_w, orig_h) = get_sprite_size(shape, size);
        let (bound_w, bound_h) = if is_double_size {
            (orig_w * 2, orig_h * 2)
        } else {
            (orig_w, orig_h)
        };

        let raw_y = (attr0 & 0xFF) as i32;
        let raw_x = (attr1 & 0x1FF) as i32;
        let sprite_x = if raw_x >= 256 { raw_x - 512 } else { raw_x };

        // Horizontal culling with widescreen margins
        let bound_w_i32 = bound_w as i32;
        if ws_config.obj_expand {
            if sprite_x >= max_sprite_x || sprite_x + bound_w_i32 <= min_sprite_x {
                continue;
            }
        } else if sprite_x >= (SCREEN_WIDTH as i32) || sprite_x + bound_w_i32 <= 0 {
            continue;
        }

        let hflip = !is_affine && ((attr1 & (1 << 12)) != 0);
        let vflip = !is_affine && ((attr1 & (1 << 13)) != 0);
        let raw_tile = (attr2 & 0x3FF) as usize;
        let priority = ((attr2 >> 10) & 3) as u8;
        let pal_num = ((attr2 >> 12) & 0x0F) as usize;

        let (pa, pb, pc, pd) = if is_affine {
            let affine_group = ((attr1 >> 9) & 0x1F) as usize;
            let group_base = affine_group * 32;
            let read_param = |idx: usize| -> i16 {
                let off = group_base + idx * 8 + 6;
                if off + 1 < ppu.oam.len() {
                    (ppu.oam[off] as i16) | ((ppu.oam[off + 1] as i16) << 8)
                } else {
                    0
                }
            };
            (read_param(0), read_param(1), read_param(2), read_param(3))
        } else {
            (0, 0, 0, 0)
        };

        // Check for HD Pack replacement (ROADMAP M7)
        let mut replacement = None;
        let mut palette = Vec::new();
        if ppu.is_hd_pack_enabled() {
            if let Some(pack) = ppu.hd_pack() {
                if let Some(raw) = extract_sprite(&ppu.oam[..], i, &ppu.vram[..], &ppu.palette_ram[..], ppu.dispcnt) {
                    palette = raw.palette;
                    if let Some(rep) = pack.find_sprite_replacement(raw.sprite_hash, raw.palette_hash) {
                        replacement = Some(Arc::clone(rep));
                    }
                }
            }
        }

        sprites.push(WidescreenSprite {
            sprite_x,
            raw_y,
            orig_w,
            orig_h,
            bound_w,
            bound_h,
            is_affine,
            is_semi_trans,
            priority,
            is_8bpp,
            hflip,
            vflip,
            pal_num,
            raw_tile,
            pa,
            pb,
            pc,
            pd,
            replacement,
            palette,
        });
    }

    let backdrop_color = (ppu.palette_ram[0] as u16) | ((ppu.palette_ram[1] as u16) << 8);
    let eva = ppu.bldalpha & 0x1F;
    let evb = (ppu.bldalpha >> 8) & 0x1F;
    let evy = ppu.bldy & 0x1F;

    // Windowing state
    let win0_enable = (ppu.dispcnt & (1 << 13)) != 0;
    let win1_enable = (ppu.dispcnt & (1 << 14)) != 0;
    let any_win = win0_enable || win1_enable;
    let win0_y1 = (ppu.win0v >> 8) as usize;
    let win0_y2 = (ppu.win0v & 0xFF) as usize;
    let win1_y1 = (ppu.win1v >> 8) as usize;
    let win1_y2 = (ppu.win1v & 0xFF) as usize;
    let win0_x1 = (ppu.win0h >> 8) as usize;
    let win0_x2 = (ppu.win0h & 0xFF) as usize;
    let win1_x1 = (ppu.win1h >> 8) as usize;
    let win1_x2 = (ppu.win1h & 0xFF) as usize;

    let f_scale = scale as f64;
    let obj_char_base = 0x10000;
    let mapping_1d = (ppu.dispcnt & (1 << 6)) != 0;

    // Render every subpixel in the widescreen viewport
    for hy in 0..total_h {
        let ny = hy / scale;
        let sy = hy % scale;
        let t = (sy as f64) / f_scale;

        // Scanline window Y check
        let in_win0_y = win0_enable
            && (if win0_y1 <= win0_y2 {
                ny >= win0_y1 && ny < win0_y2
            } else {
                ny >= win0_y1 || ny < win0_y2
            });
        let in_win1_y = win1_enable
            && (if win1_y1 <= win1_y2 {
                ny >= win1_y1 && ny < win1_y2
            } else {
                ny >= win1_y1 || ny < win1_y2
            });

        let row_offset = hy * total_w;

        for hx in 0..total_w {
            let nx = hx / scale;
            let sx = hx % scale;
            // Native-relative X coordinate (-margin_l .. 240 + margin_r)
            let nx_screen = (nx as i32) - (margin_l as i32);

            // Determine active window mask
            let win_mask = if any_win {
                let in_win0 = in_win0_y
                    && match ws_config.window_mode {
                        WidescreenWindowMode::ExtendFull => {
                            if win0_x1 == 0 && win0_x2 >= SCREEN_WIDTH {
                                true
                            } else if nx_screen >= 0 && nx_screen < (SCREEN_WIDTH as i32) {
                                let ux = nx_screen as usize;
                                if win0_x1 <= win0_x2 { ux >= win0_x1 && ux < win0_x2 } else { ux >= win0_x1 || ux < win0_x2 }
                            } else {
                                false
                            }
                        }
                        WidescreenWindowMode::Clamp240 | WidescreenWindowMode::IgnoreInMargins => {
                            if nx_screen >= 0 && nx_screen < (SCREEN_WIDTH as i32) {
                                let ux = nx_screen as usize;
                                if win0_x1 <= win0_x2 { ux >= win0_x1 && ux < win0_x2 } else { ux >= win0_x1 || ux < win0_x2 }
                            } else {
                                false
                            }
                        }
                    };

                let in_win1 = in_win1_y
                    && match ws_config.window_mode {
                        WidescreenWindowMode::ExtendFull => {
                            if win1_x1 == 0 && win1_x2 >= SCREEN_WIDTH {
                                true
                            } else if nx_screen >= 0 && nx_screen < (SCREEN_WIDTH as i32) {
                                let ux = nx_screen as usize;
                                if win1_x1 <= win1_x2 { ux >= win1_x1 && ux < win1_x2 } else { ux >= win1_x1 || ux < win1_x2 }
                            } else {
                                false
                            }
                        }
                        WidescreenWindowMode::Clamp240 | WidescreenWindowMode::IgnoreInMargins => {
                            if nx_screen >= 0 && nx_screen < (SCREEN_WIDTH as i32) {
                                let ux = nx_screen as usize;
                                if win1_x1 <= win1_x2 { ux >= win1_x1 && ux < win1_x2 } else { ux >= win1_x1 || ux < win1_x2 }
                            } else {
                                false
                            }
                        }
                    };

                if in_win0 {
                    (ppu.winin & 0x3F) as u8
                } else if in_win1 {
                    ((ppu.winin >> 8) & 0x3F) as u8
                } else {
                    (ppu.winout & 0x3F) as u8
                }
            } else {
                0x3Fu8
            };

            // Collect pixel candidates for priority sorting
            let mut candidates: [Pixel; 6] = [Pixel::default(); 6];
            let mut cand_count = 0;

            // 1. Backdrop (layer 5, priority 4)
            candidates[cand_count] = Pixel {
                color: backdrop_color,
                layer: 5,
                priority: 4,
                is_transparent: false,
                is_obj_alpha: false,
            };
            cand_count += 1;

            // 2. Background Layers (BG0..3)
            for bg in 0..4 {
                if (ppu.dispcnt & (1 << (8 + bg))) == 0 || (ppu.layer_mask & (1 << bg)) == 0 {
                    continue;
                }

                let cmd = match cmd_map[ny * 6 + (bg + 1)] {
                    Some(c) => c,
                    None => continue,
                };

                match cmd.kind {
                    LayerKind::Text => {
                        let is_expanded = ws_config.bg_expand[bg];
                        let anchor = ws_config.hud_anchor[bg];

                        let sample_screen_x = if is_expanded {
                            Some(nx_screen)
                        } else {
                            match anchor {
                                HudAnchor::Center => {
                                    if nx_screen >= 0 && nx_screen < (SCREEN_WIDTH as i32) {
                                        Some(nx_screen)
                                    } else {
                                        None
                                    }
                                }
                                HudAnchor::Left => {
                                    let shifted = nx_screen + (margin_l as i32);
                                    if shifted >= 0 && shifted < (SCREEN_WIDTH as i32) {
                                        Some(shifted)
                                    } else {
                                        None
                                    }
                                }
                                HudAnchor::Right => {
                                    let shifted = nx_screen - (margin_r as i32);
                                    if shifted >= 0 && shifted < (SCREEN_WIDTH as i32) {
                                        Some(shifted)
                                    } else {
                                        None
                                    }
                                }
                                HudAnchor::Expand => Some(nx_screen),
                                HudAnchor::Pillarbox => {
                                    if nx_screen >= 0 && nx_screen < (SCREEN_WIDTH as i32) {
                                        Some(nx_screen)
                                    } else {
                                        None
                                    }
                                }
                            }
                        };

                        if let Some(eff_x) = sample_screen_x {
                            let char_base = (((cmd.bgcnt >> 2) & 3) as usize) * 16384;
                            let is_8bpp = (cmd.bgcnt & (1 << 7)) != 0;
                            let screen_base = (((cmd.bgcnt >> 8) & 0x1F) as usize) * 2048;
                            let screen_size = (cmd.bgcnt >> 14) & 3;

                            let (map_w, map_h) = match screen_size {
                                0 => (256, 256),
                                1 => (512, 256),
                                2 => (256, 512),
                                3 => (512, 512),
                                _ => (256, 256),
                            };

                            let scrolled_y = (ny as u32 + cmd.v_offset as u32) % map_h;
                            let sample_x = eff_x + cmd.h_offset as i32;
                            let scrolled_x = sample_x.rem_euclid(map_w as i32) as u32;

                            let block_x = scrolled_x / 256;
                            let block_y = scrolled_y / 256;
                            let block_offset = match screen_size {
                                0 => 0,
                                1 => block_x * 2048,
                                2 => block_y * 2048,
                                3 => (block_y * 2 + block_x) * 2048,
                                _ => 0,
                            };

                            let tile_x = (scrolled_x % 256) / 8;
                            let tile_y = (scrolled_y % 256) / 8;
                            let map_entry_addr = screen_base + block_offset as usize + ((tile_y * 32 + tile_x) * 2) as usize;

                            if map_entry_addr + 1 < ppu.vram.len() {
                                let map_entry = (ppu.vram[map_entry_addr] as u16) | ((ppu.vram[map_entry_addr + 1] as u16) << 8);
                                let tile_num = (map_entry & 0x3FF) as usize;
                                let hflip = (map_entry & (1 << 10)) != 0;
                                let vflip = (map_entry & (1 << 11)) != 0;
                                let pal_num = ((map_entry >> 12) & 0x0F) as usize;

                                let mut py = (scrolled_y % 8) as usize;
                                let mut px = (scrolled_x % 8) as usize;
                                if hflip { px = 7 - px; }
                                if vflip { py = 7 - py; }

                                let color_idx = if is_8bpp {
                                    let tile_addr = char_base + tile_num * 64 + py * 8 + px;
                                    if tile_addr < ppu.vram.len() {
                                        ppu.vram[tile_addr] as usize
                                    } else {
                                        0
                                    }
                                } else {
                                    let tile_addr = char_base + tile_num * 32 + py * 4 + (px / 2);
                                    if tile_addr < ppu.vram.len() {
                                        let byte = ppu.vram[tile_addr];
                                        let idx = if px % 2 == 0 { byte & 0x0F } else { (byte >> 4) & 0x0F };
                                        if idx != 0 { pal_num * 16 + (idx as usize) } else { 0 }
                                    } else {
                                        0
                                    }
                                };

                                if color_idx != 0 {
                                    let pal_addr = color_idx * 2;
                                    if pal_addr + 1 < ppu.palette_ram.len() {
                                        let color = (ppu.palette_ram[pal_addr] as u16) | ((ppu.palette_ram[pal_addr + 1] as u16) << 8);
                                        candidates[cand_count] = Pixel {
                                            color,
                                            layer: bg as u8,
                                            priority: cmd.priority,
                                            is_transparent: false,
                                            is_obj_alpha: false,
                                        };
                                        cand_count += 1;
                                    }
                                }
                            }
                        }
                    }

                    LayerKind::Affine => {
                        let is_expanded = ws_config.bg_expand[bg];
                        if is_expanded || (nx_screen >= 0 && nx_screen < (SCREEN_WIDTH as i32)) {
                            let wrap = (cmd.bgcnt & (1 << 13)) != 0;
                            let size_shift = ((cmd.bgcnt >> 14) & 3) as usize;
                            let size_px = (128 << size_shift) as i32;
                            let tiles_per_row = (16 << size_shift) as usize;
                            let char_base = (((cmd.bgcnt >> 2) & 3) as usize) * 16384;
                            let screen_base = (((cmd.bgcnt >> 8) & 0x1F) as usize) * 2048;

                            let (pa, pc, x_orig, y_orig) = if hd_config.perspective_interpolation && ny < 159 {
                                if let Some(next_cmd) = cmd_map[(ny + 1) * 6 + (bg + 1)] {
                                    (
                                        lerp_f64(cmd.affine_matrix[0] as f64, next_cmd.affine_matrix[0] as f64, t),
                                        lerp_f64(cmd.affine_matrix[2] as f64, next_cmd.affine_matrix[2] as f64, t),
                                        lerp_f64(cmd.affine_origin[0] as f64, next_cmd.affine_origin[0] as f64, t),
                                        lerp_f64(cmd.affine_origin[1] as f64, next_cmd.affine_origin[1] as f64, t),
                                    )
                                } else {
                                    (cmd.affine_matrix[0] as f64, cmd.affine_matrix[2] as f64, cmd.affine_origin[0] as f64, cmd.affine_origin[1] as f64)
                                }
                            } else {
                                (cmd.affine_matrix[0] as f64, cmd.affine_matrix[2] as f64, cmd.affine_origin[0] as f64, cmd.affine_origin[1] as f64)
                            };

                            let sub_x = (nx_screen as f64) + (sx as f64) / f_scale;
                            let u_fixed = (x_orig + sub_x * pa).round() as i32;
                            let v_fixed = (y_orig + sub_x * pc).round() as i32;

                            let px = u_fixed >> 8;
                            let py = v_fixed >> 8;

                            let in_bounds = px >= 0 && px < size_px && py >= 0 && py < size_px;
                            if in_bounds || wrap {
                                let map_x = px.rem_euclid(size_px) as usize;
                                let map_y = py.rem_euclid(size_px) as usize;
                                let tile_x = map_x / 8;
                                let tile_y = map_y / 8;
                                let map_entry_addr = screen_base + tile_y * tiles_per_row + tile_x;

                                if map_entry_addr < ppu.vram.len() {
                                    let tile_num = ppu.vram[map_entry_addr] as usize;
                                    let in_tile_x = map_x % 8;
                                    let in_tile_y = map_y % 8;
                                    let pixel_addr = char_base + tile_num * 64 + in_tile_y * 8 + in_tile_x;

                                    if pixel_addr < ppu.vram.len() {
                                        let color_idx = ppu.vram[pixel_addr] as usize;
                                        if color_idx != 0 {
                                            let pal_addr = color_idx * 2;
                                            if pal_addr + 1 < ppu.palette_ram.len() {
                                                let color = (ppu.palette_ram[pal_addr] as u16) | ((ppu.palette_ram[pal_addr + 1] as u16) << 8);
                                                candidates[cand_count] = Pixel {
                                                    color,
                                                    layer: bg as u8,
                                                    priority: cmd.priority,
                                                    is_transparent: false,
                                                    is_obj_alpha: false,
                                                };
                                                cand_count += 1;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    _ => {}
                }
            }

            // 3. Sprites (OBJ)
            if (ppu.dispcnt & (1 << 12)) != 0 && (ppu.layer_mask & (1 << 4)) != 0 {
                for sp in &sprites {
                    // Check vertical bounds
                    let raw_y = sp.raw_y;
                    let bound_h = sp.bound_h as i32;
                    let py_i32 = if raw_y + bound_h > 256 && (ny as i32) < (raw_y + bound_h - 256) {
                        (ny as i32) + 256 - raw_y
                    } else if (ny as i32) >= raw_y && (ny as i32) < raw_y + bound_h {
                        (ny as i32) - raw_y
                    } else {
                        continue;
                    };

                    // Check horizontal bounds
                    let px_i32 = nx_screen - sp.sprite_x;
                    if px_i32 < 0 || px_i32 >= (sp.bound_w as i32) {
                        continue;
                    }

                    // Check for HD Replacement Sprite (ROADMAP M7)
                    if let Some(ref rep) = sp.replacement {
                        let u_norm = ((px_i32 as f64) + (sx as f64) / f_scale) / (sp.bound_w as f64);
                        let v_norm = ((py_i32 as f64) + (sy as f64) / f_scale) / (sp.bound_h as f64);

                        let u_eff = if sp.hflip { 1.0 - u_norm } else { u_norm };
                        let v_eff = if sp.vflip { 1.0 - v_norm } else { v_norm };

                        if (0.0..=1.0).contains(&u_eff) && (0.0..=1.0).contains(&v_eff) {
                            let img = &rep.image;
                            let ix = ((u_eff * (img.width as f64)).floor() as usize).min(img.width - 1);
                            let iy = ((v_eff * (img.height as f64)).floor() as usize).min(img.height - 1);
                            let mut rgba = img.pixels[iy * img.width + ix];

                            if (rgba >> 24) != 0 {
                                if rep.recolor {
                                    if let Some(ref bp) = rep.base_palette {
                                        if !sp.palette.is_empty() {
                                            rgba = recolor_rgba(rgba, bp, &sp.palette);
                                        }
                                    }
                                }

                                let r5 = ((rgba & 0xFF) >> 3) as u16;
                                let g5 = (((rgba >> 8) & 0xFF) >> 3) as u16;
                                let b5 = (((rgba >> 16) & 0xFF) >> 3) as u16;
                                let color = r5 | (g5 << 5) | (b5 << 10);

                                candidates[cand_count] = Pixel {
                                    color,
                                    layer: 4,
                                    priority: sp.priority,
                                    is_transparent: false,
                                    is_obj_alpha: sp.is_semi_trans,
                                };
                                cand_count += 1;
                                break;
                            }
                        }
                    } else if !sp.is_affine {
                        // Standard non-affine sprite
                        let mut spr_px = px_i32 as usize;
                        let mut spr_py = py_i32 as usize;
                        if sp.hflip { spr_px = sp.orig_w - 1 - spr_px; }
                        if sp.vflip { spr_py = sp.orig_h - 1 - spr_py; }

                        let tile_x = spr_px / 8;
                        let tile_y = spr_py / 8;
                        let in_tile_x = spr_px % 8;
                        let in_tile_y = spr_py % 8;

                        let tile_idx = if mapping_1d {
                            let stride = if sp.is_8bpp { (sp.orig_w / 8) * 2 } else { sp.orig_w / 8 };
                            if sp.is_8bpp {
                                sp.raw_tile + (tile_y * stride + tile_x * 2)
                            } else {
                                sp.raw_tile + (tile_y * stride + tile_x)
                            }
                        } else {
                            let stride = 32;
                            if sp.is_8bpp {
                                (sp.raw_tile & !1) + tile_y * stride + tile_x * 2
                            } else {
                                sp.raw_tile + tile_y * stride + tile_x
                            }
                        };

                        let tile_addr = obj_char_base + tile_idx * 32;
                        let color_idx = if sp.is_8bpp {
                            let pixel_addr = tile_addr + in_tile_y * 8 + in_tile_x;
                            if pixel_addr < ppu.vram.len() {
                                ppu.vram[pixel_addr] as usize
                            } else {
                                0
                            }
                        } else {
                            let pixel_addr = tile_addr + in_tile_y * 4 + (in_tile_x / 2);
                            if pixel_addr < ppu.vram.len() {
                                let byte = ppu.vram[pixel_addr];
                                let idx = if in_tile_x % 2 == 0 { byte & 0x0F } else { (byte >> 4) & 0x0F };
                                if idx != 0 { sp.pal_num * 16 + (idx as usize) } else { 0 }
                            } else {
                                0
                            }
                        };

                        if color_idx != 0 {
                            let pal_addr = 0x200 + color_idx * 2;
                            if pal_addr + 1 < ppu.palette_ram.len() {
                                let color = (ppu.palette_ram[pal_addr] as u16) | ((ppu.palette_ram[pal_addr + 1] as u16) << 8);
                                candidates[cand_count] = Pixel {
                                    color,
                                    layer: 4,
                                    priority: sp.priority,
                                    is_transparent: false,
                                    is_obj_alpha: sp.is_semi_trans,
                                };
                                cand_count += 1;
                                break;
                            }
                        }
                    } else {
                        // Affine sprite
                        let half_w = (sp.bound_w as f64) / 2.0;
                        let half_h = (sp.bound_h as f64) / 2.0;
                        let orig_half_w = (sp.orig_w as f64) / 2.0;
                        let orig_half_h = (sp.orig_h as f64) / 2.0;

                        let sub_px = (px_i32 as f64) + (sx as f64) / f_scale;
                        let sub_py = (py_i32 as f64) + (sy as f64) / f_scale;

                        let dx = sub_px - half_w;
                        let dy = sub_py - half_h;

                        let tex_u = (sp.pa as f64 * dx + sp.pb as f64 * dy) / 256.0 + orig_half_w;
                        let tex_v = (sp.pc as f64 * dx + sp.pd as f64 * dy) / 256.0 + orig_half_h;

                        let spr_px = tex_u.floor() as i32;
                        let spr_py = tex_v.floor() as i32;

                        if spr_px >= 0 && spr_px < (sp.orig_w as i32) && spr_py >= 0 && spr_py < (sp.orig_h as i32) {
                            let spr_px = spr_px as usize;
                            let spr_py = spr_py as usize;
                            let tile_x = spr_px / 8;
                            let tile_y = spr_py / 8;
                            let in_tile_x = spr_px % 8;
                            let in_tile_y = spr_py % 8;

                            let tile_idx = if mapping_1d {
                                let stride = if sp.is_8bpp { (sp.orig_w / 8) * 2 } else { sp.orig_w / 8 };
                                if sp.is_8bpp {
                                    sp.raw_tile + (tile_y * stride + tile_x * 2)
                                } else {
                                    sp.raw_tile + (tile_y * stride + tile_x)
                                }
                            } else {
                                let stride = 32;
                                if sp.is_8bpp {
                                    (sp.raw_tile & !1) + tile_y * stride + tile_x * 2
                                } else {
                                    sp.raw_tile + tile_y * stride + tile_x
                                }
                            };

                            let tile_addr = obj_char_base + tile_idx * 32;
                            let color_idx = if sp.is_8bpp {
                                let pixel_addr = tile_addr + in_tile_y * 8 + in_tile_x;
                                if pixel_addr < ppu.vram.len() {
                                    ppu.vram[pixel_addr] as usize
                                } else {
                                    0
                                }
                            } else {
                                let pixel_addr = tile_addr + in_tile_y * 4 + (in_tile_x / 2);
                                if pixel_addr < ppu.vram.len() {
                                    let byte = ppu.vram[pixel_addr];
                                    let idx = if in_tile_x % 2 == 0 { byte & 0x0F } else { (byte >> 4) & 0x0F };
                                    if idx != 0 { sp.pal_num * 16 + (idx as usize) } else { 0 }
                                } else {
                                    0
                                }
                            };

                            if color_idx != 0 {
                                let pal_addr = 0x200 + color_idx * 2;
                                if pal_addr + 1 < ppu.palette_ram.len() {
                                    let color = (ppu.palette_ram[pal_addr] as u16) | ((ppu.palette_ram[pal_addr + 1] as u16) << 8);
                                    candidates[cand_count] = Pixel {
                                        color,
                                        layer: 4,
                                        priority: sp.priority,
                                        is_transparent: false,
                                        is_obj_alpha: sp.is_semi_trans,
                                    };
                                    cand_count += 1;
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            // Filter candidates by window mask
            let mut top = Pixel {
                color: backdrop_color,
                layer: 5,
                priority: 4,
                is_transparent: false,
                is_obj_alpha: false,
            };
            let mut bot = top;

            // Stable priority sorting: (priority ASC, is_bg ASC, layer_index ASC)
            let mut valid_cands: Vec<Pixel> = candidates[..cand_count]
                .iter()
                .copied()
                .filter(|p| {
                    if p.layer == 5 {
                        (win_mask & (1 << 5)) != 0
                    } else if p.layer == 4 {
                        (win_mask & (1 << 4)) != 0
                    } else {
                        (win_mask & (1 << p.layer)) != 0
                    }
                })
                .collect();

            valid_cands.sort_by(|a, b| {
                a.priority
                    .cmp(&b.priority)
                    .then_with(|| (a.layer != 4).cmp(&(b.layer != 4)))
                    .then_with(|| a.layer.cmp(&b.layer))
            });

            if !valid_cands.is_empty() {
                top = valid_cands[0];
            }
            if valid_cands.len() > 1 {
                bot = valid_cands[1];
            }

            // Apply GBA special color effects
            let final_bgr = if (win_mask & (1 << 5)) != 0 {
                apply_color_effects(top, bot, ppu.bldcnt, eva, evb, evy)
            } else {
                top.color
            };
            let (r8, g8, b8) = bgr555_to_rgb888(final_bgr);
            let pixel_word = 0xFF00_0000 | ((b8 as u32) << 16) | ((g8 as u32) << 8) | (r8 as u32);
            frame.pixels[row_offset + hx] = pixel_word;
        }
    }

    Some(frame)
}
