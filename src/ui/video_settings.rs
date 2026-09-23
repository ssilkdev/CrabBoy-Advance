//! Display Settings window.
//!
//! Replaces the old Video menu, which had grown to ~90 items in one dropdown
//! and ran off the bottom of the screen. Settings are grouped into pages
//! picked from a sidebar; only one page is shown at a time, the page
//! scrolls, and the window is capped to the screen so nothing can end up
//! out of reach. Choices are compact combo boxes and segmented buttons
//! instead of long radio lists.
//!
//! The dialog edits a [`VideoSettings`] view of the app's fields and reports
//! what changed; `GbaApp` applies the side effects (core config, file
//! dialogs, persistence).

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
}

impl Page {
    const ALL: [Page; 4] = [Page::Picture, Page::Enhance, Page::Hd, Page::Screen];

    fn title(self) -> &'static str {
        match self {
            Page::Picture => "🎨 Picture",
            Page::Enhance => "✨ Enhance",
            Page::Hd => "🖼 HD & Widescreen",
            Page::Screen => "🖥 Screen & Frame",
        }
    }

    fn blurb(self) -> &'static str {
        match self {
            Page::Picture => "Filter/shader and frame blending.",
            Page::Enhance => "Sharpening, color correction and glow.",
            Page::Hd => "High-res rendering, sprite packs and widescreen.",
            Page::Screen => "Size, aspect ratio, bezel and screenshots.",
        }
    }
}

/// The settings the window edits (borrowed from `GbaApp`).
pub struct VideoSettings<'a> {
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
}

/// Things the app has to act on after the window is drawn.
#[derive(Default, Debug, PartialEq)]
pub struct VideoActions {
    /// A persisted setting changed.
    pub changed: bool,
    pub hd_changed: bool,
    pub hd_pack_toggled: bool,
    pub widescreen_changed: bool,
    pub load_shader: bool,
    pub load_hd_pack: bool,
    pub dump_tiles: bool,
}

#[derive(Default)]
pub struct VideoSettingsDialog {
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

impl VideoSettingsDialog {
    /// Select a page by index (0 = Picture ... 3 = Screen & Frame).
    pub fn set_page(&mut self, i: usize) {
        self.page = Page::ALL[i.min(Page::ALL.len() - 1)];
    }

    pub fn show(&mut self, ctx: &egui::Context, mut s: VideoSettings<'_>) -> VideoActions {
        let mut act = VideoActions::default();
        if !self.is_open {
            return act;
        }
        let screen = ctx.screen_rect();
        // Never taller or wider than the screen (minus the menu bar and a
        // margin), so every control stays reachable on small displays.
        let max_h = (screen.height() - 80.0).max(200.0);
        let max_w = (screen.width() - 40.0).max(300.0);
        let mut open = self.is_open;
        egui::Window::new("Display Settings")
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
                        for p in Page::ALL {
                            let text = RichText::new(p.title()).size(14.5);
                            let r = ui.add_sized(
                                [170.0, 32.0],
                                egui::Button::new(text).selected(self.page == p).frame(self.page == p),
                            );
                            if r.clicked() {
                                self.page = p;
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
                        });
                    });
                });
            });
        self.is_open = open;
        act
    }
}

fn picture(ui: &mut egui::Ui, s: &mut VideoSettings<'_>, act: &mut VideoActions) {
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

fn enhance(ui: &mut egui::Ui, s: &mut VideoSettings<'_>, act: &mut VideoActions) {
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

fn hd(ui: &mut egui::Ui, s: &mut VideoSettings<'_>, act: &mut VideoActions) {
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

fn screen_page(ui: &mut egui::Ui, s: &mut VideoSettings<'_>, act: &mut VideoActions) {
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
