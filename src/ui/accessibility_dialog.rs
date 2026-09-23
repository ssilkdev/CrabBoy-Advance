//! Accessibility dialog (ROADMAP M10).
//!
//! Every change is saved for the loaded game straight away (or as the global
//! default when no game is loaded), so there's no "save" step to forget.

use crate::gba::accessibility::{
    apply_colorblind_filter, key_name, AccessibilityManager, ColorblindMode, Handedness, SlowMotionAudio,
    ALL_KEYS, MIN_SLOW_MOTION, UI_SCALE_MAX, UI_SCALE_MIN,
};
use crate::ui::controls::KeyBindings;
use egui::{Color32, RichText, Window};

#[derive(Default)]
pub struct AccessibilityDialog {
    pub is_open: bool,
}

impl AccessibilityDialog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true when a setting changed (the caller commits + saves).
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        acc: &mut AccessibilityManager,
        user_bindings: &KeyBindings,
        toast: &mut Option<String>,
    ) -> bool {
        if !self.is_open {
            return false;
        }
        let mut changed = false;
        let mut open = self.is_open;
        Window::new("Accessibility")
            .open(&mut open)
            .default_width(520.0)
            .resizable(true)
            .show(ctx, |ui| {
                match acc.current_game_id.clone() {
                    Some(id) => {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Settings for");
                            ui.label(RichText::new(&id).strong());
                            ui.label(
                                RichText::new(if acc.has_game_override() {
                                    "(saved for this game)"
                                } else {
                                    "(using the global default)"
                                })
                                .weak(),
                            );
                        });
                    }
                    None => {
                        ui.label("No game loaded: changes set the global default.");
                    }
                }
                ui.separator();

                egui::ScrollArea::vertical().show(ui, |ui| {
                    changed |= color_section(ui, acc);
                    ui.add_space(6.0);
                    changed |= slow_motion_section(ui, acc);
                    ui.add_space(6.0);
                    changed |= sticky_section(ui, acc);
                    ui.add_space(6.0);
                    changed |= one_handed_section(ui, acc, user_bindings);
                    ui.add_space(6.0);
                    changed |= scale_section(ui, acc);
                    ui.add_space(6.0);

                    ui.group(|ui| {
                        ui.heading("Defaults");
                        ui.horizontal_wrapped(|ui| {
                            if acc.current_game_id.is_some() {
                                if ui
                                    .add_enabled(acc.has_game_override(), egui::Button::new("Reset this game to the default"))
                                    .clicked()
                                {
                                    acc.reset_game_to_default();
                                    *toast = Some("Accessibility: using the global default".into());
                                    changed = true;
                                }
                                if ui.button("Use these settings for all games").clicked() {
                                    acc.set_as_global_default();
                                    *toast = Some("Accessibility: saved as the global default".into());
                                    changed = true;
                                }
                            }
                        });
                    });
                });
            });
        self.is_open = open;
        changed
    }
}

fn color_section(ui: &mut egui::Ui, acc: &mut AccessibilityManager) -> bool {
    let mut changed = false;
    let p = &mut acc.active;
    ui.group(|ui| {
        ui.heading("Color vision");
        ui.label(
            RichText::new("Corrects colors so red/green/blue differences stay visible (Daltonization), or switches to grayscale / high contrast.")
                .weak()
                .small(),
        );
        ui.horizontal_wrapped(|ui| {
            for mode in ColorblindMode::ALL {
                changed |= ui.radio_value(&mut p.colorblind_mode, mode, mode.display_name()).changed();
            }
        });
        if p.colorblind_mode != ColorblindMode::None {
            ui.horizontal(|ui| {
                ui.label("Strength");
                changed |= ui
                    .add(egui::Slider::new(&mut p.colorblind_intensity, 0.1..=1.0).custom_formatter(|n, _| format!("{:.0}%", n * 100.0)))
                    .changed();
            });
        }
        ui.horizontal(|ui| {
            for (name, (r, g, b)) in [
                ("Red", (220u8, 40u8, 40u8)),
                ("Orange", (240, 140, 30)),
                ("Yellow", (240, 220, 30)),
                ("Green", (40, 190, 40)),
                ("Blue", (40, 100, 240)),
                ("Purple", (150, 60, 200)),
            ] {
                let (fr, fg, fb) = apply_colorblind_filter(r, g, b, p.colorblind_mode, p.colorblind_intensity);
                ui.vertical(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(44.0, 20.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 3.0, Color32::from_rgb(fr, fg, fb));
                    ui.label(RichText::new(name).small());
                });
            }
        });
    });
    changed
}

fn slow_motion_section(ui: &mut egui::Ui, acc: &mut AccessibilityManager) -> bool {
    let mut changed = false;
    let s = &mut acc.active.slow_motion;
    ui.group(|ui| {
        ui.heading("Slow motion");
        ui.label(RichText::new("Runs the whole game slower. Also in Emulation > Speed.").weak().small());
        ui.horizontal_wrapped(|ui| {
            changed |= ui.checkbox(&mut s.enabled, "Enabled").changed();
            for (label, v) in [("75%", 0.75), ("50%", 0.5), ("25%", 0.25), ("10%", 0.1)] {
                if ui.selectable_label(s.enabled && (s.speed_factor - v).abs() < 0.01, label).clicked() {
                    s.enabled = true;
                    s.speed_factor = v;
                    changed = true;
                }
            }
        });
        ui.add_enabled_ui(s.enabled, |ui| {
            ui.horizontal(|ui| {
                ui.label("Speed");
                changed |= ui
                    .add(egui::Slider::new(&mut s.speed_factor, MIN_SLOW_MOTION..=1.0).custom_formatter(|n, _| format!("{:.0}%", n * 100.0)))
                    .changed();
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Sound");
                for a in SlowMotionAudio::ALL {
                    changed |= ui.radio_value(&mut s.audio, a, a.display_name()).changed();
                }
            });
        });
    });
    changed
}

fn sticky_section(ui: &mut egui::Ui, acc: &mut AccessibilityManager) -> bool {
    let mut changed = false;
    let st = &mut acc.active.sticky_buttons;
    ui.group(|ui| {
        ui.heading("Toggle instead of hold");
        ui.label(
            RichText::new("Checked buttons latch: press once to hold, press again to let go. Handy for running (B) or charging moves.")
                .weak()
                .small(),
        );
        ui.horizontal_wrapped(|ui| {
            for key in ALL_KEYS {
                let mut on = st.is_key_toggle_enabled(key);
                let mut label = RichText::new(key_name(key));
                if st.is_key_latched(key) {
                    label = label.strong().color(Color32::from_rgb(110, 230, 110));
                }
                if ui.checkbox(&mut on, label).changed() {
                    st.set_key_toggle_enabled(key, on);
                    changed = true;
                }
            }
        });
        if st.latched_states != 0 && ui.button("Release all held buttons").clicked() {
            st.clear_latches();
        }
    });
    changed
}

fn one_handed_section(ui: &mut egui::Ui, acc: &mut AccessibilityManager, user: &KeyBindings) -> bool {
    let mut changed = false;
    let p = &mut acc.active;
    ui.group(|ui| {
        ui.heading("One-handed controls");
        ui.label(RichText::new("Keyboard layout. \"Two hands\" is your own bindings, which the presets never change.").weak().small());
        ui.horizontal(|ui| {
            for h in Handedness::ALL {
                changed |= ui.radio_value(&mut p.one_handed_desktop, h, h.display_name()).changed();
            }
        });
        let summary = user.for_layout(p.one_handed_desktop).game_button_summary();
        let text = summary.iter().map(|(b, k)| format!("{b}: {k}")).collect::<Vec<_>>().join("   ");
        ui.label(RichText::new(text).monospace().small());
    });
    changed
}

fn scale_section(ui: &mut egui::Ui, acc: &mut AccessibilityManager) -> bool {
    let mut changed = false;
    let p = &mut acc.active;
    ui.group(|ui| {
        ui.heading("Interface size");
        ui.horizontal_wrapped(|ui| {
            for (label, v) in [("100%", 1.0), ("125%", 1.25), ("150%", 1.5), ("175%", 1.75), ("200%", 2.0)] {
                if ui.selectable_label((p.ui_scale - v).abs() < 0.01, label).clicked() {
                    p.ui_scale = v;
                    changed = true;
                }
            }
        });
        // The app applies the new size once the mouse button is released, so
        // the slider doesn't jump away from the pointer mid-drag.
        changed |= ui
            .add(egui::Slider::new(&mut p.ui_scale, UI_SCALE_MIN..=UI_SCALE_MAX).custom_formatter(|n, _| format!("{:.0}%", n * 100.0)))
            .changed();
    });
    changed
}
