//! Controller configuration dialog: pad selection, profiles, live remapping.
//!
//! UX rules this dialog is built around:
//!
//! * **Nothing is lost by accident.** Editing a built-in profile forks it into
//!   a user copy instead of mutating stock bindings, and every destructive
//!   action (delete profile, reset bindings) is a two-step confirm.
//!
//! * **The user can always see what the pad is doing.** A live "input monitor"
//!   shows held actions and raw stick deflection, so "my controller isn't
//!   working" becomes a question the dialog answers on the spot rather than one
//!   the user has to file a bug about.
//!
//! * **Remapping is press-what-you-mean.** Click a binding, press the button.
//!   Holding two buttons captures a chord. Conflicts are reported immediately,
//!   with the option to keep both.
//!
//! * **Changes persist without a Save button.** Every edit marks the config
//!   dirty and the app flushes it; an explicit Save exists only as reassurance.

use eframe::egui::{self, Color32, RichText, ScrollArea, Vec2};

use super::controls::{
    pretty_input, CaptureState, FaceLayout, GamepadManager, PadAction, PadInput, PadProfile,
};

/// Confirmations awaiting a second click.
#[derive(PartialEq, Clone)]
enum Pending {
    None,
    DeleteProfile(String),
    ResetProfile(String),
}

pub struct ControlsDialog {
    pub is_open: bool,
    /// Set whenever a binding/profile/pad setting changed, so the app knows to
    /// write config.json. Cleared by the app once flushed.
    pub dirty: bool,
    tab: Tab,
    pending: Pending,
    /// Conflict banner: (input, actions it was already bound to).
    last_conflict: Option<(PadInput, Vec<PadAction>)>,
    rename_buffer: String,
    renaming: bool,
    filter: String,
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Devices,
    Bindings,
    Guide,
}

impl Default for ControlsDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlsDialog {
    pub fn new() -> Self {
        Self {
            is_open: false,
            dirty: false,
            tab: Tab::Devices,
            pending: Pending::None,
            last_conflict: None,
            rename_buffer: String::new(),
            renaming: false,
            filter: String::new(),
        }
    }

    pub fn open(&mut self) {
        self.is_open = true;
        self.pending = Pending::None;
    }

    pub fn show(&mut self, ctx: &egui::Context, gp: &mut GamepadManager, toast: &mut Option<String>) {
        if !self.is_open {
            // A dialog closed mid-capture must not leave the pad swallowed.
            if gp.is_capturing() {
                gp.cancel_capture();
            }
            return;
        }

        // Escape always aborts a listening capture before it does anything else,
        // so a user who clicked the wrong row is never stuck.
        if gp.is_capturing() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            gp.cancel_capture();
        }

        // Commit a finished capture.
        if let Some((action, input)) = gp.take_capture() {
            self.apply_binding(gp, action, input, toast);
        }

        let mut is_open = self.is_open;
        egui::Window::new("🎮 Controllers & Key Mapping")
            .open(&mut is_open)
            .default_size(Vec2::new(720.0, 640.0))
            .min_size(Vec2::new(560.0, 420.0))
            .resizable(true)
            .show(ctx, |ui| {
                self.header(ui, gp);
                ui.separator();

                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Devices, "🔌 Devices");
                    ui.selectable_value(&mut self.tab, Tab::Bindings, "🎯 Button Mapping");
                    ui.selectable_value(&mut self.tab, Tab::Guide, "📖 Guide Controls");
                });
                ui.separator();

                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.tab {
                    Tab::Devices => self.devices_tab(ui, gp, toast),
                    Tab::Bindings => self.bindings_tab(ui, gp, toast),
                    Tab::Guide => self.guide_tab(ui, gp),
                });
            });

        self.is_open = is_open;

        // The capture prompt is a separate always-on-top window: it must stay
        // visible even if the user's button press moves focus, and it must be
        // impossible to miss.
        if let CaptureState::Listening(action) = gp.capture {
            self.capture_prompt(ctx, gp, action);
        }
    }

    // --- Header ----------------------------------------------------------

    fn header(&mut self, ui: &mut egui::Ui, gp: &GamepadManager) {
        ui.horizontal(|ui| {
            if !gp.subsystem_available() {
                ui.label(
                    RichText::new("⚠ Gamepad subsystem unavailable on this system.")
                        .color(Color32::from_rgb(255, 120, 120))
                        .strong(),
                );
                return;
            }
            match gp.active_pad_info() {
                Some(info) => {
                    ui.label(
                        RichText::new(format!("● {}", info.name))
                            .color(Color32::from_rgb(120, 230, 140))
                            .strong(),
                    );
                    ui.label(RichText::new(format!("[{}]", info.power)).weak());
                    ui.label(RichText::new(format!("profile: {}", info.profile)).weak());
                }
                None => {
                    ui.label(
                        RichText::new("○ No controller connected")
                            .color(Color32::from_rgb(235, 205, 110)),
                    );
                    ui.label(
                        RichText::new(
                            "— plug in via USB, the 2.4GHz dongle, or pair over Bluetooth.",
                        )
                        .weak(),
                    );
                }
            }
        });
    }

    // --- Devices tab -----------------------------------------------------

    fn devices_tab(
        &mut self,
        ui: &mut egui::Ui,
        gp: &mut GamepadManager,
        toast: &mut Option<String>,
    ) {
        ui.heading("Connected Controllers");
        ui.label(
            RichText::new(
                "Pick which controller plays the game. The choice is remembered, so the same \
                 pad becomes Player 1 next launch.",
            )
            .weak(),
        );
        ui.add_space(6.0);

        if gp.pads.is_empty() {
            ui.group(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(12.0);
                    ui.label(RichText::new("No controllers detected").size(15.0).strong());
                    ui.label(
                        RichText::new(
                            "An 8BitDo Ultimate 2 (Pro) is recognised automatically over its \
                             2.4GHz dongle, Bluetooth or USB-C.\n\
                             On Linux, check that your user can read /dev/input/event* \
                             (usually the `input` group).",
                        )
                        .weak(),
                    );
                    ui.add_space(12.0);
                });
            });
        }

        // Snapshot to avoid borrowing `gp` while we mutate it inside the loop.
        let pads: Vec<_> = gp.pads.clone();
        let active = gp.active_pad;
        let profile_names: Vec<String> = gp.settings.profiles.iter().map(|p| p.name.clone()).collect();

        for pad in &pads {
            let is_active = Some(pad.id) == active;
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            let title = RichText::new(&pad.name).strong().size(14.0);
                            ui.label(if is_active {
                                title.color(Color32::from_rgb(120, 230, 140))
                            } else {
                                title
                            });
                            if is_active {
                                ui.label(
                                    RichText::new("PLAYER 1")
                                        .small()
                                        .strong()
                                        .color(Color32::from_rgb(120, 230, 140)),
                                );
                            }
                            if pad.recently_active {
                                ui.label(
                                    RichText::new("• active input")
                                        .small()
                                        .color(Color32::from_rgb(140, 190, 255)),
                                )
                                .on_hover_text("This pad sent input in the last few seconds");
                            }
                        });
                        ui.label(RichText::new(format!("{} · id {}", pad.power, pad.id)).weak().small());
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!is_active, egui::Button::new("Use as Player 1"))
                            .clicked()
                        {
                            gp.select_pad(pad.id);
                            self.dirty = true;
                            *toast = Some(format!("🎮 {} is now Player 1", pad.name));
                        }

                        let mut chosen = pad.profile.clone();
                        egui::ComboBox::from_id_salt(("pad_profile", pad.uuid.clone()))
                            .width(200.0)
                            .selected_text(chosen.clone())
                            .show_ui(ui, |ui| {
                                for name in &profile_names {
                                    ui.selectable_value(&mut chosen, name.clone(), name);
                                }
                            });
                        if chosen != pad.profile {
                            gp.settings.pad_profiles.insert(pad.uuid.clone(), chosen.clone());
                            if let Some(p) = gp.pads.iter_mut().find(|p| p.uuid == pad.uuid) {
                                p.profile = chosen.clone();
                            }
                            self.dirty = true;
                            *toast = Some(format!("{} → {}", pad.name, chosen));
                        }
                        ui.label("Profile:");
                    });
                });
            });
            ui.add_space(4.0);
        }

        ui.add_space(8.0);
        ui.separator();
        ui.heading("Multiple Controllers");
        let mut merge = gp.settings.merge_all_pads;
        if ui
            .checkbox(
                &mut merge,
                "Accept input from every connected controller (couch co-pilot)",
            )
            .on_hover_text(
                "Off (default): only the Player 1 pad controls the game, so a spare controller \
                 left on the couch cannot drift the character across the screen.\n\
                 On: all pads are merged — handy for handing a second pad to someone.",
            )
            .changed()
        {
            gp.settings.merge_all_pads = merge;
            self.dirty = true;
        }

        ui.add_space(10.0);
        self.input_monitor(ui, gp);
    }

    /// Live view of what the pad is reporting right now.
    fn input_monitor(&mut self, ui: &mut egui::Ui, gp: &mut GamepadManager) {
        ui.separator();
        ui.heading("Live Input Monitor");
        ui.label(RichText::new("Press something on your controller — it lights up here.").weak());
        ui.add_space(4.0);

        let profile = gp.active_profile();
        let held: Vec<PadAction> = PadAction::ALL
            .into_iter()
            .filter(|a| gp.state.held(*a))
            .collect();

        ui.horizontal_wrapped(|ui| {
            if held.is_empty() {
                ui.label(RichText::new("(nothing held)").weak());
            }
            for a in &held {
                ui.label(
                    RichText::new(format!(" {} ", a.label()))
                        .background_color(Color32::from_rgb(40, 80, 55))
                        .color(Color32::from_rgb(170, 245, 185))
                        .strong(),
                );
            }
        });

        if let Some((lx, ly, rx, ry)) = gp.active_stick_raw() {
            ui.add_space(6.0);
            egui::Grid::new("stick_monitor").num_columns(2).show(ui, |ui| {
                ui.label("Left stick:");
                ui.label(format!("X {:+.3}   Y {:+.3}", lx, ly));
                ui.end_row();
                ui.label("Right stick:");
                ui.label(format!("X {:+.3}   Y {:+.3}", rx, ry));
                ui.end_row();
                ui.label("Deadzone:");
                let inside = lx.abs() < profile.deadzone && ly.abs() < profile.deadzone;
                ui.label(
                    RichText::new(format!(
                        "{:.2} — left stick currently {}",
                        profile.deadzone,
                        if inside { "INSIDE (ignored)" } else { "outside (active)" }
                    ))
                    .color(if inside {
                        Color32::GRAY
                    } else {
                        Color32::from_rgb(140, 190, 255)
                    }),
                );
                ui.end_row();
            });
            ui.label(
                RichText::new(
                    "Tip: let go of the sticks. If the numbers above are not ~0.000, raise the \
                     deadzone in Button Mapping until they are ignored.",
                )
                .weak()
                .small(),
            );
        }
    }

    // --- Bindings tab ----------------------------------------------------

    fn bindings_tab(
        &mut self,
        ui: &mut egui::Ui,
        gp: &mut GamepadManager,
        toast: &mut Option<String>,
    ) {
        let profile_name = gp.active_profile_name();
        let Some(profile) = gp.settings.profile(&profile_name).cloned() else {
            ui.label("No profile selected.");
            return;
        };

        // Profile toolbar
        ui.horizontal_wrapped(|ui| {
            ui.label("Editing profile:");
            ui.label(RichText::new(&profile.name).strong());
            if profile.builtin {
                ui.label(
                    RichText::new("built-in")
                        .small()
                        .color(Color32::from_rgb(200, 180, 120)),
                )
                .on_hover_text(
                    "Built-in profiles ship with CrabBoy Advance. Your first edit copies it to \
                     a personal profile so updates can keep improving the original.",
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("🗐 Duplicate").clicked() {
                    let copy = self.fork_profile(gp, &profile);
                    gp.assign_profile(&copy);
                    self.dirty = true;
                    *toast = Some(format!("Created profile “{}”", copy));
                }
                if !profile.builtin && ui.button("✏ Rename").clicked() {
                    self.renaming = true;
                    self.rename_buffer = profile.name.clone();
                }
                if !profile.builtin {
                    let confirming = self.pending == Pending::DeleteProfile(profile.name.clone());
                    let label = if confirming { "⚠ Click again to delete" } else { "🗑 Delete" };
                    if ui.button(label).clicked() {
                        if confirming {
                            self.delete_profile(gp, &profile.name, toast);
                        } else {
                            self.pending = Pending::DeleteProfile(profile.name.clone());
                        }
                    }
                }
                let resetting = self.pending == Pending::ResetProfile(profile.name.clone());
                let rlabel = if resetting { "⚠ Click again to reset" } else { "↺ Reset to defaults" };
                if ui.button(rlabel).clicked() {
                    if resetting {
                        self.reset_profile(gp, &profile.name, toast);
                    } else {
                        self.pending = Pending::ResetProfile(profile.name.clone());
                    }
                }
            });
        });

        if self.renaming {
            ui.horizontal(|ui| {
                ui.label("New name:");
                ui.text_edit_singleline(&mut self.rename_buffer);
                if ui.button("OK").clicked() {
                    let new_name = self.rename_buffer.trim().to_string();
                    if !new_name.is_empty() && gp.settings.profile(&new_name).is_none() {
                        if let Some(p) = gp.settings.profile_mut(&profile.name) {
                            p.name = new_name.clone();
                        }
                        // Repoint every pad that referenced the old name.
                        for v in gp.settings.pad_profiles.values_mut() {
                            if *v == profile.name {
                                *v = new_name.clone();
                            }
                        }
                        if gp.settings.default_profile == profile.name {
                            gp.settings.default_profile = new_name.clone();
                        }
                        for pad in gp.pads.iter_mut() {
                            if pad.profile == profile.name {
                                pad.profile = new_name.clone();
                            }
                        }
                        self.dirty = true;
                    } else {
                        *toast = Some("That profile name is empty or already taken".to_string());
                    }
                    self.renaming = false;
                }
                if ui.button("Cancel").clicked() {
                    self.renaming = false;
                }
            });
        }

        ui.add_space(6.0);

        // Per-profile tuning
        ui.horizontal_wrapped(|ui| {
            let mut layout = profile.layout;
            ui.label("Face buttons labelled:");
            egui::ComboBox::from_id_salt("layout_combo")
                .selected_text(match layout {
                    FaceLayout::Nintendo => "Nintendo / 8BitDo (A on the right)",
                    FaceLayout::Xbox => "Xbox (A on the bottom)",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut layout,
                        FaceLayout::Nintendo,
                        "Nintendo / 8BitDo (A on the right)",
                    );
                    ui.selectable_value(&mut layout, FaceLayout::Xbox, "Xbox (A on the bottom)");
                });
            if layout != profile.layout {
                let target = self.ensure_editable(gp, &profile);
                if let Some(p) = gp.settings.profile_mut(&target) {
                    p.layout = layout;
                }
                self.dirty = true;
            }
        });

        let mut deadzone = profile.deadzone;
        if ui
            .add(egui::Slider::new(&mut deadzone, 0.02..=0.60).text("Stick deadzone"))
            .on_hover_text(
                "How far a stick must move before it counts. Hall-effect sticks (Ultimate 2 Pro) \
                 do not drift, so a low value like 0.18 gives far finer control than the 0.35 \
                 needed by older worn sticks.",
            )
            .changed()
        {
            let target = self.ensure_editable(gp, &profile);
            if let Some(p) = gp.settings.profile_mut(&target) {
                p.deadzone = deadzone;
            }
            self.dirty = true;
        }

        ui.add_space(8.0);
        ui.separator();

        // Conflict banner
        if let Some((input, actions)) = self.last_conflict.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!(
                        "⚠ {} is also bound to: {}",
                        pretty_input(input, profile.layout),
                        actions
                            .iter()
                            .map(|a| a.label())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                    .color(Color32::from_rgb(240, 200, 120)),
                );
                if ui
                    .button("Remove the other bindings")
                    .on_hover_text("Keep only the mapping you just made")
                    .clicked()
                {
                    let target = self.ensure_editable(gp, &profile);
                    if let Some(p) = gp.settings.profile_mut(&target) {
                        for a in &actions {
                            p.unbind(*a, input);
                        }
                    }
                    self.dirty = true;
                    self.last_conflict = None;
                }
                if ui.button("Keep both").clicked() {
                    self.last_conflict = None;
                }
            });
            ui.separator();
        }

        ui.horizontal(|ui| {
            ui.label("🔍");
            ui.text_edit_singleline(&mut self.filter);
            if !self.filter.is_empty() && ui.button("✖").clicked() {
                self.filter.clear();
            }
            ui.label(RichText::new("filter actions").weak());
        });
        ui.add_space(4.0);

        // Binding rows, grouped.
        let filter = self.filter.to_ascii_lowercase();
        let mut last_group = "";
        for action in PadAction::ALL {
            if !filter.is_empty() && !action.label().to_ascii_lowercase().contains(&filter) {
                continue;
            }
            if action.group() != last_group {
                last_group = action.group();
                ui.add_space(8.0);
                ui.label(RichText::new(last_group).strong().size(14.0));
                ui.separator();
            }
            self.binding_row(ui, gp, &profile, action);
        }

        ui.add_space(12.0);
        ui.label(
            RichText::new(
                "Changes save automatically. Hold two buttons together while remapping to bind \
                 a combo (like Select + Start).",
            )
            .weak()
            .small(),
        );
    }

    fn binding_row(
        &mut self,
        ui: &mut egui::Ui,
        gp: &mut GamepadManager,
        profile: &PadProfile,
        action: PadAction,
    ) {
        let live = gp.state.held(action);
        ui.horizontal_wrapped(|ui| {
            let label = RichText::new(format!("{:<22}", action.label()));
            ui.label(if live {
                label.color(Color32::from_rgb(140, 240, 160)).strong()
            } else {
                label
            });

            let binds: Vec<PadInput> = profile.binds(action).to_vec();
            if binds.is_empty() {
                ui.label(RichText::new("unbound").weak().italics());
            }
            for input in &binds {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(pretty_input(*input, profile.layout))
                            .background_color(Color32::from_rgb(38, 42, 52))
                            .color(Color32::from_rgb(210, 220, 240)),
                    );
                    if ui
                        .small_button("✖")
                        .on_hover_text("Remove this binding")
                        .clicked()
                    {
                        let target = self.ensure_editable(gp, profile);
                        if let Some(p) = gp.settings.profile_mut(&target) {
                            p.unbind(action, *input);
                        }
                        self.dirty = true;
                    }
                });
            }

            if ui
                .button("＋ Add")
                .on_hover_text("Listen for a button, stick direction, or two-button combo")
                .clicked()
            {
                gp.begin_capture(action);
            }
        });
    }

    fn capture_prompt(&mut self, ctx: &egui::Context, gp: &mut GamepadManager, action: PadAction) {
        egui::Window::new("listening_for_input")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!("Press an input for “{}”", action.label()))
                            .size(17.0)
                            .strong(),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Button, stick direction, or hold two buttons together for a combo.",
                        )
                        .weak(),
                    );
                    ui.add_space(6.0);

                    match gp.captured_input {
                        Some(input) => ui.label(
                            RichText::new(pretty_input(input, gp.active_profile().layout))
                                .size(16.0)
                                .color(Color32::from_rgb(140, 240, 160))
                                .strong(),
                        ),
                        None => ui.label(RichText::new("waiting…").weak()),
                    };

                    ui.add_space(6.0);
                    let left = gp.capture_seconds_left();
                    ui.add(
                        egui::ProgressBar::new(left / 8.0)
                            .desired_width(240.0)
                            .text(format!("{:.0}s", left.ceil())),
                    );
                    ui.add_space(6.0);
                    if ui.button("Cancel (Esc)").clicked() {
                        gp.cancel_capture();
                    }
                    ui.add_space(6.0);
                });
            });
        // Keep the countdown ticking even when nothing else requests a repaint.
        ctx.request_repaint();
    }

    // --- Guide tab -------------------------------------------------------

    fn guide_tab(&mut self, ui: &mut egui::Ui, gp: &mut GamepadManager) {
        ui.heading("Strategy Guide on the Controller");
        ui.label(
            RichText::new(
                "Open the illustrated guide without touching the keyboard, then read it with \
                 the same pad you are playing with.",
            )
            .weak(),
        );
        ui.add_space(8.0);

        let profile = gp.active_profile();
        egui::Grid::new("guide_binding_summary")
            .striped(true)
            .num_columns(2)
            .show(ui, |ui| {
                for action in [
                    PadAction::GuideToggle,
                    PadAction::GuideScrollUp,
                    PadAction::GuideScrollDown,
                    PadAction::GuidePrevPage,
                    PadAction::GuideNextPage,
                ] {
                    ui.label(action.label());
                    let binds = profile.binds(action);
                    if binds.is_empty() {
                        ui.label(RichText::new("unbound").weak().italics());
                    } else {
                        ui.label(
                            binds
                                .iter()
                                .map(|i| pretty_input(*i, profile.layout))
                                .collect::<Vec<_>>()
                                .join("  ·  "),
                        );
                    }
                    ui.end_row();
                }
            });

        ui.add_space(6.0);
        ui.label(
            RichText::new("Remap any of these in the Button Mapping tab.")
                .weak()
                .small(),
        );

        ui.add_space(12.0);
        ui.separator();

        let mut capture = gp.settings.guide_captures_pad;
        if ui
            .checkbox(
                &mut capture,
                "While the guide is open, the controller drives the guide (recommended)",
            )
            .on_hover_text(
                "On: your pad scrolls and pages the guide, and those presses do not reach the \
                 game — so reading a walkthrough mid-battle cannot make your character walk \
                 into a trainer.\n\
                 Off: the guide only responds to its own bindings and the game keeps receiving \
                 every button.",
            )
            .changed()
        {
            gp.settings.guide_captures_pad = capture;
            self.dirty = true;
        }

        let mut speed = profile.guide_scroll_speed;
        if ui
            .add(egui::Slider::new(&mut speed, 200.0..=2500.0).text("Scroll speed (points/sec)"))
            .on_hover_text("How fast a fully-deflected stick scrolls the page")
            .changed()
        {
            let target = self.ensure_editable(gp, &profile);
            if let Some(p) = gp.settings.profile_mut(&target) {
                p.guide_scroll_speed = speed;
            }
            self.dirty = true;
        }

        ui.add_space(10.0);
        ui.group(|ui| {
            ui.label(RichText::new("How it feels in the hand").strong());
            ui.label(
                "• The stick scrolls proportionally — nudge it to creep, push it to fly.\n\
                 • Holding a page-turn button repeats after a short pause, so one press is \
                 always exactly one page.\n\
                 • The open/close combo is deliberately a two-button chord so it cannot be hit \
                 by accident mid-fight.",
            );
        });
    }

    // --- Profile mutation helpers ---------------------------------------

    /// Returns the name of a profile that is safe to write to, forking a
    /// built-in on first edit.
    fn ensure_editable(&mut self, gp: &mut GamepadManager, profile: &PadProfile) -> String {
        if !profile.builtin {
            return profile.name.clone();
        }
        let name = self.fork_profile(gp, profile);
        gp.assign_profile(&name);
        name
    }

    fn fork_profile(&mut self, gp: &mut GamepadManager, src: &PadProfile) -> String {
        let base = format!("{} (custom)", src.name);
        let mut name = base.clone();
        let mut n = 2;
        while gp.settings.profile(&name).is_some() {
            name = format!("{} {}", base, n);
            n += 1;
        }
        let mut copy = src.clone();
        copy.name = name.clone();
        copy.builtin = false;
        gp.settings.profiles.push(copy);
        self.dirty = true;
        name
    }

    fn apply_binding(
        &mut self,
        gp: &mut GamepadManager,
        action: PadAction,
        input: PadInput,
        toast: &mut Option<String>,
    ) {
        let profile = gp.active_profile();
        let conflicts = profile.conflicts(input, action);
        let target = self.ensure_editable(gp, &profile);
        let layout = profile.layout;
        if let Some(p) = gp.settings.profile_mut(&target) {
            p.bind(action, input);
        }
        self.dirty = true;
        self.pending = Pending::None;
        self.last_conflict = if conflicts.is_empty() {
            None
        } else {
            Some((input, conflicts))
        };
        *toast = Some(format!(
            "{} → {}",
            action.label(),
            pretty_input(input, layout)
        ));
    }

    fn delete_profile(&mut self, gp: &mut GamepadManager, name: &str, toast: &mut Option<String>) {
        gp.settings.profiles.retain(|p| p.name != name);
        gp.settings.pad_profiles.retain(|_, v| v != name);
        gp.settings.reseed_builtins();
        let fallback = gp.settings.default_profile.clone();
        for pad in gp.pads.iter_mut() {
            if pad.profile == name {
                pad.profile = fallback.clone();
            }
        }
        self.pending = Pending::None;
        self.dirty = true;
        *toast = Some(format!("Deleted profile “{}”", name));
    }

    fn reset_profile(&mut self, gp: &mut GamepadManager, name: &str, toast: &mut Option<String>) {
        // Restore from the matching built-in when there is one; otherwise fall
        // back to the Ultimate 2 layout, which is this build's reference pad.
        let stock = PadProfile::builtins()
            .into_iter()
            .find(|b| name.starts_with(&b.name))
            .unwrap_or_else(PadProfile::ultimate2_pro);
        if let Some(p) = gp.settings.profile_mut(name) {
            p.bindings = stock.bindings.clone();
            p.deadzone = stock.deadzone;
            p.layout = stock.layout;
            p.guide_scroll_speed = stock.guide_scroll_speed;
        }
        self.pending = Pending::None;
        self.dirty = true;
        self.last_conflict = None;
        *toast = Some(format!("Reset “{}” to default bindings", name));
    }
}
