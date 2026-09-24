//! The Settings window.
//!
//! One window for every setting that used to live inline in the menus
//! (the Video menu alone had ~90 items and ran off the screen). A sidebar
//! groups pages by section (Display, Emulation, Audio); only one page shows
//! at a time, pages scroll, and the window is capped to the screen. Each
//! setting is a labelled row with a short explanation underneath, using
//! combo boxes and segmented buttons instead of long radio lists.
//!
//! Menus stay short: actions with their shortcuts, a few quick pickers, and
//! "⚙ … settings…" which opens this window at the matching section.
//!
//! The window edits a [`Settings`] view of the app's fields and reports what
//! changed in [`SettingsActions`]; `GbaApp` applies side effects (core
//! config, file dialogs, persistence).

use crate::gba::accessibility::{SlowMotionAudio, SlowMotionConfig, MIN_SLOW_MOTION};
use crate::gba::apu::audio_output::SurroundMode;
use crate::gba::frame_blend::FrameBlendMode;
use crate::gba::ppu::hd_mode7::{HdMode7Config, HdScale};
use crate::gba::widescreen::WidescreenMode;
use crate::ui::bezels::BezelMode;
use crate::ui::screen::{AspectRatio, DisplayFilter, ScaleMode};
use egui::{Color32, RichText};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    Picture,
    Enhance,
    Hd,
    Screen,
    Speed,
    Saves,
    Sound,
}

impl Page {
    pub const ALL: [Page; 7] =
        [Page::Picture, Page::Enhance, Page::Hd, Page::Screen, Page::Speed, Page::Sound, Page::Saves];

    /// Sidebar groups, in order.
    const SECTIONS: [(&'static str, &'static [Page]); 3] = [
        ("DISPLAY", &[Page::Picture, Page::Enhance, Page::Hd, Page::Screen]),
        ("EMULATION", &[Page::Speed, Page::Saves]),
        ("AUDIO", &[Page::Sound]),
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::Picture => "🎨 Picture",
            Page::Enhance => "✨ Enhance",
            Page::Hd => "🖼 HD & Widescreen",
            Page::Screen => "🖥 Screen & Frame",
            Page::Speed => "⏱ Speed & Latency",
            Page::Saves => "💾 Auto-save",
            Page::Sound => "🔊 Sound",
        }
    }

    fn blurb(self) -> &'static str {
        match self {
            Page::Picture => "Filter/shader and frame blending.",
            Page::Enhance => "Sharpening, color correction and glow.",
            Page::Hd => "High-res rendering, sprite packs and widescreen.",
            Page::Screen => "Size, aspect ratio, bezel and screenshots.",
            Page::Speed => "Game speed, slow motion and input latency.",
            Page::Saves => "Periodic saves, kept apart from your save slots.",
            Page::Sound => "Volume, surround and bass.",
        }
    }
}

/// The settings the window edits (borrowed from `GbaApp`).
pub struct Settings<'a> {
    pub filter: &'a mut DisplayFilter,
    pub xbrz_factor: &'a mut usize,
    pub blend: &'a mut FrameBlendMode,
    pub sharpen: &'a mut bool,
    pub sharpness: &'a mut f32,
    pub color_correction: &'a mut bool,
    pub ambient_glow: &'a mut bool,
    pub hd: &'a mut HdMode7Config,
    pub hd_pack_enabled: &'a mut bool,
    /// Name, scale, sprites, tiles of the loaded pack.
    pub hd_pack_info: Option<(String, usize, usize, usize)>,
    pub widescreen_enabled: &'a mut bool,
    pub widescreen_mode: &'a mut WidescreenMode,
    /// Profile line for the loaded game.
    pub widescreen_profile: String,
    pub scale_mode: &'a mut ScaleMode,
    pub aspect: &'a mut AspectRatio,
    pub bezel: &'a mut BezelMode,
    pub screenshot_enhanced: &'a mut bool,
    pub custom_shader_name: Option<String>,

    // Emulation
    /// 1 = normal, 2/4 = fast forward.
    pub speed: &'a mut u32,
    pub slow_motion: &'a mut SlowMotionConfig,
    pub run_ahead_frames: &'a mut u32,
    pub run_ahead_second_instance: &'a mut bool,
    /// "for <game>" / "default for all games".
    pub run_ahead_scope: String,
    pub run_ahead_fallback: Option<String>,

    // Audio
    pub muted: &'a mut bool,
    pub volume: &'a mut f32,
    pub surround: &'a mut SurroundMode,
    pub bass: &'a mut f32,
    pub width: &'a mut f32,
    pub hw_channels: usize,

    // Saves
    pub autosave_enabled: &'a mut bool,
    /// Minutes of play between auto-saves.
    pub autosave_minutes: &'a mut u32,
}

/// Things the app has to act on after the window is drawn.
#[derive(Default, Debug, PartialEq)]
pub struct SettingsActions {
    /// A persisted setting changed.
    pub changed: bool,
    pub hd_changed: bool,
    pub hd_pack_toggled: bool,
    pub widescreen_changed: bool,
    pub load_shader: bool,
    pub load_hd_pack: bool,
    pub dump_tiles: bool,
    pub slow_motion_changed: bool,
    pub run_ahead_changed: bool,
    pub audio_dsp_changed: bool,
    pub open_rtc: bool,
    pub open_mixer: bool,
    pub open_accessibility: bool,
}

#[derive(Default)]
pub struct SettingsWindow {
    pub is_open: bool,
    pub page: Page,
}

pub fn filter_name(f: DisplayFilter) -> &'static str {
    match f {
        DisplayFilter::Crisp => "Crisp pixels",
        DisplayFilter::Linear => "Smooth (bilinear)",
        DisplayFilter::LcdGrid => "LCD grid",
        DisplayFilter::LcdSubpixel => "GBA LCD subpixels",
        DisplayFilter::CrtScanlines => "CRT scanlines",
        DisplayFilter::CrtGeom => "CRT aperture grille",
        DisplayFilter::NvidiaSharpen => "Sharpened",
        DisplayFilter::Xbrz => "xBRZ smoothing",
        DisplayFilter::Custom => "Custom shader",
    }
}

pub const FILTERS: [DisplayFilter; 8] = [
    DisplayFilter::Crisp,
    DisplayFilter::Linear,
    DisplayFilter::LcdGrid,
    DisplayFilter::LcdSubpixel,
    DisplayFilter::CrtScanlines,
    DisplayFilter::CrtGeom,
    DisplayFilter::Xbrz,
    DisplayFilter::Custom,
];

pub fn blend_name(b: FrameBlendMode) -> &'static str {
    match b {
        FrameBlendMode::Off => "Off",
        FrameBlendMode::Simple50 => "50/50 blend",
        FrameBlendMode::SmartDeFlicker => "Smart de-flicker",
        FrameBlendMode::LcdGhosting { .. } => "LCD ghosting",
    }
}

pub const BLENDS: [FrameBlendMode; 4] = [
    FrameBlendMode::Off,
    FrameBlendMode::Simple50,
    FrameBlendMode::SmartDeFlicker,
    FrameBlendMode::LcdGhosting { decay: 0.65 },
];

fn scale_name(s: ScaleMode) -> &'static str {
    match s {
        ScaleMode::IntegerAuto => "Auto (pixel-perfect)",
        ScaleMode::Fit => "Fit to window",
        ScaleMode::Scale1x => "1× (240×160)",
        ScaleMode::Scale2x => "2× (480×320)",
        ScaleMode::Scale3x => "3× (720×480)",
        ScaleMode::Scale4x => "4× (960×640)",
        ScaleMode::Scale5x => "5× (1200×800)",
        ScaleMode::Scale6x => "6× (1440×960)",
        ScaleMode::Scale8x => "8× (1920×1280)",
        ScaleMode::Scale10x => "10× (2400×1600)",
        ScaleMode::Scale12x => "12× (2880×1920)",
        ScaleMode::Scale14x => "14× (3360×2240, 4K)",
    }
}

const SCALES: [ScaleMode; 12] = [
    ScaleMode::IntegerAuto,
    ScaleMode::Fit,
    ScaleMode::Scale1x,
    ScaleMode::Scale2x,
    ScaleMode::Scale3x,
    ScaleMode::Scale4x,
    ScaleMode::Scale5x,
    ScaleMode::Scale6x,
    ScaleMode::Scale8x,
    ScaleMode::Scale10x,
    ScaleMode::Scale12x,
    ScaleMode::Scale14x,
];

fn bezel_name(b: BezelMode) -> &'static str {
    match b {
        BezelMode::None => "None",
        BezelMode::GbaClassicIndigo => "GBA (Indigo)",
        BezelMode::GbaClassicGlacier => "GBA (Glacier)",
        BezelMode::GbaSpFlameRed => "GBA SP (Flame Red)",
        BezelMode::GameBoyPlayer => "Game Boy Player",
    }
}

const BEZELS: [BezelMode; 5] = [
    BezelMode::None,
    BezelMode::GbaClassicIndigo,
    BezelMode::GbaClassicGlacier,
    BezelMode::GbaSpFlameRed,
    BezelMode::GameBoyPlayer,
];

const LABEL_W: f32 = 160.0;

/// A settings row: left-aligned name, control beside it, and a readable
/// hint underneath aligned with the control.
fn row(ui: &mut egui::Ui, label: &str, hint: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(LABEL_W, ui.spacing().interact_size.y),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(LABEL_W);
                ui.label(label);
            },
        );
        add(ui);
    });
    if !hint.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(LABEL_W + ui.spacing().item_spacing.x);
            ui.add(egui::Label::new(RichText::new(hint).size(12.5).color(ui.visuals().weak_text_color())).wrap());
        });
    }
    ui.add_space(10.0);
}

/// Bold section heading with a little breathing room.
fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).strong().size(15.0));
    ui.add_space(4.0);
}

/// A combo box over `items`; returns true when the choice changed.
fn combo<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut T,
    items: &[T],
    name: impl Fn(T) -> String,
) -> bool {
    let mut changed = false;
    let current = name(*value);
    let r = egui::ComboBox::from_id_salt(id).width(220.0).selected_text(current.clone()).show_ui(ui, |ui| {
        for &it in items {
            changed |= ui.selectable_value(value, it, name(it)).changed();
        }
    });
    // egui leaves a combo box unnamed for screen readers unless a label is
    // drawn beside it; name it after its current value.
    let enabled = ui.is_enabled();
    r.response.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, enabled, current.clone());
        info.current_text_value = Some(current.clone());
        info
    });
    changed
}

/// Segmented buttons (a row of toggle-style buttons); true when changed.
fn segmented<T: Copy + PartialEq>(ui: &mut egui::Ui, value: &mut T, items: &[(T, &str)]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for &(it, label) in items {
            if ui.selectable_label(*value == it, label).clicked() && *value != it {
                *value = it;
                changed = true;
            }
        }
    });
    changed
}

impl SettingsWindow {
    /// Select a page by index into [`Page::ALL`].
    pub fn set_page(&mut self, i: usize) {
        self.page = Page::ALL[i.min(Page::ALL.len() - 1)];
    }

    /// Open the window at `page`.
    pub fn open_at(&mut self, page: Page) {
        self.page = page;
        self.is_open = true;
    }

    pub fn show(&mut self, ctx: &egui::Context, mut s: Settings<'_>) -> SettingsActions {
        let mut act = SettingsActions::default();
        if !self.is_open {
            return act;
        }
        let screen = ctx.screen_rect();
        // Never taller or wider than the screen (minus the menu bar and a
        // margin), so every control stays reachable on small displays.
        let max_h = (screen.height() - 80.0).max(200.0);
        let max_w = (screen.width() - 40.0).max(300.0);
        let mut open = self.is_open;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([620.0, 460.0_f32.min(max_h)])
            .max_height(max_h)
            .max_width(max_w)
            .default_pos(screen.center() - egui::vec2(310.0, 240.0_f32.min(max_h / 2.0)))
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    // Sidebar.
                    ui.vertical(|ui| {
                        ui.set_width(170.0);
                        ui.spacing_mut().item_spacing.y = 4.0;
                        for (i, (heading, pages)) in Page::SECTIONS.iter().enumerate() {
                            if i > 0 {
                                ui.add_space(8.0);
                            }
                            ui.label(RichText::new(*heading).size(11.5).strong().color(ui.visuals().weak_text_color()));
                            for &p in *pages {
                                let text = RichText::new(p.title()).size(14.5);
                                let r = ui.add_sized(
                                    [170.0, 30.0],
                                    egui::Button::new(text).selected(self.page == p).frame(self.page == p),
                                );
                                if r.clicked() {
                                    self.page = p;
                                }
                            }
                        }
                    });
                    ui.separator();
                    // Page.
                    ui.vertical(|ui| {
                        ui.label(RichText::new(self.page.title()).size(20.0).strong());
                        ui.label(RichText::new(self.page.blurb()).color(ui.visuals().weak_text_color()));
                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.page {
                            Page::Picture => picture(ui, &mut s, &mut act),
                            Page::Enhance => enhance(ui, &mut s, &mut act),
                            Page::Hd => hd(ui, &mut s, &mut act),
                            Page::Screen => screen_page(ui, &mut s, &mut act),
                            Page::Speed => speed_page(ui, &mut s, &mut act),
                            Page::Saves => saves_page(ui, &mut s, &mut act),
                            Page::Sound => sound_page(ui, &mut s, &mut act),
                        });
                    });
                });
            });
        self.is_open = open;
        act
    }
}

fn picture(ui: &mut egui::Ui, s: &mut Settings<'_>, act: &mut SettingsActions) {
    let filter = &mut *s.filter;
    row(ui, "Filter / shader", "How pixels are drawn: crisp, smoothed, or imitating an LCD/CRT.", |ui| {
        act.changed |= combo(ui, "vs_filter", filter, &FILTERS, |f| {
            if f == DisplayFilter::Custom {
                s.custom_shader_name.clone().map_or("Custom shader".into(), |n| format!("Custom ({n})"))
            } else {
                filter_name(f).into()
            }
        });
    });
    if *filter == DisplayFilter::Xbrz {
        let f = &mut *s.xbrz_factor;
        row(ui, "xBRZ scale", "4× is a good balance of quality and speed.", |ui| {
            act.changed |= segmented(ui, f, &[(2, "2×"), (3, "3×"), (4, "4×"), (5, "5×"), (6, "6×")]);
        });
    }
    row(ui, "Custom shader", "Load a .shader / .json profile.", |ui| {
        if ui.button("Load…").clicked() {
            act.load_shader = true;
        }
    });
    let blend = &mut *s.blend;
    row(
        ui,
        "Frame blending",
        "Some games flicker sprites for transparency; blending shows them as the real screen did.",
        |ui| {
            act.changed |= combo(ui, "vs_blend", blend, &BLENDS, |b| blend_name(b).into());
        },
    );
}

fn enhance(ui: &mut egui::Ui, s: &mut Settings<'_>, act: &mut SettingsActions) {
    let sharpen = &mut *s.sharpen;
    row(ui, "Sharpening", "Adaptive sharpening (NVIDIA NIS / AMD CAS style).", |ui| {
        act.changed |= ui.checkbox(sharpen, "Enabled").changed();
    });
    if *sharpen || *s.filter == DisplayFilter::NvidiaSharpen {
        let v = &mut *s.sharpness;
        row(ui, "Strength", "", |ui| {
            act.changed |= ui.add(egui::Slider::new(v, 0.0..=1.0)).changed();
        });
    }
    let cc = &mut *s.color_correction;
    row(ui, "Color correction", "Muted colors like the original GBA screen.", |ui| {
        act.changed |= ui.checkbox(cc, "Enabled").changed();
    });
    let glow = &mut *s.ambient_glow;
    row(ui, "Ambient edge glow", "Fills the side bars with a soft glow of the picture's edge colors.", |ui| {
        act.changed |= ui.checkbox(glow, "Enabled").changed();
    });
}

fn hd(ui: &mut egui::Ui, s: &mut Settings<'_>, act: &mut SettingsActions) {
    section(ui, "HD Mode 7");
    let hdc = &mut *s.hd;
    let before = *hdc;
    row(
        ui,
        "Resolution",
        "Redraws rotated and scaled graphics (racetracks, world maps, spinning effects) at high \
         resolution. Ordinary tiles and sprites look the same, so many scenes won't change.",
        |ui| {
            segmented(ui, &mut hdc.scale, &[(HdScale::Off, "Off"), (HdScale::X2, "2×"), (HdScale::X4, "4×"), (HdScale::X8, "8×")]);
        },
    );
    if hdc.scale != HdScale::Off {
        row(ui, "Smooth perspective", "Removes stair-stepping on 3D-style floors.", |ui| {
            ui.checkbox(&mut hdc.perspective_interpolation, "Enabled");
        });
        row(ui, "Anti-aliasing (SSAA)", "Downsamples back to native size: smoother edges, same size.", |ui| {
            ui.checkbox(&mut hdc.ssaa, "Enabled");
        });
    }
    if *hdc != before {
        act.hd_changed = true;
        act.changed = true;
    }

    ui.separator();
    section(ui, "HD sprite & tile packs");
    let pack_on = &mut *s.hd_pack_enabled;
    row(ui, "Replacement pack", "", |ui| {
        if ui.add_enabled(s.hd_pack_info.is_some(), egui::Checkbox::new(pack_on, "Enabled")).changed() {
            act.hd_pack_toggled = true;
            act.changed = true;
        }
    });
    let status = match &s.hd_pack_info {
        Some((name, scale, sprites, tiles)) => {
            RichText::new(format!("{name}: {scale}× art, {sprites} sprites, {tiles} tiles")).color(Color32::LIGHT_GREEN)
        }
        None => RichText::new("No pack loaded").color(ui.visuals().weak_text_color()),
    };
    ui.horizontal(|ui| {
        ui.add_space(LABEL_W + ui.spacing().item_spacing.x);
        ui.label(status);
    });
    ui.horizontal(|ui| {
        ui.add_space(LABEL_W + ui.spacing().item_spacing.x);
        if ui.button("Load pack folder…").clicked() {
            act.load_hd_pack = true;
        }
        if ui.button("Dump tiles & sprites…").on_hover_text("Export this screen's graphics as a pack template").clicked() {
            act.dump_tiles = true;
        }
    });
    ui.add_space(8.0);

    ui.separator();
    section(ui, "Widescreen");
    let ws = &mut *s.widescreen_enabled;
    row(ui, "Expand to widescreen", &s.widescreen_profile, |ui| {
        if ui.checkbox(ws, "Enabled").changed() {
            act.widescreen_changed = true;
            act.changed = true;
        }
    });
    let mode = &mut *s.widescreen_mode;
    if *ws {
        row(ui, "Shape", "", |ui| {
            if segmented(
                ui,
                mode,
                &[
                    (WidescreenMode::Ratio16_9, "16:9"),
                    (WidescreenMode::TileAligned16_9, "16:9 tile-aligned"),
                    (WidescreenMode::Ratio16_10, "16:10"),
                ],
            ) {
                act.widescreen_changed = true;
                act.changed = true;
            }
        });
    }
}

fn screen_page(ui: &mut egui::Ui, s: &mut Settings<'_>, act: &mut SettingsActions) {
    let scale = &mut *s.scale_mode;
    row(ui, "Size", "Auto keeps every GBA pixel the same size (no shimmering).", |ui| {
        act.changed |= combo(ui, "vs_scale", scale, &SCALES, |x| scale_name(x).into());
    });
    let aspect = &mut *s.aspect;
    row(ui, "Aspect ratio", "F3 cycles this while playing.", |ui| {
        act.changed |= combo(ui, "vs_aspect", aspect, &AspectRatio::ALL, |a| a.display_name().into());
    });
    let bezel = &mut *s.bezel;
    row(ui, "Bezel", "A console frame around the picture.", |ui| {
        act.changed |= combo(ui, "vs_bezel", bezel, &BEZELS, |b| bezel_name(b).into());
    });
    let shot = &mut *s.screenshot_enhanced;
    row(ui, "Screenshots", "", |ui| {
        act.changed |= segmented(ui, shot, &[(false, "Raw 240×160"), (true, "As shown (filters)")]);
    });
}

pub fn speed_name(speed: u32, slow: &SlowMotionConfig) -> String {
    if speed > 1 {
        format!("{speed}× fast")
    } else if slow.enabled {
        format!("{:.0}% slow motion", slow.speed_factor * 100.0)
    } else {
        "Normal".into()
    }
}

pub fn surround_name(m: SurroundMode) -> &'static str {
    match m {
        SurroundMode::Stereo => "Stereo",
        SurroundMode::Surround51 => "5.1 surround",
        SurroundMode::Headphone3D => "3D headphones",
    }
}

pub const SURROUNDS: [SurroundMode; 3] = [SurroundMode::Headphone3D, SurroundMode::Surround51, SurroundMode::Stereo];

/// A full-width link-style button under the control column.
fn open_button(ui: &mut egui::Ui, label: &str) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.add_space(LABEL_W + ui.spacing().item_spacing.x);
        clicked = ui.button(label).clicked();
    });
    ui.add_space(6.0);
    clicked
}

fn speed_page(ui: &mut egui::Ui, s: &mut Settings<'_>, act: &mut SettingsActions) {
    section(ui, "Speed");
    let speed = &mut *s.speed;
    row(ui, "Game speed", "Hold the turbo key for 4× at any time.", |ui| {
        act.changed |= segmented(ui, speed, &[(1, "Normal"), (2, "2×"), (4, "4×")]);
    });
    let sm = &mut *s.slow_motion;
    let mut pct = if sm.enabled { (sm.speed_factor * 100.0).round() as u32 } else { 100 };
    let before = pct;
    row(ui, "Slow motion", "Saved per game. Only applies at normal speed.", |ui| {
        segmented(ui, &mut pct, &[(100, "Off"), (75, "75%"), (50, "50%"), (25, "25%"), (10, "10%")]);
    });
    if pct != before {
        sm.enabled = pct < 100;
        if pct < 100 {
            sm.speed_factor = (pct as f32 / 100.0).max(MIN_SLOW_MOTION);
        }
        act.slow_motion_changed = true;
    }
    if sm.enabled {
        let mut audio = sm.audio;
        row(ui, "Slow-motion sound", "", |ui| {
            if segmented(
                ui,
                &mut audio,
                &[
                    (SlowMotionAudio::PitchPreserved, "Keep pitch"),
                    (SlowMotionAudio::Tape, "Tape"),
                    (SlowMotionAudio::Mute, "Mute"),
                ],
            ) {
                sm.audio = audio;
                act.slow_motion_changed = true;
            }
        });
    }

    ui.separator();
    section(ui, "Input latency");
    let frames = &mut *s.run_ahead_frames;
    let hint = format!("Hides the game's own input lag by running ahead. Saved {}.", s.run_ahead_scope);
    row(ui, "Run-ahead", &hint, |ui| {
        act.run_ahead_changed |= segmented(ui, frames, &[(0, "Off"), (1, "1 frame"), (2, "2 frames")]);
    });
    if *frames > 0 {
        let second = &mut *s.run_ahead_second_instance;
        row(ui, "Second instance", "Runs ahead in a separate copy so audio never glitches. Uses more CPU.", |ui| {
            act.run_ahead_changed |= ui.checkbox(second, "Enabled").changed();
        });
        if let Some(reason) = &s.run_ahead_fallback {
            ui.horizontal(|ui| {
                ui.add_space(LABEL_W + ui.spacing().item_spacing.x);
                ui.colored_label(Color32::YELLOW, format!("⚠ Using single instance: {reason}"));
            });
        }
    }

    ui.separator();
    section(ui, "More");
    if open_button(ui, "🕒 Real-time clock…") {
        act.open_rtc = true;
    }
    if open_button(ui, "♿ Accessibility…") {
        act.open_accessibility = true;
    }
}

fn saves_page(ui: &mut egui::Ui, s: &mut Settings<'_>, act: &mut SettingsActions) {
    use crate::autosave::{MAX_INTERVAL_MINUTES, MIN_INTERVAL_MINUTES, ROTATION};
    section(ui, "Auto-save");
    let enabled = &mut *s.autosave_enabled;
    row(
        ui,
        "Auto-save",
        "Saves the game state on a timer. Only play time counts: paused time and menus don't.",
        |ui| {
            act.changed |= ui.checkbox(enabled, "Enabled").changed();
        },
    );
    let on = *enabled;
    let minutes = &mut *s.autosave_minutes;
    ui.add_enabled_ui(on, |ui| {
        row(ui, "Every", "", |ui| {
            act.changed |= segmented(ui, minutes, &[(1, "1 min"), (2, "2 min"), (5, "5 min"), (10, "10 min"), (15, "15 min")]);
        });
        row(ui, "Custom", "Any interval from 1 to 60 minutes. The default is 5.", |ui| {
            act.changed |= ui
                .add(
                    egui::Slider::new(minutes, MIN_INTERVAL_MINUTES..=MAX_INTERVAL_MINUTES)
                        .suffix(" min")
                        .clamping(egui::SliderClamping::Always),
                )
                .changed();
        });
    });
    ui.separator();
    section(ui, "How it works");
    ui.label(format!(
        "The last {ROTATION} auto-saves are kept for each game. When all {ROTATION} are used, the oldest is \
         replaced. They're stored with your saves, stay after you quit, and never touch your manual save slots."
    ));
    ui.label(RichText::new("Load them from File › Auto-saves, or from the Save states window.").weak());
}

fn sound_page(ui: &mut egui::Ui, s: &mut Settings<'_>, act: &mut SettingsActions) {
    section(ui, "Output");
    let muted = &mut *s.muted;
    row(ui, "Sound", "", |ui| {
        let mut on = !*muted;
        if ui.checkbox(&mut on, "On").changed() {
            *muted = !on;
            act.changed = true;
        }
    });
    let vol = &mut *s.volume;
    row(ui, "Volume", "", |ui| {
        act.changed |= ui.add(egui::Slider::new(vol, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0))).changed();
    });

    ui.separator();
    section(ui, "Surround & bass");
    let sur = &mut *s.surround;
    let ch = s.hw_channels;
    let hint = format!("Your output device has {ch} channel{}.", if ch == 1 { "" } else { "s" });
    row(ui, "Mode", &hint, |ui| {
        act.audio_dsp_changed |= combo(ui, "st_surround", sur, &SURROUNDS, |m| surround_name(m).into());
    });
    let bass = &mut *s.bass;
    row(ui, "Bass boost", "", |ui| {
        act.audio_dsp_changed |= ui.add(egui::Slider::new(bass, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0))).changed();
    });
    if *sur != SurroundMode::Stereo {
        let width = &mut *s.width;
        row(ui, "Surround width", "How far the sound spreads around you.", |ui| {
            act.audio_dsp_changed |= ui.add(egui::Slider::new(width, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0))).changed();
        });
    }

    ui.separator();
    section(ui, "More");
    if open_button(ui, "🎛 Channel mixer & HD music…") {
        act.open_mixer = true;
    }
}

/// A menu item: label on the left, keyboard shortcut right-aligned and
/// dimmed (the standard desktop menu layout). Returns true when clicked.
pub fn menu_item(ui: &mut egui::Ui, label: impl Into<egui::WidgetText>, shortcut: &str) -> bool {
    let mut b = egui::Button::new(label);
    if !shortcut.is_empty() {
        b = b.shortcut_text(RichText::new(shortcut).color(ui.visuals().weak_text_color()));
    }
    ui.add(b).clicked()
}
