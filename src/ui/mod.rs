//! Modern Windows 11 UI/UX Application Implementation

pub mod audio_mixer_dialog;
pub mod bezels;
pub mod cheats_dialog;
pub mod controls;
pub mod debug;
pub mod diorama_renderer;
pub mod gif_recorder;
pub mod guide_dialog;
pub mod link_dialog;
pub mod pokemon_companion;
pub mod rewind;
pub mod save_manager;
pub mod screen;
pub mod screenshot;
pub mod sensors_dialog;
pub mod tas;
pub mod tas_dialog;
pub mod updater;
pub mod updater_dialog;

use audio_mixer_dialog::AudioMixerDialog;
use bezels::{BezelMode, BezelRenderer};
use cheats_dialog::CheatsDialog;
use diorama_renderer::DioramaRenderer;
use gif_recorder::GifRecorder;
use guide_dialog::GuideDialog;
use link_dialog::LinkDialog;
use pokemon_companion::PokemonCompanion;
use sensors_dialog::SensorsDialog;
use tas::{TasEngine, TasMode};
use tas_dialog::TasDialog;
use updater::UpdateManager;
use updater_dialog::UpdaterDialog;

use crate::gba::apu::SurroundMode;
use crate::gba::keypad::Key;
use crate::gba::{Gba, SCREEN_HEIGHT, SCREEN_WIDTH};
use controls::{handle_input, GamepadManager, KeyBindings};
use debug::DebugWindows;
use rewind::RewindManager;
use save_manager::SaveStateManager;
use screen::{DisplayFilter, ScaleMode, ScreenRenderer};

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct GbaApp {
    pub gba: Gba,
    pub screen_renderer: ScreenRenderer,
    pub debug_windows: DebugWindows,
    pub key_bindings: KeyBindings,
    pub gamepad_manager: GamepadManager,
    pub rewind_manager: RewindManager,
    pub save_manager: SaveStateManager,

    // Power Features & Dialogs
    pub cheats_dialog: CheatsDialog,
    pub link_dialog: LinkDialog,
    pub sensors_dialog: SensorsDialog,
    pub pokemon_companion: PokemonCompanion,
    pub gif_recorder: GifRecorder,
    pub bezel_renderer: BezelRenderer,
    pub audio_mixer_dialog: AudioMixerDialog,
    pub tas_engine: TasEngine,
    pub tas_dialog: TasDialog,
    pub guide_dialog: GuideDialog,
    pub updater: UpdateManager,
    pub updater_dialog: UpdaterDialog,

    // Settings
    pub display_filter: DisplayFilter,
    pub nvidia_sharpen: bool,
    pub nvidia_sharpness: f32,
    pub xbrz_factor: usize,
    pub color_correction: bool,
    pub ultrawide_ambient_glow: bool,
    pub scale_mode: ScaleMode,
    pub lock_aspect_ratio: bool,
    pub speed_multiplier: u32,
    pub is_paused: bool,
    pub is_fullscreen: bool,
    pub is_rewinding: bool,
    pub show_fps: bool,
    pub screenshot_enhanced: bool,

    // Timing & Metrics
    fps: f64,
    emulated_frames: u32,
    fps_timer: Instant,
    last_frame_instant: Instant,
    frame_accumulator: Duration,

    // 2.5D / 3D Diorama Mode (Inspired by 3Dsen)
    pub diorama_renderer: DioramaRenderer,
    pub diorama_mode: bool,
    pub show_diorama_controls: bool,

    // UI state
    toast_message: Option<(String, Instant)>,
    show_controls_dialog: bool,
    show_save_manager_dialog: bool,
    show_rtc_dialog: bool,
    pub show_about_dialog: bool,
    loaded_rom_name: String,
}

impl GbaApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial_rom: Option<PathBuf>) -> Self {
        // Configure Windows 11 fluent dark visuals
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(20, 22, 27);
        visuals.window_fill = Color32::from_rgb(28, 30, 36);
        visuals.window_stroke = Stroke::new(1.0_f32, Color32::from_rgb(48, 52, 64));
        cc.egui_ctx.set_visuals(visuals);

        let mut gba = Gba::new();
        let mut loaded_rom_name = "No ROM Loaded".to_string();

        if let Some(ref path) = initial_rom {
            if let Ok(()) = gba.load_rom(path) {
                loaded_rom_name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                log::info!("Successfully loaded initial ROM: {}", loaded_rom_name);
            }
        }

        // Clean up any lingering backup executable from previous updates
        UpdateManager::cleanup_old_exe();

        // Launch asynchronous, non-blocking check for updates on startup
        let updater = UpdateManager::new();
        updater.check_for_updates_async();

        Self {
            gba,
            screen_renderer: ScreenRenderer::new(),
            debug_windows: DebugWindows::default(),
            key_bindings: KeyBindings::default(),
            gamepad_manager: GamepadManager::new(),
            rewind_manager: RewindManager::default(),
            save_manager: SaveStateManager::default(),
            cheats_dialog: CheatsDialog::new(),
            link_dialog: LinkDialog::new(),
            sensors_dialog: SensorsDialog::new(),
            pokemon_companion: PokemonCompanion::new(),
            gif_recorder: GifRecorder::new(),
            bezel_renderer: BezelRenderer::new(),
            audio_mixer_dialog: AudioMixerDialog::new(),
            tas_engine: TasEngine::new(),
            tas_dialog: TasDialog::new(),
            guide_dialog: GuideDialog::new(),
            updater,
            updater_dialog: UpdaterDialog::new(),
            display_filter: DisplayFilter::Crisp,
            nvidia_sharpen: false,
            nvidia_sharpness: 0.6,
            xbrz_factor: 4,
            color_correction: false,
            ultrawide_ambient_glow: true,
            scale_mode: ScaleMode::Scale3x,
            lock_aspect_ratio: true,
            speed_multiplier: 1,
            is_paused: false,
            is_fullscreen: false,
            is_rewinding: false,
            show_fps: true,
            screenshot_enhanced: true,
            fps: 60.0,
            emulated_frames: 0,
            fps_timer: Instant::now(),
            last_frame_instant: Instant::now(),
            frame_accumulator: Duration::ZERO,
            toast_message: Some(("Welcome to GBA Simulator".to_string(), Instant::now())),
            show_controls_dialog: false,
            show_save_manager_dialog: false,
            show_rtc_dialog: false,
            show_about_dialog: false,
            diorama_renderer: DioramaRenderer::new(),
            diorama_mode: false,
            show_diorama_controls: false,
            loaded_rom_name,
        }
    }

    pub fn take_screenshot(&mut self) {
        let (path, filename) = screenshot::generate_screenshot_path(&self.loaded_rom_name, self.screenshot_enhanced);
        let res = if self.screenshot_enhanced {
            screenshot::save_color_image(&path, self.screen_renderer.last_image())
        } else {
            screenshot::save_raw_framebuffer(&path, self.gba.get_framebuffer())
        };

        match res {
            Ok(()) => self.set_toast(format!("📸 Saved {}", filename)),
            Err(e) => self.set_toast(format!("Screenshot error: {}", e)),
        }
    }

    pub fn load_rom_from_path(&mut self, path: &Path) {
        match self.gba.load_rom(path) {
            Ok(()) => {
                self.loaded_rom_name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                self.set_toast(format!("Loaded: {}", self.loaded_rom_name));
            }
            Err(e) => {
                self.set_toast(format!("Failed to load ROM: {}", e));
            }
        }
    }

    pub fn set_toast(&mut self, msg: impl Into<String>) {
        self.toast_message = Some((msg.into(), Instant::now()));
    }
}

impl eframe::App for GbaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Handle drag and drop ROM files
        ctx.input(|i| {
            if let Some(file) = i.raw.dropped_files.first() {
                if let Some(ref path) = file.path {
                    self.load_rom_from_path(path);
                }
            }
        });

        // Keypad inputs (Keyboard + 8BitDo / XInput Gamepad)
        let mut key_events = Vec::new();
        handle_input(ctx, &self.key_bindings, &mut self.gamepad_manager, &mut |key: Key, pressed: bool| {
            key_events.push((key, pressed));
        });
        for (k, p) in key_events {
            self.gba.mmu.keypad.set_key_state(k, p);
        }

        // Fullscreen Toggle (F11 / Alt+Enter / Gamepad Select+Start)
        let wants_fullscreen = ctx.input(|i| {
            i.key_pressed(self.key_bindings.fullscreen)
                || (i.modifiers.alt && i.key_pressed(egui::Key::Enter))
        }) || self.gamepad_manager.fullscreen_pressed;

        if wants_fullscreen {
            self.is_fullscreen = !self.is_fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
            self.set_toast(if self.is_fullscreen {
                "Fullscreen Enabled (F11 / Alt+Enter)"
            } else {
                "Fullscreen Disabled"
            });
        }

        // Screenshot Hotkey (F12)
        if ctx.input(|i| i.key_pressed(self.key_bindings.screenshot)) {
            self.take_screenshot();
        }

        // Save State Manager Dialog (Ctrl+S)
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::S)) {
            self.show_save_manager_dialog = !self.show_save_manager_dialog;
        }

        // Slot Selection Keys (0..=9)
        ctx.input(|i| {
            let number_keys = [
                (egui::Key::Num0, 0),
                (egui::Key::Num1, 1),
                (egui::Key::Num2, 2),
                (egui::Key::Num3, 3),
                (egui::Key::Num4, 4),
                (egui::Key::Num5, 5),
                (egui::Key::Num6, 6),
                (egui::Key::Num7, 7),
                (egui::Key::Num8, 8),
                (egui::Key::Num9, 9),
            ];
            for (k, slot) in number_keys {
                if i.key_pressed(k) && !i.modifiers.ctrl && !i.modifiers.alt {
                    self.save_manager.active_slot = slot;
                    self.set_toast(format!("Selected Save Slot {}", slot));
                    break;
                }
            }
        });

        // Gamepad Hotkeys
        if self.gamepad_manager.pause_button_pressed {
            self.is_paused = !self.is_paused;
            self.set_toast(if self.is_paused { "Paused (Controller)" } else { "Resumed (Controller)" });
        }
        if self.gamepad_manager.quick_save_pressed {
            let slot = self.save_manager.active_slot;
            match self.save_manager.save_slot(slot, &self.gba, &self.loaded_rom_name) {
                Ok(()) => self.set_toast(format!("Saved to Slot {} (Controller)", slot)),
                Err(e) => self.set_toast(e),
            }
        }
        if self.gamepad_manager.quick_load_pressed {
            let slot = self.save_manager.active_slot;
            match self.save_manager.load_slot(slot, &mut self.gba, &self.loaded_rom_name) {
                Ok(()) => self.set_toast(format!("Loaded Slot {} (Controller)", slot)),
                Err(e) => self.set_toast(e),
            }
        }

        // Keyboard Hotkeys
        ctx.input(|i| {
            if i.key_pressed(self.key_bindings.pause) {
                self.is_paused = !self.is_paused;
                self.set_toast(if self.is_paused { "Paused" } else { "Resumed" });
            }
            if i.key_pressed(self.key_bindings.reset) && i.modifiers.ctrl {
                self.gba.reset();
                self.rewind_manager.clear();
                self.set_toast("Reset Emulation");
            }
            if i.key_pressed(self.key_bindings.frame_step) && self.is_paused {
                self.gba.run_frame();
            }
            if i.key_pressed(self.key_bindings.quick_save) {
                let slot = self.save_manager.active_slot;
                match self.save_manager.save_slot(slot, &self.gba, &self.loaded_rom_name) {
                    Ok(()) => self.set_toast(format!("Saved to Slot {} (F5)", slot)),
                    Err(e) => self.set_toast(e),
                }
            }
            if i.key_pressed(self.key_bindings.quick_load) {
                let slot = self.save_manager.active_slot;
                match self.save_manager.load_slot(slot, &mut self.gba, &self.loaded_rom_name) {
                    Ok(()) => self.set_toast(format!("Loaded Slot {} (F8)", slot)),
                    Err(e) => self.set_toast(e),
                }
            }
            // Hotkeys for new power features
            if i.modifiers.ctrl && i.key_pressed(egui::Key::C) {
                self.cheats_dialog.is_open = !self.cheats_dialog.is_open;
            }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::L) {
                self.link_dialog.is_open = !self.link_dialog.is_open;
            }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::P) {
                self.pokemon_companion.is_open = !self.pokemon_companion.is_open;
            }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::M) {
                self.audio_mixer_dialog.is_open = !self.audio_mixer_dialog.is_open;
            }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::Y) {
                self.tas_dialog.is_open = !self.tas_dialog.is_open;
            }
            if i.key_pressed(egui::Key::F1) || (i.modifiers.ctrl && i.key_pressed(egui::Key::H)) {
                self.guide_dialog.is_open = !self.guide_dialog.is_open;
            }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::F12) {
                if self.gif_recorder.is_recording {
                    match self.gif_recorder.stop_and_save(&self.loaded_rom_name) {
                        Ok((_p, name)) => self.set_toast(format!("🎥 Saved GIF: {}", name)),
                        Err(e) => self.set_toast(format!("GIF Error: {}", e)),
                    }
                } else {
                    self.gif_recorder.start_recording();
                    self.set_toast("🔴 Recording Animated GIF (Ctrl+F12 to finish)");
                }
            }
            if i.key_pressed(egui::Key::F6) {
                self.debug_windows.show_ppu = !self.debug_windows.show_ppu;
            }
            if i.key_pressed(egui::Key::F7) {
                self.debug_windows.show_audio = !self.debug_windows.show_audio;
            }
            if i.key_pressed(egui::Key::F8) {
                self.debug_windows.show_diagnostics = !self.debug_windows.show_diagnostics;
            }
            if i.key_pressed(egui::Key::F3) {
                self.diorama_mode = !self.diorama_mode;
                self.gba.set_diorama_enabled(self.diorama_mode);
                self.set_toast(if self.diorama_mode {
                    "🎥 2.5D Diorama Mode Enabled (F3)"
                } else {
                    "📺 Classic 2D Mode Enabled (F3)"
                });
            }
            if self.diorama_mode && i.key_pressed(egui::Key::R) && !i.modifiers.ctrl {
                self.diorama_renderer.camera.reset();
                self.set_toast("Reset 3D Diorama Camera (R)");
            }
            if (i.key_pressed(egui::Key::N) || i.key_pressed(egui::Key::Period)) && self.is_paused {
                self.gba.run_frame();
            }
        });

        // Keep PPU diorama accumulation pipeline in sync with UI mode
        self.gba.set_diorama_enabled(self.diorama_mode);

        // Snapshot / override keypad inputs for TAS
        let keys_pressed_array = [
            self.gba.mmu.keypad.is_key_pressed(Key::A),
            self.gba.mmu.keypad.is_key_pressed(Key::B),
            self.gba.mmu.keypad.is_key_pressed(Key::Select),
            self.gba.mmu.keypad.is_key_pressed(Key::Start),
            self.gba.mmu.keypad.is_key_pressed(Key::Right),
            self.gba.mmu.keypad.is_key_pressed(Key::Left),
            self.gba.mmu.keypad.is_key_pressed(Key::Up),
            self.gba.mmu.keypad.is_key_pressed(Key::Down),
            self.gba.mmu.keypad.is_key_pressed(Key::R),
            self.gba.mmu.keypad.is_key_pressed(Key::L),
        ];

        if self.tas_engine.mode == TasMode::Recording {
            self.tas_engine.record_frame(&keys_pressed_array);
        } else if self.tas_engine.mode == TasMode::Playback {
            if let Some(inputs) = self.tas_engine.get_playback_inputs() {
                self.gba.mmu.keypad.set_key_state(Key::A, inputs[0]);
                self.gba.mmu.keypad.set_key_state(Key::B, inputs[1]);
                self.gba.mmu.keypad.set_key_state(Key::Select, inputs[2]);
                self.gba.mmu.keypad.set_key_state(Key::Start, inputs[3]);
                self.gba.mmu.keypad.set_key_state(Key::Right, inputs[4]);
                self.gba.mmu.keypad.set_key_state(Key::Left, inputs[5]);
                self.gba.mmu.keypad.set_key_state(Key::Up, inputs[6]);
                self.gba.mmu.keypad.set_key_state(Key::Down, inputs[7]);
                self.gba.mmu.keypad.set_key_state(Key::R, inputs[8]);
                self.gba.mmu.keypad.set_key_state(Key::L, inputs[9]);
            } else if self.tas_engine.pause_on_finish {
                self.is_paused = true;
                self.set_toast("TAS Playback Complete");
            }
        }

        // Turbo and Rewind States
        let wants_rewind = ctx.input(|i| i.key_down(self.key_bindings.rewind)) || self.gamepad_manager.rewind_button_down;
        self.is_rewinding = wants_rewind && !self.is_paused;

        let is_turbo = ctx.input(|i| i.key_down(self.key_bindings.turbo)) || self.gamepad_manager.turbo_button_pressed;
        let effective_speed = if is_turbo { 4 } else { self.speed_multiplier };

        // Fixed-Time Accumulator for hardware-accurate 59.7275 Hz / 60 FPS frame pacing
        let now = Instant::now();
        let delta = now.duration_since(self.last_frame_instant).min(Duration::from_millis(100));
        self.last_frame_instant = now;

        let target_frame_duration = Duration::from_secs_f64(1.0 / (59.7275 * effective_speed as f64));
        let mut frames_run = 0;

        if !self.is_paused {
            self.frame_accumulator += delta;

            if self.is_rewinding {
                while self.frame_accumulator >= target_frame_duration && frames_run < 4 {
                    self.rewind_manager.rewind_step(&mut self.gba);
                    self.frame_accumulator -= target_frame_duration;
                    frames_run += 1;
                }
            } else {
                while self.frame_accumulator >= target_frame_duration && frames_run < 4 {
                    self.gba.run_frame();
                    self.rewind_manager.record_frame(&self.gba);
                    self.gif_recorder.capture_frame(self.gba.get_framebuffer());
                    self.frame_accumulator -= target_frame_duration;
                    frames_run += 1;
                    self.emulated_frames += 1;
                }
            }

            if self.frame_accumulator > target_frame_duration * 2 {
                self.frame_accumulator = Duration::ZERO;
            }
        }

        self.gba.mmu.apu.audio_output.set_fast_forwarding(is_turbo || effective_speed > 1);

        // Poll Pokémon party periodically if companion is active
        if self.pokemon_companion.is_open && self.emulated_frames.is_multiple_of(30) {
            if let Some(ref cart) = self.gba.mmu.cartridge {
                self.pokemon_companion.poll_party_memory(&self.gba.mmu, &cart.game_code);
            }
        }

        // Update FPS calculation from true emulated GBA frames
        let elapsed = self.fps_timer.elapsed().as_secs_f64();
        if elapsed >= 0.5 {
            self.fps = (self.emulated_frames as f64) / elapsed;
            self.emulated_frames = 0;
            self.fps_timer = Instant::now();
        }

        // Auto-hiding menu bar in Fullscreen mode (revealed when mouse cursor is near top)
        let show_menu_bar = !self.is_fullscreen || ctx.input(|i| {
            i.pointer.hover_pos().is_some_and(|pos| pos.y < 35.0)
        });

        if show_menu_bar {
            egui::TopBottomPanel::top("top_menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open ROM...").clicked() {
                        ui.close_menu();
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("GBA ROM", &["gba", "bin"])
                            .pick_file()
                        {
                            self.load_rom_from_path(&path);
                        }
                    }
                    if ui.button("Reset (Ctrl+R)").clicked() {
                        ui.close_menu();
                        self.gba.reset();
                        self.rewind_manager.clear();
                        self.set_toast("Emulation Reset");
                    }
                    ui.separator();
                    if ui.button(format!("Quick Save State - Slot {} (F5)", self.save_manager.active_slot)).clicked() {
                        ui.close_menu();
                        let slot = self.save_manager.active_slot;
                        match self.save_manager.save_slot(slot, &self.gba, &self.loaded_rom_name) {
                            Ok(()) => self.set_toast(format!("Saved to Slot {}", slot)),
                            Err(e) => self.set_toast(e),
                        }
                    }
                    if ui.button(format!("Quick Load State - Slot {} (F8)", self.save_manager.active_slot)).clicked() {
                        ui.close_menu();
                        let slot = self.save_manager.active_slot;
                        match self.save_manager.load_slot(slot, &mut self.gba, &self.loaded_rom_name) {
                            Ok(()) => self.set_toast(format!("Loaded Slot {}", slot)),
                            Err(e) => self.set_toast(e),
                        }
                    }
                    if ui.button("Save State Manager (Ctrl+S)...").clicked() {
                        ui.close_menu();
                        self.show_save_manager_dialog = true;
                    }
                    ui.separator();
                    if ui.button("Take Screenshot (F12)").clicked() {
                        ui.close_menu();
                        self.take_screenshot();
                    }
                    let fs_label = if self.is_fullscreen { "Exit Fullscreen (F11 / Alt+Enter)" } else { "Fullscreen Mode (F11 / Alt+Enter)" };
                    if ui.button(fs_label).clicked() {
                        ui.close_menu();
                        self.is_fullscreen = !self.is_fullscreen;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
                        self.set_toast(if self.is_fullscreen { "Fullscreen Enabled" } else { "Fullscreen Disabled" });
                    }
                    ui.separator();
                    if ui.button("Save Battery (.sav)").clicked() {
                        ui.close_menu();
                        if let Some(ref mut cart) = self.gba.mmu.cartridge {
                            cart.flash.sync_to_disk();
                            self.set_toast("Battery Save Synced to Disk");
                        }
                    }
                    ui.separator();
                    if ui.button("Exit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                ui.menu_button("Emulation", |ui| {
                    let pause_label = if self.is_paused { "Resume (P)" } else { "Pause (P)" };
                    if ui.button(pause_label).clicked() {
                        self.is_paused = !self.is_paused;
                        ui.close_menu();
                    }
                    if ui.button("Frame Step (F)").clicked() {
                        if self.is_paused {
                            self.gba.run_frame();
                        }
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.menu_button(format!("Active Save Slot: {}", self.save_manager.active_slot), |ui| {
                        for slot in 0..=9 {
                            if ui.radio_value(&mut self.save_manager.active_slot, slot, format!("Slot {}", slot)).clicked() {
                                self.set_toast(format!("Selected Save Slot {}", slot));
                                ui.close_menu();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("Real-Time Clock (RTC) Settings...").clicked() {
                        ui.close_menu();
                        self.show_rtc_dialog = true;
                    }
                    ui.separator();
                    ui.label("Speed:");
                    if ui.radio_value(&mut self.speed_multiplier, 1, "1x (Normal 60 FPS)").clicked() { ui.close_menu(); }
                    if ui.radio_value(&mut self.speed_multiplier, 2, "2x Fast Forward").clicked() { ui.close_menu(); }
                    if ui.radio_value(&mut self.speed_multiplier, 4, "4x Turbo").clicked() { ui.close_menu(); }
                });

                ui.menu_button("Video", |ui| {
                    ui.label("Filter / Scaler:");
                    ui.radio_value(&mut self.display_filter, DisplayFilter::Crisp, "Crisp Pixel (Nearest)");
                    ui.radio_value(&mut self.display_filter, DisplayFilter::Xbrz, "xBRZ High-Definition (AI Edge Smoothing)");
                    if self.display_filter == DisplayFilter::Xbrz {
                        ui.indent("xbrz_selector", |ui| {
                            ui.label("xBRZ Scale Factor:");
                            ui.radio_value(&mut self.xbrz_factor, 2, "2x (480p)");
                            ui.radio_value(&mut self.xbrz_factor, 3, "3x (720p)");
                            ui.radio_value(&mut self.xbrz_factor, 4, "4x (960p - Recommended)");
                            ui.radio_value(&mut self.xbrz_factor, 5, "5x (1200p)");
                            ui.radio_value(&mut self.xbrz_factor, 6, "6x (1440p HD)");
                        });
                    }
                    ui.radio_value(&mut self.display_filter, DisplayFilter::Linear, "Smooth (Bilinear)");
                    ui.radio_value(&mut self.display_filter, DisplayFilter::LcdGrid, "Retro LCD Grid");
                    ui.radio_value(&mut self.display_filter, DisplayFilter::CrtScanlines, "CRT Scanlines");
                    ui.separator();
                    ui.label("Enhancements & Post-Processing:");
                    ui.checkbox(&mut self.nvidia_sharpen, "NVIDIA Adaptive Sharpening (NIS / CAS)");
                    if self.nvidia_sharpen || self.display_filter == DisplayFilter::NvidiaSharpen {
                        ui.indent("sharpen_slider", |ui| {
                            ui.add(egui::Slider::new(&mut self.nvidia_sharpness, 0.0..=1.0).text("Sharpness Strength"));
                        });
                    }
                    ui.checkbox(&mut self.color_correction, "Authentic GBA LCD Color Correction");
                    ui.checkbox(&mut self.ultrawide_ambient_glow, "Ultrawide Ambient Edge Glow");
                    ui.separator();
                    ui.label("Scale Preset (4K / Ultrawide):");
                    ui.radio_value(&mut self.scale_mode, ScaleMode::IntegerAuto, "Auto Integer (Pixel-Perfect)");
                    ui.radio_value(&mut self.scale_mode, ScaleMode::Scale1x, "1x (240x160)");
                    ui.radio_value(&mut self.scale_mode, ScaleMode::Scale2x, "2x (480x320)");
                    ui.radio_value(&mut self.scale_mode, ScaleMode::Scale3x, "3x (720x480)");
                    ui.radio_value(&mut self.scale_mode, ScaleMode::Scale4x, "4x (960x640)");
                    ui.radio_value(&mut self.scale_mode, ScaleMode::Scale5x, "5x (1200x800)");
                    ui.radio_value(&mut self.scale_mode, ScaleMode::Scale6x, "6x (1440x960)");
                    ui.radio_value(&mut self.scale_mode, ScaleMode::Scale8x, "8x (1920x1280)");
                    ui.radio_value(&mut self.scale_mode, ScaleMode::Scale10x, "10x (2400x1600 - 1600p Ultrawide)");
                    ui.radio_value(&mut self.scale_mode, ScaleMode::Scale12x, "12x (2880x1920)");
                    ui.radio_value(&mut self.scale_mode, ScaleMode::Scale14x, "14x (3360x2240 - 4K Fill)");
                    ui.radio_value(&mut self.scale_mode, ScaleMode::Fit, "Fit to Window");
                    ui.separator();
                    ui.label("Console Bezel / Frame:");
                    ui.radio_value(&mut self.bezel_renderer.mode, BezelMode::None, "None (Clean Screen)");
                    ui.radio_value(&mut self.bezel_renderer.mode, BezelMode::GbaClassicIndigo, "GBA Classic (Indigo Purple)");
                    ui.radio_value(&mut self.bezel_renderer.mode, BezelMode::GbaClassicGlacier, "GBA Classic (Glacier Ice)");
                    ui.radio_value(&mut self.bezel_renderer.mode, BezelMode::GbaSpFlameRed, "GBA SP (Flame Red)");
                    ui.radio_value(&mut self.bezel_renderer.mode, BezelMode::GameBoyPlayer, "Game Boy Player (GameCube)");
                    ui.separator();
                    ui.checkbox(&mut self.lock_aspect_ratio, "Lock Aspect Ratio (3:2)");
                    ui.checkbox(&mut self.screenshot_enhanced, "Screenshot: Capture Enhanced (xBRZ/NIS) vs Raw 1x");
                    ui.separator();
                    ui.label("🎥 2.5D / 3D Diorama Mode (3Dsen Style):");
                    if ui.checkbox(&mut self.diorama_mode, "Enable 3D Diorama Mode (F3)").changed() {
                        self.gba.set_diorama_enabled(self.diorama_mode);
                        self.set_toast(if self.diorama_mode { "🎥 2.5D Diorama Mode Enabled (F3)" } else { "📺 Classic 2D Mode Enabled (F3)" });
                    }
                    if ui.button("3D Diorama Settings & Perspective...").clicked() {
                        self.show_diorama_controls = true;
                        ui.close_menu();
                    }
                    if self.diorama_mode && ui.button("Reset 3D Camera (R)").clicked() {
                        self.diorama_renderer.camera.reset();
                        ui.close_menu();
                    }
                });

                ui.menu_button("Audio", |ui| {
                    if ui.button("6-Channel Sound Mixer & Turbo DSP (Ctrl+M)...").clicked() {
                        self.audio_mixer_dialog.is_open = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.checkbox(&mut self.gba.mmu.apu.audio_output.muted, "Mute Audio");
                    ui.add(egui::Slider::new(&mut self.gba.mmu.apu.audio_output.volume, 0.0..=1.0).text("Volume"));
                    ui.separator();
                    ui.label("Spatial Audio & Headphone Surround:");
                    let mut mode = self.gba.mmu.apu.audio_output.surround_mode();
                    if ui.radio_value(&mut mode, SurroundMode::Headphone3D, "3D Binaural Headphone Spatializer").changed() {
                        self.gba.mmu.apu.audio_output.set_surround_mode(mode);
                    }
                    if ui.radio_value(&mut mode, SurroundMode::Surround51, "5.1 Surround Sound Matrix Upmixer").changed() {
                        self.gba.mmu.apu.audio_output.set_surround_mode(mode);
                    }
                    if ui.radio_value(&mut mode, SurroundMode::Stereo, "Direct Stereo (2.0)").changed() {
                        self.gba.mmu.apu.audio_output.set_surround_mode(mode);
                    }
                    ui.separator();
                    ui.label("Subwoofer & Ambience DSP:");
                    let mut bass = self.gba.mmu.apu.audio_output.bass_boost();
                    if ui.add(egui::Slider::new(&mut bass, 0.0..=1.0).text("Subwoofer Bass Boost")).changed() {
                        self.gba.mmu.apu.audio_output.set_bass_boost(bass);
                    }
                    let mut width = self.gba.mmu.apu.audio_output.surround_width();
                    if ui.add(egui::Slider::new(&mut width, 0.0..=1.0).text("Surround Ambience Width")).changed() {
                        self.gba.mmu.apu.audio_output.set_surround_width(width);
                    }
                    let hw_channels = self.gba.mmu.apu.audio_output.hardware_channels();
                    ui.label(RichText::new(format!("Hardware Channels Detected: {}", hw_channels)).weak().small());
                });

                ui.menu_button("Tools", |ui| {
                    if ui.button("📜 Cheats & RAM Searcher (Ctrl+C)...").clicked() {
                        self.cheats_dialog.is_open = true;
                        ui.close_menu();
                    }
                    if ui.button("🔗 Link Cable & Multiplayer (Ctrl+L)...").clicked() {
                        self.link_dialog.is_open = true;
                        ui.close_menu();
                    }
                    if ui.button("🐾 Pokémon Gen 3 Companion (Ctrl+P)...").clicked() {
                        self.pokemon_companion.is_open = true;
                        ui.close_menu();
                    }
                    let rec_label = if self.gif_recorder.is_recording {
                        "⏹ Stop Recording GIF (Ctrl+F12)"
                    } else {
                        "🎥 Record Animated GIF (Ctrl+F12)"
                    };
                    if ui.button(rec_label).clicked() {
                        ui.close_menu();
                        if self.gif_recorder.is_recording {
                            match self.gif_recorder.stop_and_save(&self.loaded_rom_name) {
                                Ok((_p, name)) => self.set_toast(format!("🎥 Saved GIF: {}", name)),
                                Err(e) => self.set_toast(format!("GIF Error: {}", e)),
                            }
                        } else {
                            self.gif_recorder.start_recording();
                            self.set_toast("🔴 Recording Animated GIF");
                        }
                    }
                    if ui.button("⏱ TAS Speedrun Engine (Ctrl+Y)...").clicked() {
                        self.tas_dialog.is_open = true;
                        ui.close_menu();
                    }
                    if ui.button("🎮 Hardware Sensors (Solar/Tilt/Rumble)...").clicked() {
                        self.sensors_dialog.is_open = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("📖 Pokémon Trainer's Strategy Guide (F1)...").clicked() {
                        self.guide_dialog.is_open = true;
                        ui.close_menu();
                    }
                });

                ui.menu_button("Controls", |ui| {
                    if ui.button("Configure Controls & Gamepad...").clicked() {
                        self.show_controls_dialog = true;
                        ui.close_menu();
                    }
                });

                ui.menu_button("Help", |ui| {
                    if ui.button("📖 Trainer's Strategy Guide & Manual (F1)...").clicked() {
                        self.guide_dialog.is_open = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("📥 Open Full PDF in Default Viewer...").clicked() {
                        ui.close_menu();
                        match self.guide_dialog.open_external_pdf() {
                            Ok(_) => self.set_toast("📖 Opened Trainer's Field Manual in PDF Viewer"),
                            Err(e) => self.set_toast(format!("Error: {}", e)),
                        }
                    }
                    if ui.button("💾 Export PDF Manual to Disk...").clicked() {
                        ui.close_menu();
                        match self.guide_dialog.export_pdf_as() {
                            Ok(Some(path)) => self.set_toast(format!("Saved: {}", path.file_name().unwrap_or_default().to_string_lossy())),
                            Ok(None) => {},
                            Err(e) => self.set_toast(format!("Export error: {}", e)),
                        }
                    }
                    ui.separator();
                    ui.menu_button("Jump to Chapter", |ui| {
                        let chapters = [
                            ("Cover Page", 0),
                            ("Ch 1: Welcome & Quick Start", 1),
                            ("Ch 2: Controls & Joypad", 2),
                            ("Ch 3: Visual Filters & Shaders", 3),
                            ("Ch 4: Live Pokémon Companion", 4),
                            ("Ch 5: RTC, Solar & Sensors", 5),
                            ("Ch 6: SIO Link & Audio Gym", 6),
                            ("Ch 7: TAS & Troubleshooting", 7),
                        ];
                        for (title, page) in chapters {
                            if ui.button(title).clicked() {
                                self.guide_dialog.open_at_page(page);
                                ui.close_menu();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("ℹ About CrabBoy Advance...").clicked() {
                        self.show_about_dialog = true;
                        ui.close_menu();
                    }
                    if ui.button("🔄 Check for Updates...").clicked() {
                        self.updater_dialog.is_open = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.label(RichText::new(format!("CrabBoy Advance v{}", env!("CARGO_PKG_VERSION"))).weak().small());
                    ui.label(RichText::new("Cycle-Accurate 32-Bit GBA Engine").weak().small());
                    ui.label(RichText::new("Built with Rust & egui").weak().small());
                });

                // Software Update Menu Tab
                let update_status_peek = {
                    let lock = self.updater.status.lock().unwrap();
                    lock.clone()
                };
                let has_update = matches!(
                    update_status_peek,
                    updater::UpdateStatus::UpdateAvailable { .. }
                        | updater::UpdateStatus::DownloadedReadyToRestart { .. }
                );

                let update_label = if has_update {
                    RichText::new("🔄 Update (New!)")
                        .color(Color32::from_rgb(255, 200, 60))
                        .strong()
                } else {
                    RichText::new("🔄 Update")
                };

                ui.menu_button(update_label, |ui| {
                    if ui.button("🔄 Open Software Update Center...").clicked() {
                        self.updater_dialog.is_open = true;
                        ui.close_menu();
                    }
                    if ui.button("🔍 Check for Updates Now").clicked() {
                        self.updater.check_for_updates_async();
                        self.set_toast("Checking GitHub for updates...");
                        ui.close_menu();
                    }
                    ui.separator();
                    match &update_status_peek {
                        updater::UpdateStatus::Idle => {
                            ui.label(RichText::new("Status: Not checked yet").weak().small());
                        }
                        updater::UpdateStatus::Checking => {
                            ui.label(RichText::new("Status: Checking GitHub...").color(Color32::LIGHT_BLUE).small());
                        }
                        updater::UpdateStatus::UpToDate { version, .. } => {
                            ui.label(RichText::new(format!("Status: Up to date (v{})", version)).color(Color32::LIGHT_GREEN).small());
                        }
                        updater::UpdateStatus::UpdateAvailable { latest, .. } => {
                            ui.label(RichText::new(format!("🎉 New version available: v{}", latest.version)).color(Color32::from_rgb(255, 200, 60)).strong().small());
                            if ui.button(RichText::new("📥 Update Now...").strong().color(Color32::WHITE)).clicked() {
                                self.updater_dialog.is_open = true;
                                ui.close_menu();
                            }
                        }
                        updater::UpdateStatus::Downloading { progress, .. } => {
                            ui.label(RichText::new(format!("Status: Downloading ({:.0}%)", progress * 100.0)).color(Color32::LIGHT_BLUE).small());
                        }
                        updater::UpdateStatus::DownloadedReadyToRestart { latest, .. } => {
                            ui.label(RichText::new(format!("✨ Ready to restart (v{})", latest.version)).color(Color32::LIGHT_GREEN).strong().small());
                            if ui.button(RichText::new("🔄 Restart CrabBoy Advance").strong().color(Color32::WHITE)).clicked() {
                                if let Err(e) = self.updater.restart_and_apply() {
                                    self.set_toast(format!("Restart error: {}", e));
                                }
                                ui.close_menu();
                            }
                        }
                        updater::UpdateStatus::Failed(_) => {
                            ui.label(RichText::new("Status: Check failed (Offline)").color(Color32::LIGHT_RED).small());
                        }
                    }
                });

                ui.menu_button("Debug Tools", |ui| {
                    ui.checkbox(&mut self.debug_windows.show_diagnostics, "🔬 System Diagnostics Hub (F8)");
                    ui.checkbox(&mut self.debug_windows.show_audio, "APU Audio Inspector & Oscilloscope (F7)");
                    ui.checkbox(&mut self.debug_windows.show_ppu, "PPU Layers & OAM Inspector (F6)");
                    ui.checkbox(&mut self.debug_windows.show_memory, "Memory Hex Viewer & Watchpoints (Ctrl+M)");
                    ui.checkbox(&mut self.debug_windows.show_cpu, "ARM7TDMI CPU Inspector");
                    ui.checkbox(&mut self.debug_windows.show_palette, "Palette RAM Viewer");
                    ui.separator();
                    ui.checkbox(&mut self.show_fps, "Show FPS Overlay");
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if has_update {
                        let btn = egui::Button::new(
                            RichText::new("🎉 New Update Available!").color(Color32::BLACK).strong()
                        ).fill(Color32::from_rgb(255, 200, 60));
                        if ui.add(btn).clicked() {
                            self.updater_dialog.is_open = true;
                        }
                    }

                    if self.show_fps {
                        let fps_color = if self.fps >= 55.0 { Color32::GREEN } else { Color32::YELLOW };
                        ui.label(RichText::new(format!("{:.1} FPS", self.fps)).color(fps_color).monospace());
                    }
                    if is_turbo {
                        ui.label(RichText::new("[TURBO 4X]").color(Color32::from_rgb(255, 100, 100)).strong());
                    }
                    if self.diorama_mode {
                        ui.label(RichText::new("[3D DIORAMA]").color(Color32::from_rgb(80, 200, 255)).strong());
                    }

                    // Gamepad connection status badge
                    if let Some(ref gp_name) = self.gamepad_manager.connected_gamepad_name {
                        let short_name = if gp_name.len() > 24 { &gp_name[..24] } else { gp_name };
                        ui.label(RichText::new(format!("🎮 {}", short_name)).color(Color32::from_rgb(100, 220, 100)).small());
                    } else {
                        ui.label(RichText::new("🎮 No Controller").color(Color32::DARK_GRAY).small());
                    }

                    ui.label(RichText::new(&self.loaded_rom_name).color(Color32::LIGHT_GRAY).small());
                });
            });
        });
        }

        // Debug Floating Windows
        self.debug_windows.show(ctx, &mut self.gba);

        // Keybindings & Gamepad Dialog
        if self.show_controls_dialog {
            egui::Window::new("Controls & Gamepad Configuration")
                .open(&mut self.show_controls_dialog)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.heading("🎮 Gamepad (8BitDo & XInput)");
                    ui.separator();
                    if let Some(ref name) = self.gamepad_manager.connected_gamepad_name {
                        ui.label(RichText::new(format!("Connected: {}", name)).color(Color32::GREEN).strong());
                    } else {
                        ui.label(RichText::new("No controller detected. Plug in your 8BitDo controller (2.4GHz dongle, Bluetooth, or USB).").color(Color32::YELLOW));
                    }

                    ui.add_space(6.0);
                    ui.label("Face Button Layout:");
                    ui.radio_value(&mut self.gamepad_manager.swap_ab, false, "8BitDo / Nintendo Layout (East=A, South=B) [Recommended]");
                    ui.radio_value(&mut self.gamepad_manager.swap_ab, true, "Xbox Standard Layout (South=A, East=B)");

                    ui.add_space(4.0);
                    ui.add(egui::Slider::new(&mut self.gamepad_manager.deadzone, 0.1..=0.8).text("Analog Stick Deadzone"));

                    ui.add_space(8.0);
                    ui.label(RichText::new("Controller Mappings:").strong());
                    egui::Grid::new("gamepad_mapping_grid").striped(true).show(ui, |ui| {
                        ui.label("Movement:"); ui.label("D-Pad or Left Analog Stick"); ui.end_row();
                        ui.label("A / B Buttons:"); ui.label("Physical A / B (or X / Y as alternates)"); ui.end_row();
                        ui.label("L / R Shoulders:"); ui.label("Bumpers (L1/R1) or Triggers (L2/R2)"); ui.end_row();
                        ui.label("Start / Select:"); ui.label("Plus (+) and Minus (-) buttons"); ui.end_row();
                        ui.label("Turbo 4x:"); ui.label("Right Stick Click (R3)"); ui.end_row();
                        ui.label("Quick Save:"); ui.label("Left Stick Click (L3)"); ui.end_row();
                        ui.label("Pause:"); ui.label("Home / Guide button"); ui.end_row();
                    });

                    ui.add_space(12.0);
                    ui.heading("⌨ Keyboard Controls");
                    ui.separator();
                    egui::Grid::new("controls_grid").striped(true).show(ui, |ui| {
                        ui.label("D-Pad:"); ui.label("Arrow Keys"); ui.end_row();
                        ui.label("A Button:"); ui.label("Z"); ui.end_row();
                        ui.label("B Button:"); ui.label("X"); ui.end_row();
                        ui.label("L Shoulder:"); ui.label("A"); ui.end_row();
                        ui.label("R Shoulder:"); ui.label("S"); ui.end_row();
                        ui.label("Start:"); ui.label("Enter"); ui.end_row();
                        ui.label("Select:"); ui.label("Backspace"); ui.end_row();
                        ui.label("Turbo (Hold):"); ui.label("Space"); ui.end_row();
                        ui.label("Pause:"); ui.label("P"); ui.end_row();
                        ui.label("Quick Save / Load:"); ui.label("F5 / F8"); ui.end_row();
                        ui.label("Reset:"); ui.label("Ctrl+R"); ui.end_row();
                    });

                    ui.add_space(8.0);
                    if ui.button("📖 Open Illustrated Strategy Guide & Manual (F1)").clicked() {
                        self.guide_dialog.open_at_page(2);
                    }
                });
        }

        // 2.5D / 3D Diorama Settings Floating Window
        if self.show_diorama_controls {
            egui::Window::new("🎥 2.5D / 3D Diorama Settings")
                .open(&mut self.show_diorama_controls)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.heading("🎥 3D Camera & Perspective");
                    ui.separator();
                    ui.add(egui::Slider::new(&mut self.diorama_renderer.camera.yaw, -std::f32::consts::PI..=std::f32::consts::PI).text("Camera Yaw"));
                    ui.add(egui::Slider::new(&mut self.diorama_renderer.camera.pitch, -1.35..=1.35).text("Camera Pitch"));
                    ui.add(egui::Slider::new(&mut self.diorama_renderer.camera.distance, 120.0..=750.0).text("Camera Zoom"));
                    if ui.button("↺ Reset Camera to Default (R)").clicked() {
                        self.diorama_renderer.camera.reset();
                    }

                    ui.add_space(8.0);
                    ui.heading("📦 Layer Depths & Extrusions");
                    ui.separator();
                    ui.add(egui::Slider::new(&mut self.diorama_renderer.layer_depth_separation, 5.0..=45.0).text("Layer 3D Depth Spacing"));
                    ui.add(egui::Slider::new(&mut self.diorama_renderer.sprite_depth_elevation, 5.0..=50.0).text("Sprite 3D Pop-Out"));
                    ui.add(egui::Slider::new(&mut self.diorama_renderer.voxel_relief_strength, 0.0..=1.0).text("Voxel Tile Relief"));

                    ui.add_space(8.0);
                    ui.heading("✨ Aesthetics & Effects");
                    ui.separator();
                    ui.checkbox(&mut self.diorama_renderer.show_layer_shadows, "Layer & Sprite Drop Shadows");
                    ui.checkbox(&mut self.diorama_renderer.show_diorama_grid, "Diorama Pedestal Floor");
                });
        }

        // Central Panel (GBA LCD Viewport)
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::from_rgb(14, 15, 18)))
            .show(ctx, |ui| {
                let available_size = ui.available_size();

                // 2.5D / 3D Diorama Mode Viewport
                if self.diorama_mode {
                    let (viewport_rect, response) = ui.allocate_exact_size(available_size, Sense::drag());

                    if response.dragged() {
                        let delta = response.drag_delta();
                        if ui.input(|i| i.modifiers.shift) {
                            self.diorama_renderer.camera.pan(delta.x, delta.y);
                        } else {
                            self.diorama_renderer.camera.rotate(delta.x, delta.y);
                        }
                    }

                    let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
                    if scroll_delta.abs() > 0.1 {
                        self.diorama_renderer.camera.zoom(scroll_delta * 0.04);
                    }

                    self.diorama_renderer.render(
                        ui,
                        ctx,
                        viewport_rect,
                        self.gba.get_diorama_data(),
                    );

                    // Floating HUD overlay in top-left of diorama viewport
                    let hud_rect = Rect::from_min_size(
                        viewport_rect.min + Vec2::new(14.0, 14.0),
                        Vec2::new(280.0, 56.0),
                    );
                    ui.painter().rect_filled(
                        hud_rect,
                        6.0_f32,
                        Color32::from_rgba_unmultiplied(18, 20, 26, 215),
                    );
                    ui.painter().rect_stroke(
                        hud_rect,
                        6.0_f32,
                        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(80, 200, 255, 90)),
                        egui::StrokeKind::Outside,
                    );
                    ui.painter().text(
                        hud_rect.min + Vec2::new(10.0, 8.0),
                        egui::Align2::LEFT_TOP,
                        "🎥 2.5D Diorama Mode Active (F3)",
                        egui::FontId::proportional(12.5),
                        Color32::from_rgb(100, 220, 255),
                    );
                    ui.painter().text(
                        hud_rect.min + Vec2::new(10.0, 26.0),
                        egui::Align2::LEFT_TOP,
                        "Drag mouse to orbit | Scroll to zoom | R to reset",
                        egui::FontId::proportional(10.5),
                        Color32::from_rgb(180, 190, 210),
                    );
                    ui.painter().text(
                        hud_rect.min + Vec2::new(10.0, 40.0),
                        egui::Align2::LEFT_TOP,
                        format!(
                            "Yaw: {:.1}°  Pitch: {:.1}°  Zoom: {:.0}",
                            self.diorama_renderer.camera.yaw.to_degrees(),
                            self.diorama_renderer.camera.pitch.to_degrees(),
                            self.diorama_renderer.camera.distance
                        ),
                        egui::FontId::monospace(10.0),
                        Color32::from_rgb(140, 210, 140),
                    );

                    // Toast Notification Overlay
                    if let Some((ref msg, time)) = self.toast_message {
                        if time.elapsed().as_secs_f32() < 3.0 {
                            let toast_rect = Rect::from_min_size(
                                viewport_rect.min + Vec2::new(14.0, 80.0),
                                Vec2::new(280.0, 32.0),
                            );
                            ui.painter().rect_filled(
                                toast_rect,
                                6.0_f32,
                                Color32::from_rgba_unmultiplied(20, 22, 28, 220),
                            );
                            ui.painter().rect_stroke(
                                toast_rect,
                                6.0_f32,
                                Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 40)),
                                egui::StrokeKind::Outside,
                            );
                            ui.painter().text(
                                toast_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                msg,
                                egui::FontId::proportional(13.0),
                                Color32::WHITE,
                            );
                        }
                    }
                    return;
                }

                let gba_aspect = (SCREEN_WIDTH as f32) / (SCREEN_HEIGHT as f32);

                let target_size = match self.scale_mode {
                    ScaleMode::Scale1x => Vec2::new(SCREEN_WIDTH as f32, SCREEN_HEIGHT as f32),
                    ScaleMode::Scale2x => Vec2::new(SCREEN_WIDTH as f32 * 2.0, SCREEN_HEIGHT as f32 * 2.0),
                    ScaleMode::Scale3x => Vec2::new(SCREEN_WIDTH as f32 * 3.0, SCREEN_HEIGHT as f32 * 3.0),
                    ScaleMode::Scale4x => Vec2::new(SCREEN_WIDTH as f32 * 4.0, SCREEN_HEIGHT as f32 * 4.0),
                    ScaleMode::Scale5x => Vec2::new(SCREEN_WIDTH as f32 * 5.0, SCREEN_HEIGHT as f32 * 5.0),
                    ScaleMode::Scale6x => Vec2::new(SCREEN_WIDTH as f32 * 6.0, SCREEN_HEIGHT as f32 * 6.0),
                    ScaleMode::Scale8x => Vec2::new(SCREEN_WIDTH as f32 * 8.0, SCREEN_HEIGHT as f32 * 8.0),
                    ScaleMode::Scale10x => Vec2::new(SCREEN_WIDTH as f32 * 10.0, SCREEN_HEIGHT as f32 * 10.0),
                    ScaleMode::Scale12x => Vec2::new(SCREEN_WIDTH as f32 * 12.0, SCREEN_HEIGHT as f32 * 12.0),
                    ScaleMode::Scale14x => Vec2::new(SCREEN_WIDTH as f32 * 14.0, SCREEN_HEIGHT as f32 * 14.0),
                    ScaleMode::IntegerAuto => {
                        let max_w = (available_size.x / SCREEN_WIDTH as f32).floor() as u32;
                        let max_h = (available_size.y / SCREEN_HEIGHT as f32).floor() as u32;
                        let scale = max_w.min(max_h).max(1);
                        Vec2::new(SCREEN_WIDTH as f32 * scale as f32, SCREEN_HEIGHT as f32 * scale as f32)
                    }
                    ScaleMode::Fit => {
                        if self.lock_aspect_ratio {
                            let w_by_h = available_size.y * gba_aspect;
                            if w_by_h <= available_size.x {
                                Vec2::new(w_by_h, available_size.y)
                            } else {
                                Vec2::new(available_size.x, available_size.x / gba_aspect)
                            }
                        } else {
                            available_size
                        }
                    }
                };

                let tex = self.screen_renderer.update_framebuffer(
                    ctx,
                    self.gba.get_framebuffer(),
                    self.display_filter,
                    self.nvidia_sharpen,
                    self.nvidia_sharpness,
                    self.color_correction,
                    self.xbrz_factor,
                );

                let x_offset = (available_size.x - target_size.x).max(0.0) / 2.0;
                let y_offset = (available_size.y - target_size.y).max(0.0) / 2.0;

                // Ultrawide Ambient Lighting / Edge Glow Backdrop
                if self.ultrawide_ambient_glow && x_offset > 16.0 {
                    let fb = self.gba.get_framebuffer();
                    let mut lr = 0u32; let mut lg = 0u32; let mut lb = 0u32;
                    let mut rr = 0u32; let mut rg = 0u32; let mut rb = 0u32;
                    for step in 0..16 {
                        let y = step * 10;
                        let left_p = fb[y * 240];
                        let right_p = fb[y * 240 + 239];
                        lr += left_p & 0xFF; lg += (left_p >> 8) & 0xFF; lb += (left_p >> 16) & 0xFF;
                        rr += right_p & 0xFF; rg += (right_p >> 8) & 0xFF; rb += (right_p >> 16) & 0xFF;
                    }
                    let left_col = Color32::from_rgba_unmultiplied((lr / 16) as u8, (lg / 16) as u8, (lb / 16) as u8, 38);
                    let right_col = Color32::from_rgba_unmultiplied((rr / 16) as u8, (rg / 16) as u8, (rb / 16) as u8, 38);

                    let left_rect = Rect::from_min_max(
                        ui.min_rect().min,
                        Pos2::new(ui.min_rect().min.x + x_offset, ui.min_rect().min.y + available_size.y),
                    );
                    let right_rect = Rect::from_min_max(
                        Pos2::new(ui.min_rect().min.x + x_offset + target_size.x, ui.min_rect().min.y),
                        Pos2::new(ui.min_rect().min.x + available_size.x, ui.min_rect().min.y + available_size.y),
                    );

                    ui.painter().rect_filled(left_rect, 0.0, left_col);
                    ui.painter().rect_filled(right_rect, 0.0, right_col);

                    // Subtle diffuse glow behind game screen
                    let glow_rect = Rect::from_min_size(
                        ui.min_rect().min + Vec2::new(x_offset - 8.0, y_offset - 8.0),
                        target_size + Vec2::new(16.0, 16.0),
                    );
                    ui.painter().rect_filled(glow_rect, 6.0, Color32::from_rgba_unmultiplied(
                        ((lr + rr) / 32) as u8,
                        ((lg + rg) / 32) as u8,
                        ((lb + rb) / 32) as u8,
                        25,
                    ));
                }

                ui.allocate_new_ui(
                    egui::UiBuilder::new().max_rect(Rect::from_min_size(ui.min_rect().min + Vec2::new(x_offset, y_offset), target_size)),
                    |ui| {
                        let (rect, _response) = ui.allocate_exact_size(target_size, Sense::hover());
                        ui.painter().image(
                            tex.id(),
                            rect,
                            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );

                        // Retro Handheld Bezel / Frame
                        self.bezel_renderer.render_bezel(ui, rect, &keys_pressed_array);

                        // Subtle bezel shadow border
                        ui.painter().rect_stroke(
                            rect,
                            4.0_f32,
                            Stroke::new(1.5_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 30)),
                            egui::StrokeKind::Outside,
                        );

                        // Toast Notification Overlay
                        if let Some((ref msg, time)) = self.toast_message {
                            if time.elapsed().as_secs_f32() < 3.0 {
                                let toast_rect = Rect::from_min_size(
                                    rect.min + Vec2::new(12.0, 12.0),
                                    Vec2::new(280.0, 32.0),
                                );
                                ui.painter().rect_filled(
                                    toast_rect,
                                    6.0_f32,
                                    Color32::from_rgba_unmultiplied(20, 22, 28, 220),
                                );
                                ui.painter().rect_stroke(
                                    toast_rect,
                                    6.0_f32,
                                    Stroke::new(1.0_f32, Color32::from_rgb(0, 120, 215)),
                                    egui::StrokeKind::Outside,
                                );
                                ui.painter().text(
                                    toast_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    msg,
                                    egui::FontId::proportional(13.0),
                                    Color32::WHITE,
                                );
                            }
                        }

                        // GIF Recording HUD indicator
                        if self.gif_recorder.is_recording {
                            let gif_rect = Rect::from_min_size(
                                rect.max - Vec2::new(180.0, 42.0),
                                Vec2::new(170.0, 30.0),
                            );
                            ui.painter().rect_filled(gif_rect, 6.0, Color32::from_rgba_unmultiplied(220, 40, 40, 230));
                            ui.painter().rect_stroke(gif_rect, 6.0, Stroke::new(1.0_f32, Color32::WHITE), egui::StrokeKind::Outside);
                            ui.painter().text(
                                gif_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                format!("● REC ({} frames)", self.gif_recorder.frames_recorded),
                                egui::FontId::proportional(12.0),
                                Color32::WHITE,
                            );
                        }

                        // TAS Engine HUD indicator
                        if self.tas_engine.mode == TasMode::Playback {
                            let tas_rect = Rect::from_min_size(
                                rect.min + Vec2::new(12.0, 52.0),
                                Vec2::new(170.0, 28.0),
                            );
                            ui.painter().rect_filled(tas_rect, 6.0, Color32::from_rgba_unmultiplied(40, 160, 60, 230));
                            ui.painter().rect_stroke(tas_rect, 6.0, Stroke::new(1.0_f32, Color32::LIGHT_GREEN), egui::StrokeKind::Outside);
                            ui.painter().text(
                                tas_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                format!("▶ TAS ({}/{})", self.tas_engine.playback_frame, self.tas_engine.recorded_inputs.len()),
                                egui::FontId::proportional(12.0),
                                Color32::WHITE,
                            );
                        } else if self.tas_engine.mode == TasMode::Recording {
                            let tas_rect = Rect::from_min_size(
                                rect.min + Vec2::new(12.0, 52.0),
                                Vec2::new(170.0, 28.0),
                            );
                            ui.painter().rect_filled(tas_rect, 6.0, Color32::from_rgba_unmultiplied(180, 50, 50, 230));
                            ui.painter().rect_stroke(tas_rect, 6.0, Stroke::new(1.0_f32, Color32::from_rgb(255, 100, 100)), egui::StrokeKind::Outside);
                            ui.painter().text(
                                tas_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                format!("🔴 TAS REC ({})", self.tas_engine.recorded_inputs.len()),
                                egui::FontId::proportional(12.0),
                                Color32::WHITE,
                            );
                        }

                        // Rewinding HUD overlay indicator
                        if self.is_rewinding {
                            let avail = self.rewind_manager.available_seconds();
                            let badge_rect = Rect::from_min_size(
                                rect.min + Vec2::new(12.0, 12.0),
                                Vec2::new(190.0, 32.0),
                            );
                            ui.painter().rect_filled(badge_rect, 6.0_f32, Color32::from_rgba_unmultiplied(200, 100, 20, 230));
                            ui.painter().rect_stroke(badge_rect, 6.0_f32, Stroke::new(1.0_f32, Color32::from_rgb(255, 180, 40)), egui::StrokeKind::Outside);
                            ui.painter().text(
                                badge_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                format!("⏪ REWINDING ({:.1}s)", avail),
                                egui::FontId::proportional(13.0),
                                Color32::WHITE,
                            );
                        }

                        // Paused overlay indicator
                        if self.is_paused && !self.is_rewinding {
                            ui.painter().rect_filled(
                                rect,
                                0.0,
                                Color32::from_rgba_unmultiplied(0, 0, 0, 120),
                            );
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "PAUSED",
                                egui::FontId::proportional(32.0),
                                Color32::WHITE,
                            );
                        }
                    },
                );
            });

        // Save State Manager Dialog
        let mut show_save_manager = self.show_save_manager_dialog;
        if show_save_manager {
            let mut action_toast = None;
            egui::Window::new("Save State Manager")
                .open(&mut show_save_manager)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.heading("💾 Multi-Slot Save State Manager");
                    ui.label("10 dedicated persistent save slots backed to disk in `saves/`.");
                    ui.separator();

                    for slot in 0..=9 {
                        let meta = self.save_manager.get_slot_metadata(slot, &self.loaded_rom_name);
                        let is_active = self.save_manager.active_slot == slot;

                        ui.horizontal(|ui| {
                            let slot_label = if is_active {
                                format!("▶ Slot {} [ACTIVE]", slot)
                            } else {
                                format!("  Slot {}", slot)
                            };

                            if ui.selectable_label(is_active, slot_label).clicked() {
                                self.save_manager.active_slot = slot;
                                action_toast = Some(format!("Active Slot set to {}", slot));
                            }

                            if meta.exists {
                                ui.label(RichText::new(format!("{} ({} KB)", meta.modified_str, meta.size_bytes / 1024)).color(Color32::LIGHT_GREEN));
                                if ui.button("Load").clicked() {
                                    match self.save_manager.load_slot(slot, &mut self.gba, &self.loaded_rom_name) {
                                        Ok(()) => action_toast = Some(format!("Loaded Slot {}", slot)),
                                        Err(e) => action_toast = Some(e),
                                    }
                                }
                                if ui.button("Overwrite").clicked() {
                                    match self.save_manager.save_slot(slot, &self.gba, &self.loaded_rom_name) {
                                        Ok(()) => action_toast = Some(format!("Overwrote Slot {}", slot)),
                                        Err(e) => action_toast = Some(e),
                                    }
                                }
                                if ui.button("Delete").clicked() {
                                    self.save_manager.delete_slot(slot, &self.loaded_rom_name);
                                    action_toast = Some(format!("Deleted Slot {}", slot));
                                }
                            } else {
                                ui.label(RichText::new("[Empty]").weak());
                                if ui.button("Save").clicked() {
                                    match self.save_manager.save_slot(slot, &self.gba, &self.loaded_rom_name) {
                                        Ok(()) => action_toast = Some(format!("Saved to Slot {}", slot)),
                                        Err(e) => action_toast = Some(e),
                                    }
                                }
                            }
                        });
                    }
                });
            self.show_save_manager_dialog = show_save_manager;
            if let Some(msg) = action_toast {
                self.set_toast(msg);
            }
        }

        // Real-Time Clock (RTC) Time-Travel Dialog
        let mut show_rtc = self.show_rtc_dialog;
        if show_rtc {
            let mut rtc_toast = None;
            egui::Window::new("Real-Time Clock (RTC) Time-Travel Chamber")
                .open(&mut show_rtc)
                .resizable(false)
                .show(ctx, |ui| {
                    let has_rtc = self.gba.mmu.cartridge.as_ref().is_some_and(|c| c.has_rtc);
                    if !has_rtc {
                        ui.label(RichText::new("The currently loaded ROM does not contain an RTC hardware chip.").color(Color32::YELLOW));
                        ui.label("RTC features are supported in cartridges with integrated Real-Time Clock hardware.");
                    } else if let Some(ref mut cart) = self.gba.mmu.cartridge {
                        let (year, month, day, hrs, mins, secs, dow) = cart.rtc.get_datetime_components();
                        ui.label(RichText::new("Virtual Game Clock:").strong());
                        ui.label(
                            RichText::new(format!(
                                "{:04}-{:02}-{:02} {:02}:{:02}:{:02} ({})",
                                year, month, day, hrs, mins, secs, dow
                            ))
                            .monospace()
                            .size(18.0)
                            .color(Color32::from_rgb(100, 200, 255)),
                        );

                        let offset_hrs = cart.rtc.time_offset_secs as f64 / 3600.0;
                        ui.label(RichText::new(format!("Active Time Offset: {:+.1} hours", offset_hrs)).weak());
                        ui.separator();

                        ui.label(RichText::new("Time-Travel Chamber:").strong());
                        ui.horizontal(|ui| {
                            if ui.button("+1 Hour (Shoal Cave Tides)").clicked() {
                                cart.rtc.add_offset_secs(3600);
                                rtc_toast = Some("⏳ RTC advanced +1 Hour".to_string());
                            }
                            if ui.button("+6 Hours (Day/Night Shift)").clicked() {
                                cart.rtc.add_offset_secs(3600 * 6);
                                rtc_toast = Some("⏳ RTC advanced +6 Hours (Day/Night)".to_string());
                            }
                        });
                        ui.horizontal(|ui| {
                            if ui.button("+24 Hours (Berry Growth Stage +1)").clicked() {
                                cart.rtc.add_offset_secs(86400);
                                rtc_toast = Some("⏳ RTC advanced +24 Hours (Daily Events & Berries Updated)".to_string());
                            }
                            if ui.button("+1 Week (Full Crop Harvest)").clicked() {
                                cart.rtc.add_offset_secs(86400 * 7);
                                rtc_toast = Some("⏳ RTC advanced +1 Week".to_string());
                            }
                        });
                        ui.separator();
                        if ui.button("Sync with Real PC Clock").clicked() {
                            cart.rtc.reset_offset();
                            rtc_toast = Some("RTC Synced to Real System Clock".to_string());
                        }
                    }
                });
            self.show_rtc_dialog = show_rtc;
            if let Some(msg) = rtc_toast {
                self.set_toast(msg);
            }
        }

        // New Feature Floating Dialogs
        let mut dialog_toast = None;
        self.cheats_dialog.show(ctx, &mut self.gba, &mut dialog_toast);
        self.link_dialog.show(ctx, &mut self.gba, &mut dialog_toast);
        self.sensors_dialog.show(ctx, &mut self.gba, &mut dialog_toast);
        self.pokemon_companion.show(ctx, &self.gba);
        self.audio_mixer_dialog.show(ctx, &mut self.gba, &mut dialog_toast);
        self.tas_dialog.show(ctx, &mut self.tas_engine, &mut self.gba, &mut dialog_toast);
        self.guide_dialog.show(ctx, &mut dialog_toast);
        self.updater_dialog.show(ctx, &self.updater, &mut dialog_toast);

        if self.show_about_dialog {
            let mut close_about = false;
            let mut open_guide = false;
            let mut show_about = self.show_about_dialog;

            egui::Window::new("About CrabBoy Advance")
                .open(&mut show_about)
                .resizable(false)
                .collapsible(false)
                .default_width(380.0)
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(4.0);
                        ui.heading(RichText::new("🦀 CrabBoy Advance").size(20.0).strong().color(Color32::from_rgb(255, 110, 60)));
                        ui.label(RichText::new(format!("v{} • Cycle-Accurate 32-Bit GBA Engine", env!("CARGO_PKG_VERSION"))).weak().small());
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(6.0);

                        egui::Grid::new("about_features_grid").striped(true).show(ui, |ui| {
                            ui.label("Core:"); ui.label("16.78 MHz ARM7TDMI with Waitstates & Pipelining"); ui.end_row();
                            ui.label("Audio:"); ui.label("5.1 Surround Downmix & 6-Channel Sound Gym"); ui.end_row();
                            ui.label("Video:"); ui.label("xBRZ HD, CRT Scanlines & GBA LCD Subpixel Shaders"); ui.end_row();
                            ui.label("SIO Netplay:"); ui.label("Local UDP Loopback Trading & Union Room"); ui.end_row();
                            ui.label("Companion:"); ui.label("Live Pokémon IV/EV & Shiny PID Telemetry"); ui.end_row();
                            ui.label("Sensors:"); ui.label("Hardware RTC, Boktai Solar Sensor & 2-Axis Gyro"); ui.end_row();
                            ui.label("Speedrun:"); ui.label("Deterministic TAS Engine (.ctas) & Frame Rewind"); ui.end_row();
                        });

                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(6.0);

                        ui.horizontal(|ui| {
                            if ui.button("📖 Trainer's Strategy Guide (F1)").clicked() {
                                open_guide = true;
                                close_about = true;
                            }
                            if ui.button("Close").clicked() {
                                close_about = true;
                            }
                        });
                        ui.add_space(4.0);
                    });
                });

            if close_about {
                show_about = false;
            }
            if open_guide {
                self.guide_dialog.is_open = true;
            }
            self.show_about_dialog = show_about;
        }

        if let Some(msg) = dialog_toast {
            self.set_toast(msg);
        }

        // Request repaint precisely when the next frame is due for locked 60 FPS pacing
        let time_until_next = target_frame_duration.saturating_sub(self.frame_accumulator);
        ctx.request_repaint_after(time_until_next);
    }
}
