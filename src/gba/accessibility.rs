//! Accessibility pack (ROADMAP M10).
//!
//! Front-end independent pieces shared by the desktop and Android builds:
//!
//! - **Colorblind filters**: Daltonization (Fidaner et al.) for protanopia,
//!   deuteranopia and tritanopia, plus grayscale and a high-contrast mode.
//!   Colors are simulated in LMS space (Viénot/Brettel matrices), and the
//!   information a viewer would lose is shifted into channels they can see.
//! - **Slow motion**: a speed factor for frame pacing. Audio is kept flowing
//!   at the right rate by `apu::slowmo_stretch` (pitch-preserving) or plain
//!   resampling ("tape" mode, lower pitch), so it never underruns.
//! - **Toggle instead of hold** ("sticky buttons") for any GBA button.
//! - **One-handed layouts**: a preset choice for the desktop keyboard and the
//!   Android touch overlay (the front-ends own the actual key maps/geometry).
//! - **Interface scale**.
//! - **Per-game profiles**: `AccessibilityStore` holds a global default plus
//!   overrides keyed by game code (or title for GB games), serializable to
//!   JSON so both front-ends persist it the same way.
//!
//! None of this touches emulation state, so determinism, replays and save
//! states are unaffected.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::keypad::Key;

// ---------------------------------------------------------------------------
// Colorblind filters
// ---------------------------------------------------------------------------

/// Colorblind correction modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorblindMode {
    #[default]
    None,
    /// Red-blind / red-weak.
    Protanopia,
    /// Green-blind / green-weak (the most common).
    Deuteranopia,
    /// Blue-blind / blue-weak.
    Tritanopia,
    /// Complete color blindness: luminance only.
    Achromatopsia,
    /// Stronger contrast and saturation for low vision.
    HighContrast,
}

impl ColorblindMode {
    pub const ALL: [ColorblindMode; 6] = [
        ColorblindMode::None,
        ColorblindMode::Protanopia,
        ColorblindMode::Deuteranopia,
        ColorblindMode::Tritanopia,
        ColorblindMode::Achromatopsia,
        ColorblindMode::HighContrast,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::None => "Off",
            Self::Protanopia => "Protanopia (red-weak)",
            Self::Deuteranopia => "Deuteranopia (green-weak)",
            Self::Tritanopia => "Tritanopia (blue-weak)",
            Self::Achromatopsia => "Grayscale",
            Self::HighContrast => "High contrast",
        }
    }

    pub fn short_name(&self) -> &'static str {
        match self {
            Self::None => "Off",
            Self::Protanopia => "Protan",
            Self::Deuteranopia => "Deutan",
            Self::Tritanopia => "Tritan",
            Self::Achromatopsia => "Gray",
            Self::HighContrast => "Contrast",
        }
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|&m| m == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "protanopia" | "protan" => Self::Protanopia,
            "deuteranopia" | "deutan" => Self::Deuteranopia,
            "tritanopia" | "tritan" => Self::Tritanopia,
            "achromatopsia" | "grayscale" | "gray" | "mono" => Self::Achromatopsia,
            "high_contrast" | "highcontrast" | "contrast" => Self::HighContrast,
            _ => Self::None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Protanopia => "protanopia",
            Self::Deuteranopia => "deuteranopia",
            Self::Tritanopia => "tritanopia",
            Self::Achromatopsia => "achromatopsia",
            Self::HighContrast => "high_contrast",
        }
    }
}

type M3 = [[f32; 3]; 3];

#[inline]
fn mul(m: &M3, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// RGB -> LMS cone response (Viénot, Brettel & Mollon 1999).
const RGB_TO_LMS: M3 = [
    [17.8824, 43.5161, 4.11935],
    [3.45565, 27.1554, 3.86714],
    [0.0299566, 0.184309, 1.46709],
];
const LMS_TO_RGB: M3 = [
    [0.080_944_45, -0.130_504_4, 0.116_721_07],
    [-0.010_248_534, 0.054_019_33, -0.113_614_71],
    [-0.000_365_297, -0.004_121_615, 0.693_511_4],
];
/// Dichromat projections in LMS space: the missing cone response is
/// reconstructed from the other two.
const SIM_PROTAN: M3 = [[0.0, 2.02344, -2.52581], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
const SIM_DEUTAN: M3 = [[1.0, 0.0, 0.0], [0.494207, 0.0, 1.24827], [0.0, 0.0, 1.0]];
const SIM_TRITAN: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [-0.395913, 0.801109, 0.0]];

/// What a dichromat would see (RGB in 0..=255 floats). `None` for modes that
/// aren't a cone deficiency.
pub fn simulate_dichromacy(rgb: [f32; 3], mode: ColorblindMode) -> Option<[f32; 3]> {
    let sim = match mode {
        ColorblindMode::Protanopia => &SIM_PROTAN,
        ColorblindMode::Deuteranopia => &SIM_DEUTAN,
        ColorblindMode::Tritanopia => &SIM_TRITAN,
        _ => return None,
    };
    Some(mul(&LMS_TO_RGB, mul(sim, mul(&RGB_TO_LMS, rgb))))
}

/// Apply the filter to one color. `intensity` (0..=1) mixes between the
/// original and the fully corrected color.
pub fn apply_colorblind_filter(r: u8, g: u8, b: u8, mode: ColorblindMode, intensity: f32) -> (u8, u8, u8) {
    let k = intensity.clamp(0.0, 1.0);
    if mode == ColorblindMode::None || k == 0.0 {
        return (r, g, b);
    }
    let src = [r as f32, g as f32, b as f32];
    let out = match mode {
        ColorblindMode::None => src,
        ColorblindMode::Protanopia | ColorblindMode::Deuteranopia | ColorblindMode::Tritanopia => {
            let sim = simulate_dichromacy(src, mode).unwrap_or(src);
            let err = [src[0] - sim[0], src[1] - sim[1], src[2] - sim[2]];
            // Fidaner error redistribution: move the lost signal into the
            // channels the viewer still distinguishes.
            let shift = match mode {
                ColorblindMode::Tritanopia => [0.7 * err[2], 0.7 * err[2], 0.0],
                _ => [0.0, 0.7 * err[0] + err[1], 0.7 * err[0] + err[2]],
            };
            [src[0] + shift[0], src[1] + shift[1], src[2] + shift[2]]
        }
        ColorblindMode::Achromatopsia => {
            let y = 0.2126 * src[0] + 0.7152 * src[1] + 0.0722 * src[2];
            [y, y, y]
        }
        ColorblindMode::HighContrast => {
            // Stretch contrast around mid-gray, then push saturation away
            // from the pixel's own luminance so hues separate more.
            let c = src.map(|v| 128.0 + (v - 128.0) * 1.4);
            let y = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            c.map(|v| y + (v - y) * 1.35)
        }
    };
    let mix = |s: f32, o: f32| (s + (o - s) * k).round().clamp(0.0, 255.0) as u8;
    (mix(src[0], out[0]), mix(src[1], out[1]), mix(src[2], out[2]))
}

/// Filter a framebuffer in place. Pixels are 0xAABBGGRR (the cores' format);
/// alpha is preserved. Runs of identical colors (most of any GBA frame) reuse
/// the previous result.
pub fn apply_colorblind_filter_to_pixels(pixels: &mut [u32], mode: ColorblindMode, intensity: f32) {
    if mode == ColorblindMode::None || intensity <= 0.0 {
        return;
    }
    let mut last_in = !pixels.first().copied().unwrap_or(0);
    let mut last_out = 0u32;
    for px in pixels.iter_mut() {
        let rgb = *px & 0x00FF_FFFF;
        if rgb != last_in {
            let (r, g, b) = apply_colorblind_filter(rgb as u8, (rgb >> 8) as u8, (rgb >> 16) as u8, mode, intensity);
            last_in = rgb;
            last_out = r as u32 | (g as u32) << 8 | (b as u32) << 16;
        }
        *px = (*px & 0xFF00_0000) | last_out;
    }
}

// ---------------------------------------------------------------------------
// Slow motion
// ---------------------------------------------------------------------------

/// How audio behaves in slow motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlowMotionAudio {
    /// Time-stretched: same pitch, longer notes.
    #[default]
    PitchPreserved,
    /// Played slower like a tape: pitch drops with speed.
    Tape,
    /// Silent while slowed down.
    Mute,
}

impl SlowMotionAudio {
    pub const ALL: [SlowMotionAudio; 3] = [Self::PitchPreserved, Self::Tape, Self::Mute];

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::PitchPreserved => "Stretch (keep pitch)",
            Self::Tape => "Tape (lower pitch)",
            Self::Mute => "Mute",
        }
    }
}

/// Slowest supported speed.
pub const MIN_SLOW_MOTION: f32 = 0.10;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SlowMotionConfig {
    pub enabled: bool,
    /// Emulation speed while enabled (0.10..=1.0).
    pub speed_factor: f32,
    pub audio: SlowMotionAudio,
}

impl Default for SlowMotionConfig {
    fn default() -> Self {
        Self { enabled: false, speed_factor: 0.5, audio: SlowMotionAudio::PitchPreserved }
    }
}

impl SlowMotionConfig {
    /// Speed multiplier to apply to frame pacing (1.0 when off).
    pub fn effective_multiplier(&self) -> f32 {
        if self.enabled {
            self.speed_factor.clamp(MIN_SLOW_MOTION, 1.0)
        } else {
            1.0
        }
    }

    /// Android-style cycling: off -> 75% -> 50% -> 25% -> off.
    pub fn cycle(&mut self) {
        let (enabled, speed) = match (self.enabled, (self.speed_factor * 100.0).round() as u32) {
            (false, _) => (true, 0.75),
            (true, 75) => (true, 0.5),
            (true, 50) => (true, 0.25),
            _ => (false, self.speed_factor),
        };
        self.enabled = enabled;
        self.speed_factor = speed;
    }
}

// ---------------------------------------------------------------------------
// Toggle instead of hold
// ---------------------------------------------------------------------------

/// Every GBA button, in `Key` bit order.
pub const ALL_KEYS: [Key; 10] = [
    Key::A, Key::B, Key::Select, Key::Start, Key::Right,
    Key::Left, Key::Up, Key::Down, Key::R, Key::L,
];

pub fn key_name(key: Key) -> &'static str {
    match key {
        Key::A => "A",
        Key::B => "B",
        Key::Select => "Select",
        Key::Start => "Start",
        Key::Right => "Right",
        Key::Left => "Left",
        Key::Up => "Up",
        Key::Down => "Down",
        Key::R => "R",
        Key::L => "L",
    }
}

/// Toggle-instead-of-hold. Only `toggle_enabled_mask` is a setting; the
/// latch/edge state is runtime-only and never saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StickyButtonsConfig {
    /// Bit `1 << key` set = that button toggles.
    pub toggle_enabled_mask: u16,
    #[serde(skip)]
    pub latched_states: u16,
    #[serde(skip)]
    pub prev_physical: u16,
}

impl StickyButtonsConfig {
    pub fn set_key_toggle_enabled(&mut self, key: Key, enabled: bool) {
        let bit = 1 << (key as u16);
        if enabled {
            self.toggle_enabled_mask |= bit;
        } else {
            self.toggle_enabled_mask &= !bit;
            self.latched_states &= !bit;
        }
    }

    pub fn is_key_toggle_enabled(&self, key: Key) -> bool {
        self.toggle_enabled_mask & (1 << (key as u16)) != 0
    }

    pub fn is_key_latched(&self, key: Key) -> bool {
        self.latched_states & (1 << (key as u16)) != 0
    }

    pub fn clear_latches(&mut self) {
        self.latched_states = 0;
    }

    /// Physical state in, state the game should see out. Toggle keys flip
    /// their latch on each press edge; other keys pass through.
    pub fn process_key(&mut self, key: Key, physical_pressed: bool) -> bool {
        let bit = 1 << (key as u16);
        let was_down = self.prev_physical & bit != 0;
        if physical_pressed {
            self.prev_physical |= bit;
        } else {
            self.prev_physical &= !bit;
        }
        if self.toggle_enabled_mask & bit == 0 {
            return physical_pressed;
        }
        if physical_pressed && !was_down {
            self.latched_states ^= bit;
        }
        self.latched_states & bit != 0
    }
}

// ---------------------------------------------------------------------------
// One-handed layouts
// ---------------------------------------------------------------------------

/// Which hand plays. Used for both the desktop keyboard preset and the
/// Android touch overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Handedness {
    /// Normal two-handed layout.
    #[default]
    Standard,
    LeftHand,
    RightHand,
}

impl Handedness {
    pub const ALL: [Handedness; 3] = [Self::Standard, Self::LeftHand, Self::RightHand];

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Standard => "Two hands",
            Self::LeftHand => "Left hand only",
            Self::RightHand => "Right hand only",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Standard => Self::LeftHand,
            Self::LeftHand => Self::RightHand,
            Self::RightHand => Self::Standard,
        }
    }
}

// ---------------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------------

/// Interface scale limits.
pub const UI_SCALE_MIN: f32 = 0.75;
pub const UI_SCALE_MAX: f32 = 2.5;

/// All accessibility settings for one game (or the global default).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccessibilityProfile {
    pub colorblind_mode: ColorblindMode,
    pub colorblind_intensity: f32,
    pub slow_motion: SlowMotionConfig,
    pub sticky_buttons: StickyButtonsConfig,
    /// Desktop keyboard layout preset.
    pub one_handed_desktop: Handedness,
    /// Android touch overlay layout.
    pub one_handed_touch: Handedness,
    /// Interface scale (egui zoom factor), 0.75..=2.5.
    pub ui_scale: f32,
}

impl Default for AccessibilityProfile {
    fn default() -> Self {
        Self {
            colorblind_mode: ColorblindMode::None,
            colorblind_intensity: 1.0,
            slow_motion: SlowMotionConfig::default(),
            sticky_buttons: StickyButtonsConfig::default(),
            one_handed_desktop: Handedness::Standard,
            one_handed_touch: Handedness::Standard,
            ui_scale: 1.0,
        }
    }
}

impl AccessibilityProfile {
    pub fn ui_scale_clamped(&self) -> f32 {
        if self.ui_scale.is_finite() {
            self.ui_scale.clamp(UI_SCALE_MIN, UI_SCALE_MAX)
        } else {
            1.0
        }
    }

    /// Settings equal, ignoring runtime latch state.
    pub fn same_settings(&self, other: &Self) -> bool {
        let mut a = self.clone();
        let mut b = other.clone();
        for p in [&mut a, &mut b] {
            p.sticky_buttons.latched_states = 0;
            p.sticky_buttons.prev_physical = 0;
        }
        a == b
    }
}

/// The persisted part: global default plus per-game overrides.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccessibilityStore {
    pub default_profile: AccessibilityProfile,
    pub per_game: HashMap<String, AccessibilityProfile>,
}

impl AccessibilityStore {
    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }
}

/// Key a game is stored under: its 4-letter game code when it has one
/// (upper-case), otherwise its header title.
pub fn game_key(game_code: &str, title: &str) -> String {
    let code = game_code.trim().trim_matches('\0');
    let title = title.trim().trim_matches('\0');
    if !code.is_empty() {
        code.to_ascii_uppercase()
    } else if !title.is_empty() {
        title.to_string()
    } else {
        "DEFAULT".to_string()
    }
}

/// Active settings plus the store they come from.
///
/// Edits go to `active`; call `commit()` afterwards. With a game loaded the
/// edit is saved as that game's override, so every setting is per game
/// automatically; with no game loaded it changes the global default.
#[derive(Debug, Clone, Default)]
pub struct AccessibilityManager {
    pub active: AccessibilityProfile,
    pub store: AccessibilityStore,
    pub current_game_id: Option<String>,
}

impl AccessibilityManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_store(store: AccessibilityStore) -> Self {
        let active = store.default_profile.clone();
        Self { active, store, current_game_id: None }
    }

    /// Switch to a game's profile (or the default if it has none).
    pub fn load_for_game(&mut self, game_code: &str, title: &str) {
        let id = game_key(game_code, title);
        self.active = self.store.per_game.get(&id).cloned().unwrap_or_else(|| self.store.default_profile.clone());
        self.current_game_id = Some(id);
    }

    /// No game loaded any more: back to the global default.
    pub fn unload_game(&mut self) {
        self.current_game_id = None;
        self.active = self.store.default_profile.clone();
    }

    /// Whether the current game has its own override.
    pub fn has_game_override(&self) -> bool {
        self.current_game_id.as_ref().is_some_and(|id| self.store.per_game.contains_key(id))
    }

    /// Save `active` where it belongs (see type docs). Returns true if the
    /// store changed (i.e. the config needs writing).
    pub fn commit(&mut self) -> bool {
        let mut saved = self.active.clone();
        saved.sticky_buttons.latched_states = 0;
        saved.sticky_buttons.prev_physical = 0;
        let slot = match &self.current_game_id {
            Some(id) => {
                match self.store.per_game.get(id) {
                    Some(p) if p.same_settings(&saved) => return false,
                    // Matching the default needs no override of its own (and
                    // "reset to default" must not recreate one).
                    None if self.store.default_profile.same_settings(&saved) => return false,
                    _ => {}
                }
                self.store.per_game.insert(id.clone(), saved);
                return true;
            }
            None => &mut self.store.default_profile,
        };
        if slot.same_settings(&saved) {
            return false;
        }
        *slot = saved;
        true
    }

    /// Drop the current game's override and fall back to the default.
    pub fn reset_game_to_default(&mut self) {
        if let Some(id) = &self.current_game_id {
            self.store.per_game.remove(id);
        }
        self.active = self.store.default_profile.clone();
    }

    /// Make the current settings the default for every game without an
    /// override of its own.
    pub fn set_as_global_default(&mut self) {
        let mut p = self.active.clone();
        p.sticky_buttons.latched_states = 0;
        p.sticky_buttons.prev_physical = 0;
        self.store.default_profile = p;
    }

    pub fn filter_framebuffer(&self, pixels: &mut [u32]) {
        apply_colorblind_filter_to_pixels(pixels, self.active.colorblind_mode, self.active.colorblind_intensity);
    }

    pub fn process_key(&mut self, key: Key, physical_pressed: bool) -> bool {
        self.active.sticky_buttons.process_key(key, physical_pressed)
    }

    /// Run a full 10-bit button mask (bit n = `Key` n) through sticky buttons.
    pub fn process_mask(&mut self, physical: u16) -> u16 {
        let mut out = 0;
        for key in ALL_KEYS {
            if self.process_key(key, physical & (1 << key as u16) != 0) {
                out |= 1 << key as u16;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }

    #[test]
    fn neutral_colors_are_untouched_by_daltonization() {
        for v in [0u8, 64, 128, 200, 255] {
            for m in [ColorblindMode::Protanopia, ColorblindMode::Deuteranopia, ColorblindMode::Tritanopia] {
                let (r, g, b) = apply_colorblind_filter(v, v, v, m, 1.0);
                assert!((r as i32 - v as i32).abs() <= 2 && (g as i32 - v as i32).abs() <= 2 && (b as i32 - v as i32).abs() <= 2, "{m:?} {v}: {r} {g} {b}");
            }
        }
    }

    #[test]
    fn daltonization_separates_red_green_confusion_for_deutans() {
        // Red and green that a deuteranope sees as nearly the same color.
        let red = [200.0, 80.0, 60.0];
        let green = [110.0, 130.0, 60.0];
        let seen = |c: [f32; 3]| simulate_dichromacy(c, ColorblindMode::Deuteranopia).unwrap();
        let before = dist(seen(red), seen(green));
        let fix = |c: [f32; 3]| {
            let (r, g, b) = apply_colorblind_filter(c[0] as u8, c[1] as u8, c[2] as u8, ColorblindMode::Deuteranopia, 1.0);
            [r as f32, g as f32, b as f32]
        };
        let after = dist(seen(fix(red)), seen(fix(green)));
        assert!(after > before * 1.3, "before {before} after {after}");
    }

    #[test]
    fn sticky_toggle_latches_on_press_edges_only() {
        let mut s = StickyButtonsConfig::default();
        s.set_key_toggle_enabled(Key::B, true);
        assert!(s.process_key(Key::B, true));
        assert!(s.process_key(Key::B, true), "holding keeps it on");
        assert!(s.process_key(Key::B, false), "release keeps latch");
        assert!(!s.process_key(Key::B, true), "second press unlatches");
        assert!(!s.process_key(Key::B, false));
        assert!(s.process_key(Key::A, true) && !s.process_key(Key::A, false), "A still normal");
    }

    #[test]
    fn edits_are_saved_per_game_and_default_is_separate() {
        let mut m = AccessibilityManager::new();
        m.load_for_game("bpee", "POKEMON EMER");
        assert_eq!(m.current_game_id.as_deref(), Some("BPEE"));
        m.active.colorblind_mode = ColorblindMode::Deuteranopia;
        assert!(m.commit());
        assert!(!m.commit(), "no change, no write");
        m.load_for_game("AMKE", "");
        assert_eq!(m.active.colorblind_mode, ColorblindMode::None);
        m.load_for_game("BPEE", "");
        assert_eq!(m.active.colorblind_mode, ColorblindMode::Deuteranopia);
        m.reset_game_to_default();
        assert_eq!(m.active.colorblind_mode, ColorblindMode::None);
        assert!(!m.commit(), "reset must not recreate an override");
        assert!(!m.has_game_override());

        // With no game loaded, edits change the global default...
        m.unload_game();
        m.active.slow_motion.enabled = true;
        assert!(m.commit());
        // ...which games without an override then pick up.
        m.load_for_game("AMKE", "");
        assert!(m.active.slow_motion.enabled);
    }

    #[test]
    fn latches_are_not_persisted() {
        let mut m = AccessibilityManager::new();
        m.load_for_game("BPEE", "");
        m.active.sticky_buttons.set_key_toggle_enabled(Key::B, true);
        m.process_key(Key::B, true);
        m.commit();
        let json = m.store.to_json();
        let back = AccessibilityStore::from_json(&json);
        let p = &back.per_game["BPEE"];
        assert!(p.sticky_buttons.is_key_toggle_enabled(Key::B));
        assert!(!p.sticky_buttons.is_key_latched(Key::B));
    }
}
