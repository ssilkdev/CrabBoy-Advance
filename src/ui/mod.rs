//! Modern Windows 11 UI/UX Application Implementation

pub mod accessibility_dialog;
pub mod ai_agent;
pub mod ai_agent_dialog;
pub mod audio_mixer_dialog;
pub mod bezels;
pub mod cheats_dialog;
pub mod config;
pub mod controls;
pub mod controls_dialog;
pub mod debug;
pub mod emu_core;
pub mod game_guide;
pub mod gif_recorder;
pub mod guide_dialog;
pub mod link_dialog;
pub mod memmap_dialog;
pub mod platform;
pub mod pokemon_companion;
pub mod rewind;
pub mod save_manager;
pub mod save_sync_dialog;
pub mod screen;
pub mod screenshot;
pub mod sensors_dialog;
pub mod tas;
pub mod tas_dialog;
pub mod updater;
pub mod updater_dialog;
pub mod settings_window;
pub mod web_guide;

use accessibility_dialog::AccessibilityDialog;
use memmap_dialog::MemoryMapDialog;
use ai_agent::AiAgent;
use ai_agent_dialog::AiAgentDialog;
use audio_mixer_dialog::AudioMixerDialog;
use bezels::BezelRenderer;
use cheats_dialog::CheatsDialog;
use gif_recorder::GifRecorder;
use guide_dialog::{GuideDialog, GuidePadInput};
use link_dialog::LinkDialog;
use pokemon_companion::PokemonCompanion;
use save_sync_dialog::SaveSyncDialog;
use sensors_dialog::SensorsDialog;
use tas::{TasEngine, TasMode};
use tas_dialog::TasDialog;
use updater::UpdateManager;
use updater_dialog::UpdaterDialog;

use crate::dmg::mmu::GbKey;
use crate::dmg::GameBoy;
use crate::gba::keypad::Key;
use crate::gba::Gba;
use emu_core::ConsoleKind;
use config::AppConfig;
use controls::{handle_input, GamepadManager, KeyBindings, PadAction};
use controls_dialog::ControlsDialog;
use debug::DebugWindows;
use rewind::RewindManager;
use save_manager::SaveStateManager;
use screen::{AspectRatio, DisplayFilter, ScaleMode, ScreenRenderer};

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct GbaApp {
    pub gba: Gba,
    /// Game Boy / Game Boy Color core, live only while a .gb/.gbc ROM is
    /// loaded. The GBA core stays resident but idle so every dialog that
    /// borrows `&mut self.gba` keeps compiling unchanged.
    pub gb: Option<GameBoy>,
    pub console: ConsoleKind,
    /// GB output letterboxed into a GBA-sized buffer, so the screen
    /// renderer, GIF recorder, screenshots and the ambient-glow sampler all
    /// consume one framebuffer shape regardless of the active core.
    gb_framebuffer: Box<[u32; 240 * 160]>,
    /// Run CGB-enhanced cartridges in original Game Boy mode. Takes effect
    /// on the next ROM load, since the model is fixed at construction.
    pub gb_force_dmg: bool,
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
    pub ai_agent: AiAgent,
    pub ai_agent_dialog: AiAgentDialog,
    pub guide_dialog: GuideDialog,
    pub controls_dialog: ControlsDialog,
    pub updater: UpdateManager,
    pub updater_dialog: UpdaterDialog,
    pub save_sync_dialog: SaveSyncDialog,
    pub accessibility_dialog: AccessibilityDialog,
    /// Per-game memory map and guided variable discovery (ROADMAP M11).
    pub memmap_dialog: MemoryMapDialog,
    /// The Settings window (display, emulation and audio settings).
    pub settings_window: settings_window::SettingsWindow,
    /// Path of the ROM last opened, for "Reload ROM".
    pub loaded_rom_path: Option<PathBuf>,
    /// Colorblind filter, slow motion, sticky buttons, one-handed layout and
    /// UI scale, per game (ROADMAP M10).
    pub accessibility: crate::gba::accessibility::AccessibilityManager,

    // Settings
    pub run_ahead: crate::gba::run_ahead::RunAhead,
    pub run_ahead_config: config::RunAheadSettings,
    pub save_sync_config: config::SaveSyncConfig,
    /// Periodic auto-save rotation (separate from the manual slots).
    pub autosaver: crate::autosave::AutoSaver,
    pub frame_blend_mode: crate::gba::frame_blend::FrameBlendMode,
    pub custom_shader_path: Option<PathBuf>,
    pub hd_mode7_config: crate::gba::ppu::hd_mode7::HdMode7Config,
    pub hd_pack_path: Option<PathBuf>,
    pub hd_pack_enabled: bool,
    pub widescreen_config: crate::gba::widescreen::WidescreenConfig,
    pub display_filter: DisplayFilter,
    pub nvidia_sharpen: bool,
    pub nvidia_sharpness: f32,
    pub xbrz_factor: usize,
    pub color_correction: bool,
    pub ultrawide_ambient_glow: bool,
    pub scale_mode: ScaleMode,
    pub aspect_ratio: AspectRatio,
    pub speed_multiplier: u32,
    pub is_paused: bool,
    pub is_fullscreen: bool,
    pub is_rewinding: bool,
    pub show_fps: bool,
    pub screenshot_enhanced: bool,

    // Timing & Metrics
    fps: f64,
    frame_pacer: crate::frame_pacing::FramePacer,
    emulated_frames: u32,
    fps_timer: Instant,
    last_frame_instant: Instant,
    frame_accumulator: Duration,

    // UI state
    toast_message: Option<(String, Instant)>,
    /// Set whenever a controller setting changed; debounced and flushed to
    /// config.json so a remap survives a crash, not just a clean exit.
    config_dirty: bool,
    last_config_flush: Instant,
    show_save_manager_dialog: bool,
    show_rtc_dialog: bool,
    pub show_about_dialog: bool,
    pub loaded_rom_name: String,
}

impl GbaApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial_rom: Option<PathBuf>) -> Self {
        // Configure Windows 11 fluent dark visuals
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(20, 22, 27);
        visuals.window_fill = Color32::from_rgb(28, 30, 36);
        visuals.window_stroke = Stroke::new(1.0_f32, Color32::from_rgb(48, 52, 64));
        cc.egui_ctx.set_visuals(visuals);

        // The ROM (of either console) is loaded after construction through
        // load_rom_from_path, so there is exactly one dispatch site deciding
        // which core a file goes to.
        let mut gba = Gba::new();
        let loaded_rom_name = "No ROM Loaded".to_string();

        // Persisted controller mappings and keyboard bindings. Load failures
        // are non-fatal by design: a broken config must never stop the
        // emulator from starting, it just falls back to stock bindings.
        let config = AppConfig::load();

        // Clean up any lingering backup executable from previous updates
        UpdateManager::cleanup_old_exe();

        // Launch asynchronous, non-blocking check for updates on startup
        let updater = UpdateManager::new();
        updater.check_for_updates_async();

        let run_ahead_config = config.run_ahead.clone();
        let run_ahead = crate::gba::run_ahead::RunAhead::new(
            run_ahead_config.default_frames,
            run_ahead_config.second_instance,
        );

            let frame_blend_mode = match config.render.frame_blend.to_lowercase().as_str() {
                "simple50" | "50" => crate::gba::frame_blend::FrameBlendMode::Simple50,
                "smart" | "deflicker" => crate::gba::frame_blend::FrameBlendMode::SmartDeFlicker,
                "lcd" | "ghosting" => crate::gba::frame_blend::FrameBlendMode::LcdGhosting { decay: 0.65 },
                _ => crate::gba::frame_blend::FrameBlendMode::Off,
            };
            let display_filter = match config.render.filter.to_lowercase().as_str() {
                "linear" => DisplayFilter::Linear,
                "lcdgrid" => DisplayFilter::LcdGrid,
                "lcdsubpixel" => DisplayFilter::LcdSubpixel,
                "crtscanlines" => DisplayFilter::CrtScanlines,
                "crtgeom" => DisplayFilter::CrtGeom,
                "nvidiasharpen" => DisplayFilter::NvidiaSharpen,
                "xbrz" => DisplayFilter::Xbrz,
                "custom" => DisplayFilter::Custom,
                _ => DisplayFilter::Crisp,
            };
            let mut screen_renderer = ScreenRenderer::new();
            if let Some(ref path) = config.render.custom_shader_path {
                if let Ok(content) = std::fs::read_to_string(path) {
                    if let Ok(params) = crate::gba::shader::CustomShaderParams::parse(&content) {
                        screen_renderer.custom_shader = Some(params);
                    }
                }
            }

            let hd_mode7_config = config.render.hd_mode7;
            gba.set_hd_mode7_config(hd_mode7_config);

            let hd_pack_path = config.render.hd_pack_path.clone();
            let hd_pack_enabled = config.render.hd_pack_enabled;
            if let Some(ref path) = hd_pack_path {
                if let Ok(pack) = crate::gba::hd_pack::HdPack::load_from_dir(path) {
                    gba.load_hd_pack(pack);
                    gba.set_hd_pack_enabled(hd_pack_enabled);
                }
            }

            let widescreen_config = config.render.widescreen.clone();
            gba.set_widescreen_config(widescreen_config.clone());

            let audio_mode = match config.audio.mode.to_lowercase().as_str() {
                "hardware" | "hardwareonly" | "off" => crate::gba::m4a::AudioEngineMode::HardwareOnly,
                _ => crate::gba::m4a::AudioEngineMode::HdReSynthesis,
            };
            gba.set_hd_audio_mode(audio_mode);

            let audio_interp = match config.audio.interpolation.to_lowercase().as_str() {
                "linear" => crate::gba::m4a::M4aInterpolation::Linear,
                "sinc" => crate::gba::m4a::M4aInterpolation::Sinc,
                _ => crate::gba::m4a::M4aInterpolation::CubicHermite,
            };
            gba.m4a.config.interpolation = audio_interp;
            gba.m4a.sampler.interpolation = audio_interp;
            gba.m4a.config.reverb_enabled = config.audio.reverb_enabled;
            gba.m4a.sampler.reverb_enabled = config.audio.reverb_enabled;
            gba.m4a.config.reverb_level = config.audio.reverb_level;
            gba.m4a.sampler.reverb_level = config.audio.reverb_level;
            gba.m4a.config.master_volume = config.audio.master_volume;

            let accessibility = crate::gba::accessibility::AccessibilityManager::with_store(config.accessibility.clone());
            cc.egui_ctx.set_zoom_factor(accessibility.active.ui_scale_clamped());

            let mut app = Self {
            gba,
            gb: None,
            console: ConsoleKind::Gba,
            gb_framebuffer: Box::new([0xFF00_0000; 240 * 160]),
            gb_force_dmg: false,
            screen_renderer,
            debug_windows: DebugWindows::default(),
            key_bindings: config.keyboard.clone(),
            gamepad_manager: GamepadManager::new(config.controllers.clone()),
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
            ai_agent: AiAgent::new(),
            ai_agent_dialog: AiAgentDialog::new(),
            guide_dialog: GuideDialog::new(),
            controls_dialog: ControlsDialog::new(),
            updater,
            updater_dialog: UpdaterDialog::new(),
            save_sync_dialog: SaveSyncDialog::new(),
            accessibility_dialog: AccessibilityDialog::new(),
            memmap_dialog: MemoryMapDialog::new(),
            settings_window: Default::default(),
            loaded_rom_path: None,
            accessibility,
            run_ahead,
            run_ahead_config,
            save_sync_config: config.save_sync.clone(),
            autosaver: crate::autosave::AutoSaver::new(
                "saves",
                config.autosave.enabled,
                config.autosave.interval_minutes,
            ),
            frame_blend_mode,
            custom_shader_path: config.render.custom_shader_path.clone(),
            hd_mode7_config,
            hd_pack_path,
            hd_pack_enabled,
            widescreen_config,
            display_filter,
            nvidia_sharpen: false,
            nvidia_sharpness: 0.6,
            xbrz_factor: 4,
            color_correction: false,
            ultrawide_ambient_glow: true,
            scale_mode: ScaleMode::Scale3x,
            aspect_ratio: AspectRatio::Native,
            speed_multiplier: 1,
            is_paused: false,
            is_fullscreen: false,
            is_rewinding: false,
            show_fps: true,
            screenshot_enhanced: true,
            fps: 60.0,
            frame_pacer: crate::frame_pacing::FramePacer::default(),
            emulated_frames: 0,
            fps_timer: Instant::now(),
            last_frame_instant: Instant::now(),
            frame_accumulator: Duration::ZERO,
            toast_message: Some(("Welcome to GBA Simulator".to_string(), Instant::now())),
            config_dirty: false,
            last_config_flush: Instant::now(),
            show_save_manager_dialog: false,
            show_rtc_dialog: false,
            show_about_dialog: false,
            loaded_rom_name,
        };

        if let Some(ref path) = initial_rom {
            app.load_rom_from_path(path);
        }

        // Seed config.json on first ever launch. Writing it eagerly (rather
        // than waiting for the user's first remap) means the file is there to
        // be found, inspected and hand-edited, and it surfaces a
        // permissions problem immediately instead of at the moment the user
        // finally remaps something and expects it to stick.
        if config::config_path().map(|p| !p.exists()).unwrap_or(false) {
            app.flush_config();
        }

        app
    }

    pub fn take_screenshot(&mut self) {
        let (path, filename) = screenshot::generate_screenshot_path(&self.loaded_rom_name, self.screenshot_enhanced);
        let res = if self.screenshot_enhanced {
            screenshot::save_color_image(&path, self.screen_renderer.last_image())
        } else {
            screenshot::save_raw_framebuffer(&path, self.display_framebuffer())
        };

        match res {
            Ok(()) => self.set_toast(format!("📸 Saved {}", filename)),
            Err(e) => self.set_toast(format!("Screenshot error: {}", e)),
        }
    }

    /// Single dispatch point for loading a ROM of either console. The file
    /// extension picks the core (see `ConsoleKind::from_extension`); a
    /// failed load leaves the previously running game untouched.
    pub fn load_rom_from_path(&mut self, path: &Path) {
        self.loaded_rom_path = Some(path.to_path_buf());
        let kind = ConsoleKind::from_extension(path);
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        match kind {
            ConsoleKind::GameBoy => match GameBoy::from_file(path, self.gb_force_dmg) {
                Ok(gb) => {
                    let model = if gb.is_cgb() { "Game Boy Color" } else { "Game Boy" };
                    let title = gb.mmu.cart.title.clone();
                    self.accessibility.load_for_game("", &title);
                    self.gb = Some(gb);
                    self.console = ConsoleKind::GameBoy;
                    self.loaded_rom_name = name;
                    self.rewind_manager.clear();
                    self.autosaver.restart_timer();
                    self.set_toast(format!("Loaded {} ({}): {}", self.loaded_rom_name, model, title));
                }
                Err(e) => self.set_toast(format!("Failed to load GB ROM: {}", e)),
            },
            ConsoleKind::Gba => {
                if let Some(parent) = path.parent() {
                    let dirs = vec![PathBuf::from("saves"), parent.to_path_buf()];
                    if let Some(ref sync_dir) = self.save_sync_config.sync_dir {
                        if self.save_sync_config.enabled {
                            let syncer = crate::gba::save_sync::SaveSync::new(sync_dir, dirs)
                                .with_max_backups(self.save_sync_config.max_backups);
                            syncer.sync_all();
                        }
                    }
                }
                match self.gba.load_rom(path) {
                    Ok(()) => {
                        self.gb = None;
                        self.console = ConsoleKind::Gba;
                        self.loaded_rom_name = name;
                        self.rewind_manager.clear();
                        self.autosaver.restart_timer();
                        self.widescreen_config = self.gba.widescreen_config().clone();
                        self.update_run_ahead_for_rom();
                        if let Some(cart) = self.gba.mmu.cartridge.as_ref() {
                            let (code, title) = (cart.game_code.clone(), cart.title.clone());
                            self.accessibility.load_for_game(&code, &title);
                            if let Some(msg) = self.memmap_dialog.on_game_loaded(&code, &title) {
                                log::info!("{msg}");
                            }
                        }
                        self.sync_saves();
                        self.set_toast(format!("Loaded: {}", self.loaded_rom_name));
                    }
                    Err(e) => self.set_toast(format!("Failed to load ROM: {}", e)),
                }
            }
        }
    }

    /// True while a Game Boy / Game Boy Color ROM is the active core.
    pub fn is_gb_mode(&self) -> bool {
        self.gb.is_some()
    }

    /// Refresh `gb_framebuffer` from the Game Boy core. Called once per UI
    /// pass before anything reads the display buffer, so consumers can take
    /// a plain field reference (which keeps egui's disjoint-borrow patterns
    /// working) instead of a &mut self accessor.
    fn sync_display_framebuffer(&mut self) {
        if let Some(ref gb) = self.gb {
            let src = gb.framebuffer_gba_sized();
            self.gb_framebuffer.copy_from_slice(&src);
        }
    }

    /// The framebuffer to display, always in GBA dimensions. For a GB game
    /// this is the 160x144 image letterboxed into 240x160.
    fn display_framebuffer(&self) -> &[u32; 240 * 160] {
        if self.gb.is_some() {
            &self.gb_framebuffer
        } else {
            self.gba.get_framebuffer()
        }
    }

    fn save_active_slot(&mut self, slot: usize) -> Result<(), String> {
        let name = self.loaded_rom_name.clone();
        let res = match self.gb {
            Some(ref gb) => self.save_manager.save_slot(slot, gb, &name),
            None => self.save_manager.save_slot(slot, &self.gba, &name),
        };
        if res.is_ok() {
            self.sync_saves();
        }
        res
    }

    fn load_active_slot(&mut self, slot: usize) -> Result<(), String> {
        self.sync_saves();
        let name = self.loaded_rom_name.clone();
        match self.gb {
            Some(ref mut gb) => self.save_manager.load_slot(slot, gb, &name),
            None => self.save_manager.load_slot(slot, &mut self.gba, &name),
        }
    }

    /// Write an auto-save into the rotation now.
    pub fn write_autosave(&mut self) -> Result<usize, String> {
        let data = match self.gb {
            Some(ref gb) => crate::ui::emu_core::SnapshotCore::save_state(gb),
            None => crate::ui::emu_core::SnapshotCore::save_state(&self.gba),
        };
        let name = self.loaded_rom_name.clone();
        let n = self.autosaver.write(&name, &data).map_err(|e| format!("Auto-save failed: {e}"))?;
        self.sync_saves();
        Ok(n)
    }

    /// Restore auto-save `number` (1-3).
    pub fn load_autosave(&mut self, number: usize) -> Result<(), String> {
        let name = self.loaded_rom_name.clone();
        let data = self.autosaver.read(&name, number).map_err(|_| "That auto-save is gone".to_string())?;
        let ok = match self.gb {
            Some(ref mut gb) => crate::ui::emu_core::SnapshotCore::load_state(gb, &data),
            None => crate::ui::emu_core::SnapshotCore::load_state(&mut self.gba, &data),
        };
        if !ok {
            return Err("Auto-save is incompatible with this version".into());
        }
        self.autosaver.restart_timer();
        Ok(())
    }

    fn record_rewind_frame(&mut self) {
        match self.gb {
            Some(ref gb) => self.rewind_manager.record_frame(gb),
            None => self.rewind_manager.record_frame(&self.gba),
        }
    }

    fn rewind_active(&mut self) {
        match self.gb {
            Some(ref mut gb) => self.rewind_manager.rewind_step(gb),
            None => self.rewind_manager.rewind_step(&mut self.gba),
        };
    }

    /// Run one frame on whichever core is active.
    fn run_active_frame(&mut self) {
        match self.gb {
            Some(ref mut gb) => gb.run_frame(),
            None => self.run_ahead.run_frame(&mut self.gba),
        }
    }

    fn reset_active(&mut self) {
        match self.gb {
            Some(ref mut gb) => gb.reset(),
            None => {
                self.gba.reset();
                // A reset re-randomizes relocating game data: new session.
                self.memmap_dialog.on_reset();
            }
        }
    }

    fn ui_snapshot(&mut self, ctx: &egui::Context, path: &str) {
        let frame = ctx.cumulative_pass_nr();
        if frame == 1 && std::env::var("CRABBOY_UI_SNAPSHOT_MENU").is_err() {
            self.settings_window.is_open = true;
            if let Some(p) = std::env::var("CRABBOY_UI_SNAPSHOT_PAGE").ok().and_then(|p| p.parse::<usize>().ok()) {
                self.settings_window.set_page(p);
            }
        }
        if frame == 20 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let shot = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            let _ = image::save_buffer(path, &bytes, img.width() as u32, img.height() as u32, image::ExtendedColorType::Rgba8);
            std::env::remove_var("CRABBOY_UI_SNAPSHOT");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
    }

    /// Audio output of whichever core is running (the Game Boy core has its
    /// own). Menu and settings edits go here so they work in GB games too.
    fn active_audio_output(&mut self) -> &mut crate::gba::apu::audio_output::AudioOutput {
        match self.gb {
            Some(ref mut gb) => &mut gb.mmu.apu.audio_output,
            None => &mut self.gba.mmu.apu.audio_output,
        }
    }

    /// Draw the Settings window and apply what changed.
    fn show_settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_window.is_open {
            return;
        }
        let mut hd_pack_enabled = self.gba.is_hd_pack_enabled();
        let hd_pack_info = self
            .gba
            .hd_pack()
            .map(|p| (p.name.clone(), p.scale as usize, p.sprite_count(), p.tile_count()));
        let mut ws_enabled = self.gba.is_widescreen_enabled();
        let widescreen_profile = match self.gba.mmu.cartridge.as_ref() {
            Some(cart) => match crate::gba::widescreen::WidescreenDatabase::lookup(&cart.game_code, &cart.title) {
                Some(p) => format!("Tuned profile for {}: {}", p.title, p.notes),
                None => "No tuned profile for this game: generic expansion, edges may show glitches.".into(),
            },
            None => "Load a game to see its widescreen profile.".into(),
        };
        let custom_shader_name = self
            .custom_shader_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string());
        // Audio DSP values live behind atomics in the output; edit copies.
        let out = self.active_audio_output();
        let (mut surround, mut bass, mut width, hw_channels) =
            (out.surround_mode(), out.bass_boost(), out.surround_width(), out.hardware_channels());
        let (mut muted, mut volume) = (out.muted, out.volume);
        let mut ra_frames = self.run_ahead.frames;
        let mut ra_second = self.run_ahead.second_instance;
        let run_ahead_scope = if self.loaded_rom_name != "No ROM Loaded" {
            format!("for {}", self.loaded_rom_name)
        } else {
            "as the default for all games".into()
        };
        let run_ahead_fallback = self.run_ahead.fallback_reason.clone();
        let mut autosave_minutes = self.autosaver.interval_minutes();
        let act = self.settings_window.show(
            ctx,
            settings_window::Settings {
                filter: &mut self.display_filter,
                xbrz_factor: &mut self.xbrz_factor,
                blend: &mut self.frame_blend_mode,
                sharpen: &mut self.nvidia_sharpen,
                sharpness: &mut self.nvidia_sharpness,
                color_correction: &mut self.color_correction,
                ambient_glow: &mut self.ultrawide_ambient_glow,
                hd: &mut self.hd_mode7_config,
                hd_pack_enabled: &mut hd_pack_enabled,
                hd_pack_info,
                widescreen_enabled: &mut ws_enabled,
                widescreen_mode: &mut self.widescreen_config.mode,
                widescreen_profile,
                scale_mode: &mut self.scale_mode,
                aspect: &mut self.aspect_ratio,
                bezel: &mut self.bezel_renderer.mode,
                screenshot_enhanced: &mut self.screenshot_enhanced,
                custom_shader_name,
                speed: &mut self.speed_multiplier,
                slow_motion: &mut self.accessibility.active.slow_motion,
                run_ahead_frames: &mut ra_frames,
                run_ahead_second_instance: &mut ra_second,
                run_ahead_scope,
                run_ahead_fallback,
                muted: &mut muted,
                volume: &mut volume,
                surround: &mut surround,
                bass: &mut bass,
                width: &mut width,
                hw_channels,
                autosave_enabled: &mut self.autosaver.enabled,
                autosave_minutes: &mut autosave_minutes,
            },
        );
        if autosave_minutes != self.autosaver.interval_minutes() {
            self.autosaver.set_interval_minutes(autosave_minutes);
        }
        let out = self.active_audio_output();
        out.muted = muted;
        out.volume = volume;
        if act.audio_dsp_changed {
            out.set_surround_mode(surround);
            out.set_bass_boost(bass);
            out.set_surround_width(width);
        }
        if act.run_ahead_changed {
            if ra_frames != self.run_ahead.frames {
                self.set_run_ahead_frames(ra_frames);
            }
            if ra_second != self.run_ahead.second_instance {
                self.set_run_ahead_second_instance(ra_second);
            }
        }
        if act.slow_motion_changed && self.accessibility.commit() {
            self.config_dirty = true;
        }
        if act.open_rtc {
            self.show_rtc_dialog = true;
        }
        if act.open_mixer {
            self.audio_mixer_dialog.is_open = true;
        }
        if act.open_accessibility {
            self.accessibility_dialog.is_open = true;
        }
        if act.hd_changed {
            self.gba.set_hd_mode7_config(self.hd_mode7_config);
        }
        if act.hd_pack_toggled {
            self.gba.set_hd_pack_enabled(hd_pack_enabled);
            self.hd_pack_enabled = hd_pack_enabled;
        }
        if act.widescreen_changed {
            self.widescreen_config.enabled = ws_enabled;
            self.gba.set_widescreen_config(self.widescreen_config.clone());
        }
        if act.changed {
            self.config_dirty = true;
        }
        if act.load_shader {
            if let Some(file) = rfd::FileDialog::new().add_filter("Shader Profile", &["shader", "json", "txt"]).pick_file() {
                match std::fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|c| {
                    crate::gba::shader::CustomShaderParams::parse(&c).map_err(|e| e.to_string())
                }) {
                    Ok(params) => {
                        self.set_toast(format!("Loaded shader: {}", params.name));
                        self.screen_renderer.custom_shader = Some(params);
                        self.custom_shader_path = Some(file);
                        self.display_filter = DisplayFilter::Custom;
                        self.config_dirty = true;
                    }
                    Err(e) => self.set_toast(format!("Failed to load shader: {e}")),
                }
            }
        }
        if act.load_hd_pack {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                match crate::gba::hd_pack::HdPack::load_from_dir(&dir) {
                    Ok(pack) => {
                        let msg = format!("Loaded HD Pack '{}' ({} replacements)", pack.name, pack.len());
                        self.gba.load_hd_pack(pack);
                        self.hd_pack_path = Some(dir);
                        self.hd_pack_enabled = true;
                        self.config_dirty = true;
                        self.set_toast(msg);
                    }
                    Err(e) => self.set_toast(format!("Failed to load HD Pack: {e}")),
                }
            }
        }
        if act.dump_tiles {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                match self.gba.dump_tiles_and_sprites(&dir) {
                    Ok(m) => self.set_toast(format!("Dumped {} items to '{}'", m.replacements.len(), dir.display())),
                    Err(e) => self.set_toast(format!("Failed to dump tiles/sprites: {e}")),
                }
            }
        }
    }

    fn active_frame_counter(&self) -> u64 {
        match self.gb {
            Some(ref gb) => gb.frame_counter,
            None => self.gba.frame_counter,
        }
    }

    /// GBA keypad events mapped onto the Game Boy's 8-button pad. L and R
    /// have no Game Boy equivalent and are dropped.
    fn gb_key_for(key: Key) -> Option<GbKey> {
        Some(match key {
            Key::A => GbKey::A,
            Key::B => GbKey::B,
            Key::Select => GbKey::Select,
            Key::Start => GbKey::Start,
            Key::Right => GbKey::Right,
            Key::Left => GbKey::Left,
            Key::Up => GbKey::Up,
            Key::Down => GbKey::Down,
            Key::L | Key::R => return None,
        })
    }

    /// Writes controller + keyboard settings to the user's config file.
    ///
    /// Failures are surfaced as a toast rather than swallowed: a user who
    /// remaps their pad and gets nothing saved deserves to know why (a
    /// read-only home directory, a full disk) instead of silently losing the
    /// work again next launch.
    pub fn flush_config(&mut self) {
        self.config_dirty = false;
        self.last_config_flush = Instant::now();
        let cfg = AppConfig {
            version: config::CONFIG_VERSION,
            controllers: self.gamepad_manager.settings.clone(),
            keyboard: self.key_bindings.clone(),
            run_ahead: self.run_ahead_config.clone(),
            save_sync: self.save_sync_config.clone(),
            autosave: config::AutoSaveConfig {
                enabled: self.autosaver.enabled,
                interval_minutes: self.autosaver.interval_minutes(),
            },
            render: config::RenderConfig {
                filter: match self.display_filter {
                    DisplayFilter::Crisp => "crisp",
                    DisplayFilter::Linear => "linear",
                    DisplayFilter::LcdGrid => "lcdgrid",
                    DisplayFilter::LcdSubpixel => "lcdsubpixel",
                    DisplayFilter::CrtScanlines => "crtscanlines",
                    DisplayFilter::CrtGeom => "crtgeom",
                    DisplayFilter::NvidiaSharpen => "nvidiasharpen",
                    DisplayFilter::Xbrz => "xbrz",
                    DisplayFilter::Custom => "custom",
                }.to_string(),
                frame_blend: match self.frame_blend_mode {
                    crate::gba::frame_blend::FrameBlendMode::Off => "off",
                    crate::gba::frame_blend::FrameBlendMode::Simple50 => "simple50",
                    crate::gba::frame_blend::FrameBlendMode::SmartDeFlicker => "smart",
                    crate::gba::frame_blend::FrameBlendMode::LcdGhosting { .. } => "lcd",
                }.to_string(),
                custom_shader_path: self.custom_shader_path.clone(),
                hd_mode7: self.hd_mode7_config,
                hd_pack_path: self.hd_pack_path.clone(),
                hd_pack_enabled: self.hd_pack_enabled,
                widescreen: self.widescreen_config.clone(),
            },
            audio: config::AudioConfig {
                mode: match self.gba.hd_audio_mode() {
                    crate::gba::m4a::AudioEngineMode::HdReSynthesis => "hd",
                    crate::gba::m4a::AudioEngineMode::HardwareOnly => "hardware",
                }.to_string(),
                interpolation: match self.gba.m4a.config.interpolation {
                    crate::gba::m4a::M4aInterpolation::Linear => "linear",
                    crate::gba::m4a::M4aInterpolation::CubicHermite => "cubic",
                    crate::gba::m4a::M4aInterpolation::Sinc => "sinc",
                }.to_string(),
                reverb_enabled: self.gba.m4a.config.reverb_enabled,
                reverb_level: self.gba.m4a.config.reverb_level,
                master_volume: self.gba.m4a.config.master_volume,
            },
            accessibility: self.accessibility.store.clone(),
        };
        if let Err(e) = cfg.save() {
            log::warn!("Could not save config: {}", e);
            self.set_toast(format!("⚠ Settings not saved: {}", e));
        }
    }

    /// Update active run-ahead frames and second-instance mode for the currently loaded ROM.
    pub fn update_run_ahead_for_rom(&mut self) {
        if let Some(cfg) = self.run_ahead_config.per_game.get(&self.loaded_rom_name) {
            self.run_ahead.frames = cfg.frames;
            self.run_ahead.second_instance = cfg.second_instance;
        } else {
            self.run_ahead.frames = self.run_ahead_config.default_frames;
            self.run_ahead.second_instance = self.run_ahead_config.second_instance;
        }
    }

    pub fn set_run_ahead_frames(&mut self, frames: u32) {
        self.run_ahead.frames = frames;
        if self.loaded_rom_name != "No ROM Loaded" {
            let entry = self
                .run_ahead_config
                .per_game
                .entry(self.loaded_rom_name.clone())
                .or_insert_with(|| config::RunAheadGameConfig {
                    frames,
                    second_instance: self.run_ahead.second_instance,
                });
            entry.frames = frames;
        } else {
            self.run_ahead_config.default_frames = frames;
        }
        self.config_dirty = true;
        self.set_toast(if frames == 0 {
            "Run-ahead disabled".to_string()
        } else {
            format!("Run-ahead: {} frame{} ahead", frames, if frames > 1 { "s" } else { "" })
        });
    }

    pub fn set_run_ahead_second_instance(&mut self, second_instance: bool) {
        self.run_ahead.second_instance = second_instance;
        if self.loaded_rom_name != "No ROM Loaded" {
            let entry = self
                .run_ahead_config
                .per_game
                .entry(self.loaded_rom_name.clone())
                .or_insert_with(|| config::RunAheadGameConfig {
                    frames: self.run_ahead.frames,
                    second_instance,
                });
            entry.second_instance = second_instance;
        } else {
            self.run_ahead_config.second_instance = second_instance;
        }
        self.config_dirty = true;
        self.set_toast(if second_instance {
            "Run-ahead: second instance (glitchless audio)".to_string()
        } else {
            "Run-ahead: single instance (rollback)".to_string()
        });
    }

    /// Returns local directories where saves and states reside on this system.
    pub fn local_save_directories(&self) -> Vec<PathBuf> {
        let mut dirs = vec![PathBuf::from("saves")];
        if let Some(ref cart) = self.gba.mmu.cartridge {
            if let Some(p) = cart.save.save_path() {
                if let Some(parent) = p.parent() {
                    let parent_buf = parent.to_path_buf();
                    if !dirs.contains(&parent_buf) {
                        dirs.push(parent_buf);
                    }
                }
            }
        }
        if let Some(ref gb) = self.gb {
            if let Some(ref p) = gb.mmu.cart.save_path {
                if let Some(parent) = p.parent() {
                    let parent_buf = parent.to_path_buf();
                    if !dirs.contains(&parent_buf) {
                        dirs.push(parent_buf);
                    }
                }
            }
        }
        dirs
    }

    /// Sync saves with configured cloud / shared folder (ROADMAP M4).
    pub fn sync_saves(&mut self) {
        if !self.save_sync_config.enabled {
            return;
        }
        if let Some(ref sync_dir) = self.save_sync_config.sync_dir {
            let dirs = self.local_save_directories();
            let syncer = crate::gba::save_sync::SaveSync::new(sync_dir, dirs)
                .with_max_backups(self.save_sync_config.max_backups);
            let report = syncer.sync_all();
            if report.total_transferred() > 0 {
                self.set_toast(report.summary());
            }
            self.save_sync_dialog.last_report = Some(report);
        }
    }

    pub fn set_toast(&mut self, msg: impl Into<String>) {
        self.toast_message = Some((msg.into(), Instant::now()));
    }

    /// Applies `--ai-*` command-line options and optionally starts the agent.
    /// Returns a human-readable summary of what was applied, for logging.
    pub fn apply_ai_launch_options(
        &mut self,
        endpoint: Option<&str>,
        model: Option<&str>,
        brain: Option<&str>,
        objective: Option<&str>,
        no_pause: bool,
        autostart: bool,
    ) -> String {
        if let Some(e) = endpoint {
            self.ai_agent.config.endpoint = e.to_string();
        }
        if let Some(m) = model {
            self.ai_agent.config.model = m.to_string();
        }
        if let Some(b) = brain {
            self.ai_agent.config.brain = match b.trim().to_ascii_lowercase().as_str() {
                "heuristic" | "offline" | "local" => ai_agent::Brain::Heuristic,
                _ => ai_agent::Brain::VisionModel,
            };
        }
        if let Some(o) = objective {
            self.ai_agent.config.objective = o.to_string();
        }
        if no_pause {
            self.ai_agent.config.pause_while_thinking = false;
        }

        if autostart {
            let rom = self.loaded_rom_name.clone();
            self.ai_agent.start(&rom);
            self.ai_agent_dialog.show_panel = true;
            self.ai_agent.probe_endpoint();
            self.set_toast("🤖 AI Agent is now playing");
        }

        format!(
            "brain={:?} endpoint={} autostart={} pause_while_thinking={}",
            self.ai_agent.config.brain,
            self.ai_agent.config.endpoint,
            autostart,
            self.ai_agent.config.pause_while_thinking,
        )
    }
}

impl eframe::App for GbaApp {
    /// Last-chance flush: eframe calls this on window close, catching any edit
    /// still inside the 600ms debounce window.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.config_dirty {
            self.flush_config();
        }
    }

    /// With CRABBOY_UI_SNAPSHOT_MENU=<menu>, click that menu so the
    /// snapshot shows it open (see `ui_snapshot`).
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if std::env::var("CRABBOY_UI_SNAPSHOT").is_err() {
            return;
        }
        let Ok(name) = std::env::var("CRABBOY_UI_SNAPSHOT_MENU") else { return };
        let frame = ctx.cumulative_pass_nr();
        let Some(r) = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("menu_rect", name.as_str())))) else { return };
        let p = r.center();
        match frame {
            3 => raw.events.push(egui::Event::PointerMoved(p)),
            4 | 5 => raw.events.push(egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed: frame == 4,
                modifiers: Default::default(),
            }),
            _ => {}
        }
        // CRABBOY_UI_SNAPSHOT_SUBMENU=<category>: then hover that category
        // inside the open menu so its submenu shows too.
        if let Ok(sub) = std::env::var("CRABBOY_UI_SNAPSHOT_SUBMENU") {
            if frame == 8 {
                if let Some(r) = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("menu_rect", sub.as_str())))) {
                    raw.events.push(egui::Event::PointerMoved(r.center()));
                }
            }
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // CRABBOY_UI_SNAPSHOT=<png> [CRABBOY_UI_SNAPSHOT_PAGE=0..3]: open
        // Display Settings, save a screenshot of the real window and quit.
        // For checking UI layout without a person at the screen.
        if let Ok(path) = std::env::var("CRABBOY_UI_SNAPSHOT") {
            self.ui_snapshot(ctx, &path);
        }
        // GIF clips encode on a background thread; report when they land.
        for res in self.gif_recorder.poll_finished() {
            match res {
                Ok(name) => self.set_toast(format!("🎥 Saved GIF: {}", name)),
                Err(e) => self.set_toast(format!("GIF Error: {}", e)),
            }
        }
        if self.gif_recorder.is_encoding() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // Handle drag and drop: ROMs load into the emulator, guides (PDF/text)
        // go into the AI agent's knowledge base. Dropping a walkthrough used to
        // be interpreted as a ROM and fail with a confusing cartridge error.
        let dropped: Option<std::path::PathBuf> =
            ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()));
        if let Some(path) = dropped {
            let ext = path
                .extension()
                .map(|s| s.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if matches!(ext.as_str(), "pdf" | "txt" | "md" | "text") {
                let frame = self.active_frame_counter();
                match self.ai_agent.load_guide(&path, frame) {
                    Ok(msg) => {
                        self.ai_agent_dialog.chat_open = true;
                        self.set_toast(msg);
                    }
                    Err(e) => self.set_toast(format!("Guide import failed: {}", e)),
                }
            } else {
                self.load_rom_from_path(&path);
            }
        }

        // Keypad inputs (Keyboard + 8BitDo / XInput Gamepad)
        //
        // A focused text field (the AI Coach instruction box, cheat codes, the
        // endpoint fields) must swallow the keyboard: without this guard,
        // typing "Complete the first trainer badge" also mashes B, A, START…
        // into the running game. Gamepad input is unaffected — only the
        // keyboard half is suppressed — so a controller still works while the
        // user types.
        let typing = ctx.wants_keyboard_input();
        let mut key_events = Vec::new();
        // Tell the pad layer whether the guide currently owns the controller,
        // BEFORE polling: the poll decides on that basis whether game buttons
        // are emitted at all.
        self.gamepad_manager.guide_open = self.guide_dialog.is_open;
        // One-handed layouts swap the whole keyboard map (ROADMAP M10).
        let kb = self.key_bindings.for_layout(self.accessibility.active.one_handed_desktop);
        let pad = handle_input(ctx, &kb, &mut self.gamepad_manager, &mut |key: Key, pressed: bool| {
            key_events.push((key, pressed));
        });
        if typing {
            // Release everything the keyboard was holding, otherwise a button
            // held at the moment focus moved into the box would stay stuck
            // down for as long as the user is typing.
            key_events.retain(|(_, pressed)| !*pressed);
        }
        for (k, p) in key_events {
            // Toggle-instead-of-hold (ROADMAP M10).
            let p = self.accessibility.process_key(k, p);
            match self.gb {
                Some(ref mut gb) => {
                    if let Some(gk) = Self::gb_key_for(k) {
                        gb.set_key(gk, p);
                    }
                }
                None => self.gba.mmu.keypad.set_key_state(k, p),
            }
        }

        // Surface hotplug events. A controller that connects or drops mid-game
        // is something the player must be told about immediately — silently
        // losing input is the single worst controller-emulator failure mode.
        if let Some(msg) = self.gamepad_manager.hotplug_message.take() {
            self.set_toast(msg);
        }

        // Persist controller edits made in the config dialog. Debounced so
        // dragging a deadzone slider does not issue a write per frame.
        if self.controls_dialog.dirty {
            self.controls_dialog.dirty = false;
            self.config_dirty = true;
        }
        if self.config_dirty && self.last_config_flush.elapsed() > Duration::from_millis(600) {
            self.flush_config();
        }

        // ---- Controller-driven strategy guide ---------------------------
        // Handled before the game hotkeys so a guide binding that shares a
        // button with a game action cannot double-fire.
        if pad.pressed(PadAction::GuideToggle) {
            self.guide_dialog.is_open = !self.guide_dialog.is_open;
            self.set_toast(if self.guide_dialog.is_open {
                "📖 Strategy Guide opened (controller)"
            } else {
                "Strategy Guide closed"
            });
        }
        if self.guide_dialog.is_open {
            let dt = ctx.input(|i| i.stable_dt);
            self.guide_dialog.apply_pad_input(
                GuidePadInput {
                    scroll: pad.guide_scroll_axis,
                    scroll_speed: self.gamepad_manager.active_profile().guide_scroll_speed,
                    prev_page: pad.pressed(PadAction::GuidePrevPage),
                    next_page: pad.pressed(PadAction::GuideNextPage),
                },
                dt,
            );
        }

        // Fullscreen Toggle (F11 / Alt+Enter / a bound controller combo)
        let wants_fullscreen = ctx.input(|i| {
            i.key_pressed(kb.fullscreen)
                || (i.modifiers.alt && i.key_pressed(egui::Key::Enter))
        }) || pad.pressed(PadAction::Fullscreen);

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
        if ctx.input(|i| i.key_pressed(kb.screenshot)) {
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
        if pad.pressed(PadAction::Screenshot) {
            self.take_screenshot();
        }
        if pad.pressed(PadAction::Pause) {
            self.is_paused = !self.is_paused;
            self.set_toast(if self.is_paused { "Paused (Controller)" } else { "Resumed (Controller)" });
        }
        if pad.pressed(PadAction::QuickSave) {
            let slot = self.save_manager.active_slot;
            match self.save_active_slot(slot) {
                Ok(()) => self.set_toast(format!("Saved to Slot {} (Controller)", slot)),
                Err(e) => self.set_toast(e),
            }
        }
        if pad.pressed(PadAction::QuickLoad) {
            let slot = self.save_manager.active_slot;
            match self.load_active_slot(slot) {
                Ok(()) => self.set_toast(format!("Loaded Slot {} (Controller)", slot)),
                Err(e) => self.set_toast(e),
            }
        }

        // Keyboard Hotkeys
        ctx.input(|i| {
            if i.key_pressed(kb.pause) {
                self.is_paused = !self.is_paused;
                self.set_toast(if self.is_paused { "Paused" } else { "Resumed" });
            }
            if i.key_pressed(kb.reset) && i.modifiers.ctrl {
                self.reset_active();
                self.rewind_manager.clear();
                self.autosaver.restart_timer();
                self.set_toast("Reset Emulation");
            }
            if i.key_pressed(kb.frame_step) && self.is_paused {
                self.run_active_frame();
            }
            if i.key_pressed(kb.quick_save) {
                let slot = self.save_manager.active_slot;
                match self.save_active_slot(slot) {
                    Ok(()) => self.set_toast(format!("Saved to Slot {} (F5)", slot)),
                    Err(e) => self.set_toast(e),
                }
            }
            if i.key_pressed(kb.quick_load) {
                let slot = self.save_manager.active_slot;
                match self.load_active_slot(slot) {
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
            if i.modifiers.ctrl && i.key_pressed(egui::Key::J) {
                self.memmap_dialog.is_open = !self.memmap_dialog.is_open;
            }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::U) {
                self.accessibility_dialog.is_open = !self.accessibility_dialog.is_open;
            }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::M) {
                self.audio_mixer_dialog.is_open = !self.audio_mixer_dialog.is_open;
            }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::Y) {
                self.tas_dialog.is_open = !self.tas_dialog.is_open;
            }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::A) {
                self.ai_agent_dialog.is_open = !self.ai_agent_dialog.is_open;
            }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::G) {
                self.ai_agent_dialog.chat_open = !self.ai_agent_dialog.chat_open;
            }
            if i.key_pressed(egui::Key::F1) || (i.modifiers.ctrl && i.key_pressed(egui::Key::H)) {
                self.guide_dialog.is_open = !self.guide_dialog.is_open;
            }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::F12) {
                if self.gif_recorder.is_recording {
                    match self.gif_recorder.stop_and_save(&self.loaded_rom_name) {
                        Ok((_p, name)) => self.set_toast(format!("🎥 Encoding GIF: {}…", name)),
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
                self.aspect_ratio = self.aspect_ratio.cycle();
                self.set_toast(format!("📐 Aspect Ratio: {}", self.aspect_ratio.display_name()));
            }
            if (i.key_pressed(egui::Key::N) || i.key_pressed(egui::Key::Period)) && self.is_paused {
                self.run_active_frame();
            }
        });

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

        // ---- AI Agent Player -------------------------------------------
        // Collect any finished inference, then overwrite the keypad with the
        // agent's currently-held buttons. This runs AFTER human/TAS input so
        // the agent wins the pad (unless co-op mode is enabled, in which case
        // its buttons are OR'd over the human's).
        self.ai_agent.poll(self.active_frame_counter());
        if self.ai_agent.enabled {
            self.ai_agent.maybe_request(&self.gba, &self.loaded_rom_name);
            self.ai_agent.apply_inputs(&mut self.gba);
        }

        // Turbo and Rewind States
        let wants_rewind = ctx.input(|i| i.key_down(kb.rewind)) || pad.held(PadAction::Rewind);
        self.is_rewinding = wants_rewind && !self.is_paused;

        let is_turbo = ctx.input(|i| i.key_down(kb.turbo)) || pad.held(PadAction::Turbo);
        // Slow motion (ROADMAP M10) applies at normal speed; fast forward
        // and turbo override it.
        let slow = if is_turbo || self.speed_multiplier > 1 {
            1.0
        } else {
            self.accessibility.active.slow_motion.effective_multiplier() as f64
        };
        let effective_speed = if is_turbo { 4.0 } else { self.speed_multiplier as f64 * slow };

        // Fixed-time accumulator: frames run at the GBA's real rate, or
        // locked to a ~60 Hz display (see FramePacer).
        let now = Instant::now();
        let delta = now.duration_since(self.last_frame_instant).min(Duration::from_millis(100));
        self.last_frame_instant = now;

        // Lock to the display when it's ~60 Hz: one emulated frame per
        // refresh. At the GBA's true 59.73 Hz a 60 Hz screen repeats a frame
        // every ~4 s, which shows up as a hitch while scrolling. The 0.46%
        // speed-up is inaudible (audio is resampled to match).
        let base_rate = self.frame_pacer.base_rate(delta);
        // Locked: one frame per vsync. Bank the display's actual interval
        // as exactly one frame so timing jitter can't double or skip one.
        let locked_gap = (0.75 / 60.0..1.25 / 60.0).contains(&delta.as_secs_f64());
        let delta = if base_rate == 60.0 && locked_gap {
            Duration::from_secs_f64(1.0 / 60.0)
        } else {
            delta
        };
        let target_frame_duration = Duration::from_secs_f64(1.0 / (base_rate * effective_speed));
        let mut frames_run = 0;

        // The agent may request that emulation freeze while it waits on the
        // model, so a slow local vision model doesn't mean the game runs on
        // unattended for thousands of frames between decisions.
        let ai_stall = self.ai_agent.enabled
            && self.ai_agent.config.pause_while_thinking
            && self.ai_agent.is_thinking();

        if !self.is_paused && !ai_stall {
            self.frame_accumulator += delta;

            if self.is_rewinding {
                while self.frame_accumulator >= target_frame_duration && frames_run < 4 {
                    self.rewind_active();
                    self.sync_display_framebuffer();
                    self.frame_accumulator -= target_frame_duration;
                    frames_run += 1;
                }
            } else {
                while self.frame_accumulator >= target_frame_duration && frames_run < 4 {
                    self.run_active_frame();
                    self.sync_display_framebuffer();
                    self.record_rewind_frame();
                    // Bind the frame first: `display_framebuffer` borrows
                    // `self`, which would otherwise conflict with the
                    // recorder's &mut self borrow in the same expression.
                    let frame = if self.gb.is_some() {
                        &*self.gb_framebuffer
                    } else {
                        self.gba.get_framebuffer()
                    };
                    self.gif_recorder.capture_frame(frame);
                    self.frame_accumulator -= target_frame_duration;
                    frames_run += 1;
                    self.emulated_frames += 1;
                }
            }

            if self.frame_accumulator > target_frame_duration * 2 {
                self.frame_accumulator = Duration::ZERO;
            }
        } else if ai_stall {
            // Don't bank up wall-clock time while frozen on inference, or the
            // emulator would sprint through a burst of frames on resume.
            self.frame_accumulator = Duration::ZERO;
        }

        // Auto-save counts real play time only (not paused, rewinding or
        // frozen on the agent).
        let has_game = self.gb.is_some() || self.gba.mmu.cartridge.is_some();
        let playing = has_game && !self.is_paused && !ai_stall && !self.is_rewinding;
        if self.autosaver.tick(delta, playing) {
            match self.write_autosave() {
                Ok(_) => self.set_toast("💾 Auto-saved"),
                Err(e) => self.set_toast(e),
            }
        }

        // Advance the agent's action cursor by the frames actually emulated.
        // Rewound frames are deliberately excluded.
        if self.ai_agent.enabled && !self.is_rewinding {
            self.ai_agent.tick(frames_run as u32);
        }

        let ff = is_turbo || effective_speed > 1.0;
        let slow_audio = self.accessibility.active.slow_motion.audio;
        let out = match self.gb {
            Some(ref mut gb) => &mut gb.mmu.apu.audio_output,
            None => &mut self.gba.mmu.apu.audio_output,
        };
        out.set_fast_forwarding(ff);
        out.set_slow_motion(slow as f32, slow_audio);

        // Poll Pokémon party periodically if companion is active
        // The companion reads GBA-specific party structures out of EWRAM, so
        // it has no meaning while a Game Boy ROM is loaded.
        if self.gb.is_none() && self.pokemon_companion.is_open && self.emulated_frames.is_multiple_of(30) {
            if let Some(ref cart) = self.gba.mmu.cartridge {
                self.pokemon_companion.poll_party_memory(&self.gba.mmu, &cart.game_code);
            }
        }

        // FPS from true emulated frames over a 2 s window. With 0.5 s the
        // reading jumped between ~58 and 60 purely because a window holds 29
        // or 30 whole frames.
        let elapsed = self.fps_timer.elapsed().as_secs_f64();
        if elapsed >= 2.0 {
            log::debug!(
                "pacing: {:.2} emulated fps, display {:?} Hz, base {:.4}",
                self.emulated_frames as f64 / elapsed,
                self.frame_pacer.repaint_hz().map(|h| (h * 100.0).round() / 100.0),
                self.frame_pacer.base_rate_hint()
            );
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
                use settings_window::{menu_item, Page};
                // Where each menu sits, for the snapshot tool.
                fn remember_menu(ctx: &egui::Context, name: &'static str, r: egui::Response) {
                    ctx.data_mut(|d| d.insert_temp(egui::Id::new(("menu_rect", name)), r.rect));
                }
                let key = |k: egui::Key| k.name().to_string();
                let ctrl = |k: &str| format!("Ctrl+{k}");
                let kb = self.key_bindings.for_layout(self.accessibility.active.one_handed_desktop);
                let has_rom = self.gb.is_some() || self.gba.mmu.cartridge.is_some();

                // One menu holds every category; the bar itself only has
                // the few actions used all the time.
                remember_menu(ctx, "Menu", ui.menu_button(RichText::new("☰ Menu").strong(), |ui| {
                    ui.set_min_width(190.0);
                    remember_menu(ctx, "File", ui.menu_button("📂 File", |ui| {
                        ui.set_min_width(280.0);
                        if menu_item(ui, "📂 Open ROM…", "") {
                            ui.close_menu();
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("Game ROM", &["gba", "gb", "gbc", "bin"])
                                .add_filter("GBA ROM", &["gba", "bin"])
                                .add_filter("Game Boy / Color", &["gb", "gbc"])
                                .pick_file()
                            {
                                self.load_rom_from_path(&path);
                            }
                        }
                        let reload = self.loaded_rom_path.clone();
                        if ui.add_enabled(reload.is_some(), egui::Button::new("🔁 Reload ROM")).clicked() {
                            ui.close_menu();
                            if let Some(p) = reload {
                                self.load_rom_from_path(&p);
                            }
                        }
                        ui.separator();
                        let slot = self.save_manager.active_slot;
                        ui.add_enabled_ui(has_rom, |ui| {
                            if menu_item(ui, format!("💾 Save state (slot {slot})"), &key(kb.quick_save)) {
                                ui.close_menu();
                                match self.save_active_slot(slot) {
                                    Ok(()) => self.set_toast(format!("Saved to Slot {}", slot)),
                                    Err(e) => self.set_toast(e),
                                }
                            }
                            if menu_item(ui, format!("📥 Load state (slot {slot})"), &key(kb.quick_load)) {
                                ui.close_menu();
                                match self.load_active_slot(slot) {
                                    Ok(()) => self.set_toast(format!("Loaded Slot {}", slot)),
                                    Err(e) => self.set_toast(e),
                                }
                            }
                        });
                        ui.menu_button(format!("🔢 Slot: {slot}"), |ui| {
                            for n in 0..=9 {
                                if ui.radio_value(&mut self.save_manager.active_slot, n, format!("Slot {n}")).clicked() {
                                    ui.close_menu();
                                }
                            }
                        });
                        if menu_item(ui, "🗂 Save states…", &ctrl("S")) {
                            ui.close_menu();
                            self.show_save_manager_dialog = true;
                        }
                        let auto_label = if self.autosaver.enabled {
                            format!("🕘 Auto-saves (every {} min)", self.autosaver.interval_minutes())
                        } else {
                            "🕘 Auto-saves (off)".to_string()
                        };
                        ui.menu_button(auto_label, |ui| {
                            let list = if has_rom { self.autosaver.list(&self.loaded_rom_name) } else { Vec::new() };
                            if list.is_empty() {
                                ui.label(RichText::new("No auto-saves for this game yet").weak());
                            }
                            for e in &list {
                                if ui.button(format!("📥 Load auto-save {} · {}", e.number, e.age_label())).clicked() {
                                    ui.close_menu();
                                    let n = e.number;
                                    match self.load_autosave(n) {
                                        Ok(()) => self.set_toast(format!("Loaded auto-save {n}")),
                                        Err(err) => self.set_toast(err),
                                    }
                                }
                            }
                            ui.separator();
                            if ui.add_enabled(has_rom, egui::Button::new("💾 Auto-save now")).clicked() {
                                ui.close_menu();
                                match self.write_autosave() {
                                    Ok(n) => self.set_toast(format!("Auto-saved (auto-save {n})")),
                                    Err(err) => self.set_toast(err),
                                }
                            }
                            if ui.button("⚙ Auto-save settings…").clicked() {
                                ui.close_menu();
                                self.settings_window.open_at(settings_window::Page::Saves);
                            }
                        });
                        ui.separator();
                        if ui.add_enabled(has_rom, egui::Button::new("🔋 Write battery save now")).on_hover_text("Saves are written automatically; this forces it right away.").clicked() {
                            ui.close_menu();
                            match self.gb {
                                Some(ref mut gb) => {
                                    gb.mmu.cart.sync_to_disk();
                                    self.set_toast("Battery Save Synced to Disk");
                                }
                                None => {
                                    if let Some(ref mut cart) = self.gba.mmu.cartridge {
                                        cart.save.sync_to_disk();
                                        self.sync_saves();
                                        self.set_toast("Battery Save Synced to Disk");
                                    }
                                }
                            }
                        }
                        if menu_item(ui, "☁ Save sync…", "") {
                            ui.close_menu();
                            self.save_sync_dialog.is_open = true;
                        }
                        ui.separator();
                        if menu_item(ui, "🚪 Exit", "") {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }).response);

                    remember_menu(ctx, "Emulation", ui.menu_button("⏱ Emulation", |ui| {
                        ui.set_min_width(280.0);
                        let pause_label = if self.is_paused { "▶ Resume" } else { "⏸ Pause" };
                        if menu_item(ui, pause_label, &key(kb.pause)) {
                            self.is_paused = !self.is_paused;
                            ui.close_menu();
                        }
                        if ui.add_enabled(self.is_paused, egui::Button::new("⏭ Step one frame").shortcut_text(RichText::new(key(kb.frame_step)).color(ui.visuals().weak_text_color()))).clicked() {
                            self.run_active_frame();
                            ui.close_menu();
                        }
                        if ui.add_enabled(has_rom, egui::Button::new("🔄 Reset").shortcut_text(RichText::new(ctrl(&key(kb.reset))).color(ui.visuals().weak_text_color()))).clicked() {
                            ui.close_menu();
                            self.reset_active();
                            self.rewind_manager.clear();
                            self.autosaver.restart_timer();
                            self.set_toast("Emulation Reset");
                        }
                        ui.separator();
                        let speed_label = settings_window::speed_name(self.speed_multiplier, &self.accessibility.active.slow_motion);
                        ui.menu_button(format!("⏩ Speed: {speed_label}"), |ui| {
                            let sm = self.accessibility.active.slow_motion;
                            let current = if self.speed_multiplier > 1 { self.speed_multiplier * 100 } else if sm.enabled { (sm.speed_factor * 100.0).round() as u32 } else { 100 };
                            for (pct, label) in [(400, "4× fast"), (200, "2× fast"), (100, "Normal"), (75, "75%"), (50, "50%"), (25, "25%"), (10, "10%")] {
                                if ui.radio(current == pct, label).clicked() {
                                    let sm = &mut self.accessibility.active.slow_motion;
                                    if pct >= 100 {
                                        self.speed_multiplier = pct / 100;
                                        sm.enabled = false;
                                    } else {
                                        self.speed_multiplier = 1;
                                        sm.enabled = true;
                                        sm.speed_factor = pct as f32 / 100.0;
                                    }
                                    if self.accessibility.commit() {
                                        self.config_dirty = true;
                                    }
                                    ui.close_menu();
                                }
                            }
                        });
                        let ra = self.run_ahead.frames;
                        ui.menu_button(format!("⚡ Run-ahead: {}", match ra { 0 => "Off".to_string(), 1 => "1 frame".into(), n => format!("{n} frames") }), |ui| {
                            for (f, label) in [(0, "Off"), (1, "1 frame"), (2, "2 frames")] {
                                if ui.radio(self.run_ahead.frames == f, label).clicked() {
                                    self.set_run_ahead_frames(f);
                                    ui.close_menu();
                                }
                            }
                        });
                        ui.separator();
                        if menu_item(ui, "⚙ Emulation settings…", "") {
                            self.settings_window.open_at(Page::Speed);
                            ui.close_menu();
                        }
                        if menu_item(ui, "🕒 Real-time clock…", "") {
                            ui.close_menu();
                            self.show_rtc_dialog = true;
                        }
                        if menu_item(ui, "♿ Accessibility…", &ctrl("U")) {
                            self.accessibility_dialog.is_open = true;
                            ui.close_menu();
                        }
                    }).response);

                    remember_menu(ctx, "Video", ui.menu_button("🖥 Video", |ui| {
                        ui.set_min_width(280.0);
                        ui.menu_button(format!("🎨 Filter: {}", settings_window::filter_name(self.display_filter)), |ui| {
                            for f in settings_window::FILTERS {
                                if ui.radio_value(&mut self.display_filter, f, settings_window::filter_name(f)).clicked() {
                                    self.config_dirty = true;
                                    ui.close_menu();
                                }
                            }
                        });
                        ui.menu_button(format!("🌗 Blending: {}", settings_window::blend_name(self.frame_blend_mode)), |ui| {
                            for b in settings_window::BLENDS {
                                if ui.radio_value(&mut self.frame_blend_mode, b, settings_window::blend_name(b)).clicked() {
                                    self.config_dirty = true;
                                    ui.close_menu();
                                }
                            }
                        });
                        ui.menu_button(format!("📐 Aspect: {}", self.aspect_ratio.display_name()), |ui| {
                            for &ar in &AspectRatio::ALL {
                                if ui.radio_value(&mut self.aspect_ratio, ar, ar.display_name()).clicked() {
                                    ui.close_menu();
                                }
                            }
                        });
                        if ui.checkbox(&mut self.color_correction, "GBA color correction").changed() {
                            self.config_dirty = true;
                        }
                        let mut ws = self.gba.is_widescreen_enabled();
                        if ui.checkbox(&mut ws, "Widescreen").changed() {
                            self.gba.set_widescreen_enabled(ws);
                            self.widescreen_config.enabled = ws;
                            self.config_dirty = true;
                        }
                        ui.separator();
                        let fs_label = if self.is_fullscreen { "🗗 Exit fullscreen" } else { "⛶ Fullscreen" };
                        if menu_item(ui, fs_label, &key(kb.fullscreen)) {
                            ui.close_menu();
                            self.is_fullscreen = !self.is_fullscreen;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
                        }
                        if ui.add_enabled(has_rom, egui::Button::new("📸 Screenshot").shortcut_text(RichText::new(key(kb.screenshot)).color(ui.visuals().weak_text_color()))).clicked() {
                            ui.close_menu();
                            self.take_screenshot();
                        }
                        ui.separator();
                        if menu_item(ui, "⚙ Display settings…", "") {
                            self.settings_window.open_at(Page::Picture);
                            ui.close_menu();
                        }
                    }).response);

                    remember_menu(ctx, "Audio", ui.menu_button("🔊 Audio", |ui| {
                        ui.set_min_width(280.0);
                        let out = self.active_audio_output();
                        let mut on = !out.muted;
                        if ui.checkbox(&mut on, "Sound on").changed() {
                            out.muted = !on;
                        }
                        ui.horizontal(|ui| {
                            ui.label("Volume");
                            ui.add(egui::Slider::new(&mut out.volume, 0.0..=1.0).show_value(false));
                        });
                        let mode = out.surround_mode();
                        ui.menu_button(format!("🎧 Output: {}", settings_window::surround_name(mode)), |ui| {
                            for m in settings_window::SURROUNDS {
                                if ui.radio(mode == m, settings_window::surround_name(m)).clicked() {
                                    self.active_audio_output().set_surround_mode(m);
                                    ui.close_menu();
                                }
                            }
                        });
                        ui.separator();
                        if menu_item(ui, "🎛 Mixer & HD music…", &ctrl("M")) {
                            self.audio_mixer_dialog.is_open = true;
                            ui.close_menu();
                        }
                        if menu_item(ui, "⚙ Audio settings…", "") {
                            self.settings_window.open_at(Page::Sound);
                            ui.close_menu();
                        }
                    }).response);

                    remember_menu(ctx, "Game Boy", ui.menu_button("🕹 Game Boy", |ui| {
                        ui.set_min_width(280.0);
                        match self.gb {
                            Some(ref gb) => {
                                let model = if gb.is_cgb() { "Game Boy Color" } else { "Game Boy (DMG)" };
                                ui.label(RichText::new(model).strong());
                                ui.label(RichText::new(format!("{} · {:?}", gb.mmu.cart.title, gb.mmu.cart.mbc)).color(ui.visuals().weak_text_color()));
                                ui.label(RichText::new(format!(
                                    "{} ROM banks · {} KiB RAM{}{}",
                                    gb.mmu.cart.rom_banks,
                                    gb.mmu.cart.ram.len() / 1024,
                                    if gb.mmu.cart.has_battery { " · battery" } else { "" },
                                    if gb.mmu.double_speed { " · double speed" } else { "" },
                                )).color(ui.visuals().weak_text_color()));
                            }
                            None => {
                                ui.label(RichText::new("No Game Boy game loaded").color(ui.visuals().weak_text_color()));
                            }
                        }
                        ui.separator();
                        if ui
                            .checkbox(&mut self.gb_force_dmg, "Force original Game Boy mode")
                            .on_hover_text("Run Color-enhanced cartridges in black-and-white mode. Applies when the ROM is (re)loaded.")
                            .changed()
                        {
                            self.set_toast(if self.gb_force_dmg { "Original Game Boy mode: reload the ROM to apply" } else { "Color mode: reload the ROM to apply" });
                        }
                        let reload = self.loaded_rom_path.clone().filter(|_| self.gb.is_some());
                        if ui.add_enabled(reload.is_some(), egui::Button::new("🔁 Reload ROM to apply")).clicked() {
                            ui.close_menu();
                            if let Some(p) = reload {
                                self.load_rom_from_path(&p);
                            }
                        }
                    }).response);

                    remember_menu(ctx, "Tools", ui.menu_button("🛠 Tools", |ui| {
                        ui.set_min_width(300.0);
                        ui.label(RichText::new("GAME").size(11.5).strong().color(ui.visuals().weak_text_color()));
                        if menu_item(ui, "📜 Cheats & RAM search…", &ctrl("C")) {
                            self.cheats_dialog.is_open = true;
                            ui.close_menu();
                        }
                        if menu_item(ui, "🐾 Pokémon companion…", &ctrl("P")) {
                            self.pokemon_companion.is_open = true;
                            ui.close_menu();
                        }
                        if menu_item(ui, "🔗 Link cable…", &ctrl("L")) {
                            self.link_dialog.is_open = true;
                            ui.close_menu();
                        }
                        if menu_item(ui, "🎮 Cartridge sensors…", "") {
                            self.sensors_dialog.is_open = true;
                            ui.close_menu();
                        }
                        ui.separator();
                        ui.label(RichText::new("AI").size(11.5).strong().color(ui.visuals().weak_text_color()));
                        if menu_item(ui, "🤖 AI player…", &ctrl("A")) {
                            self.ai_agent_dialog.is_open = true;
                            ui.close_menu();
                        }
                        if menu_item(ui, "💬 AI coach…", &ctrl("G")) {
                            self.ai_agent_dialog.chat_open = true;
                            ui.close_menu();
                        }
                        ui.separator();
                        ui.label(RichText::new("RECORD").size(11.5).strong().color(ui.visuals().weak_text_color()));
                        let rec_label = if self.gif_recorder.is_recording { "⏹ Stop GIF recording" } else { "🎥 Record GIF" };
                        if menu_item(ui, rec_label, &ctrl("F12")) {
                            ui.close_menu();
                            if self.gif_recorder.is_recording {
                                match self.gif_recorder.stop_and_save(&self.loaded_rom_name) {
                                    Ok((_p, name)) => self.set_toast(format!("🎥 Encoding GIF: {}…", name)),
                                    Err(e) => self.set_toast(format!("GIF Error: {}", e)),
                                }
                            } else {
                                self.gif_recorder.start_recording();
                                self.set_toast("🔴 Recording Animated GIF");
                            }
                        }
                        if menu_item(ui, "⏱ TAS movies…", &ctrl("Y")) {
                            self.tas_dialog.is_open = true;
                            ui.close_menu();
                        }
                    }).response);

                    remember_menu(ctx, "Controls", ui.menu_button("🎮 Controls", |ui| {
                        ui.set_min_width(260.0);
                        if menu_item(ui, "🎮 Keyboard & controllers…", "") {
                            self.controls_dialog.open();
                            ui.close_menu();
                        }
                        let layout = self.accessibility.active.one_handed_desktop;
                        ui.menu_button(format!("🖐 Layout: {}", layout.display_name()), |ui| {
                            for h in crate::gba::accessibility::Handedness::ALL {
                                if ui.radio(layout == h, h.display_name()).clicked() {
                                    self.accessibility.active.one_handed_desktop = h;
                                    self.accessibility.commit();
                                    self.config_dirty = true;
                                    ui.close_menu();
                                }
                            }
                        });
                    }).response);

                    remember_menu(ctx, "Help", ui.menu_button("📖 Help", |ui| {
                        ui.set_min_width(280.0);
                        if menu_item(ui, "📖 Manual & strategy guide", "F1") {
                            self.guide_dialog.is_open = true;
                            ui.close_menu();
                        }
                        ui.menu_button("📑 Jump to chapter", |ui| {
                            let chapters = [
                                ("Cover", 0),
                                ("1  Welcome & quick start", 1),
                                ("2  Controls", 2),
                                ("3  Filters & shaders", 3),
                                ("4  Pokémon companion", 4),
                                ("5  Clock & sensors", 5),
                                ("6  Link cable & audio", 6),
                                ("7  TAS & troubleshooting", 7),
                            ];
                            for (title, page) in chapters {
                                if ui.button(title).clicked() {
                                    self.guide_dialog.open_at_page(page);
                                    ui.close_menu();
                                }
                            }
                        });
                        if menu_item(ui, "📥 Open manual as PDF", "") {
                            ui.close_menu();
                            match self.guide_dialog.open_external_pdf() {
                                Ok(_) => self.set_toast("📖 Opened the manual in your PDF viewer"),
                                Err(e) => self.set_toast(format!("Error: {}", e)),
                            }
                        }
                        if menu_item(ui, "💾 Export manual…", "") {
                            ui.close_menu();
                            match self.guide_dialog.export_pdf_as() {
                                Ok(Some(path)) => self.set_toast(format!("Saved: {}", path.file_name().unwrap_or_default().to_string_lossy())),
                                Ok(None) => {}
                                Err(e) => self.set_toast(format!("Export error: {}", e)),
                            }
                        }
                        ui.separator();
                        if menu_item(ui, "🔄 Check for updates…", "") {
                            self.updater_dialog.is_open = true;
                            ui.close_menu();
                        }
                        if menu_item(ui, "ℹ About CrabBoy Advance", "") {
                            self.show_about_dialog = true;
                            ui.close_menu();
                        }
                        ui.separator();
                        ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).small().color(ui.visuals().weak_text_color()));
                    }).response);

                    remember_menu(ctx, "Debug", ui.menu_button("🔬 Debug", |ui| {
                        ui.set_min_width(300.0);
                        fn toggle(ui: &mut egui::Ui, on: &mut bool, label: &str, sc: &str) {
                            let text = if *on { format!("✔ {label}") } else { format!("     {label}") };
                            if menu_item(ui, text, sc) {
                                *on = !*on;
                            }
                        }
                        let d = &mut self.debug_windows;
                        toggle(ui, &mut d.show_diagnostics, "🔬 Diagnostics", "F8");
                        toggle(ui, &mut d.show_cpu, "CPU", "");
                        toggle(ui, &mut d.show_ppu, "Graphics layers & sprites", "F6");
                        toggle(ui, &mut d.show_palette, "Palettes", "");
                        toggle(ui, &mut d.show_audio, "Audio channels", "F7");
                        toggle(ui, &mut d.show_memory, "Memory viewer", "");
                        toggle(ui, &mut self.memmap_dialog.is_open, "🧠 Memory map", "Ctrl+J");
                        ui.separator();
                        toggle(ui, &mut self.show_fps, "FPS counter", "");
                    }).response);
                }).response);

                ui.separator();
                let open = ui
                    .add(egui::Button::new("📂 Open").frame(false))
                    .on_hover_text("Open a ROM (.gba, .gb, .gbc)");
                if open.clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Game ROM", &["gba", "gb", "gbc", "bin"])
                        .add_filter("GBA ROM", &["gba", "bin"])
                        .add_filter("Game Boy / Color", &["gb", "gbc"])
                        .pick_file()
                    {
                        self.load_rom_from_path(&path);
                    }
                }
                remember_menu(ctx, "Open", open);
                let pause_label = if self.is_paused { "▶ Resume" } else { "⏸ Pause" };
                let pause = ui
                    .add_enabled(has_rom, egui::Button::new(pause_label).frame(false))
                    .on_hover_text(format!("Pause or resume ({})", key(kb.pause)));
                if pause.clicked() {
                    self.is_paused = !self.is_paused;
                }
                remember_menu(ctx, "Pause", pause);
                let settings = ui
                    .add(egui::Button::new("⚙ Settings").frame(false))
                    .on_hover_text("Display, emulation and audio settings");
                if settings.clicked() {
                    self.settings_window.is_open = !self.settings_window.is_open;
                }
                remember_menu(ctx, "Settings", settings);

                // Update status appears in the bar only when there's news;
                // the Update Center is always in Help.
                let update_status_peek = {
                    let lock = self.updater.status.lock().unwrap_or_else(|e| e.into_inner());
                    lock.clone()
                };
                let update_label = match &update_status_peek {
                    updater::UpdateStatus::UpdateAvailable { latest, .. } => {
                        Some(RichText::new(format!("⬆ v{} available", latest.version)).color(Color32::from_rgb(255, 200, 60)).strong())
                    }
                    updater::UpdateStatus::DownloadedReadyToRestart { .. } => {
                        Some(RichText::new("✨ Restart to update").color(Color32::LIGHT_GREEN).strong())
                    }
                    updater::UpdateStatus::Downloading { progress, .. } => {
                        Some(RichText::new(format!("⬇ Updating {:.0}%", progress * 100.0)).color(Color32::LIGHT_BLUE))
                    }
                    _ => None,
                };
                if let Some(label) = update_label {
                    ui.menu_button(label, |ui| {
                        ui.set_min_width(240.0);
                        if let updater::UpdateStatus::DownloadedReadyToRestart { .. } = &update_status_peek {
                            if menu_item(ui, "🔄 Restart now", "") {
                                if let Err(e) = self.updater.restart_and_apply() {
                                    self.set_toast(format!("Restart error: {}", e));
                                }
                                ui.close_menu();
                            }
                        }
                        if menu_item(ui, "Open update center…", "") {
                            self.updater_dialog.is_open = true;
                            ui.close_menu();
                        }
                    });
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {

                    if self.show_fps {
                        let fps_color = if self.fps >= 55.0 { Color32::GREEN } else { Color32::YELLOW };
                        ui.label(RichText::new(format!("{:.1} FPS", self.fps)).color(fps_color).monospace());
                    }
                    if is_turbo {
                        ui.label(RichText::new("[TURBO 4X]").color(Color32::from_rgb(255, 100, 100)).strong());
                    }
                    let ar_btn = ui.add(
                        egui::Button::new(
                            RichText::new(self.aspect_ratio.badge_text())
                                .color(Color32::from_rgb(100, 200, 255))
                                .strong()
                        ).frame(false)
                    ).on_hover_text(format!("Aspect Ratio: {} (Click or press F3 to cycle)", self.aspect_ratio.display_name()));
                    if ar_btn.clicked() {
                        self.aspect_ratio = self.aspect_ratio.cycle();
                        self.set_toast(format!("📐 Aspect Ratio: {}", self.aspect_ratio.display_name()));
                    }

                    // Gamepad status badge. Clicking it opens the controller
                    // config — the badge is the first thing a user looks at
                    // when a pad misbehaves, so it should also be the fix.
                    let pad_count = self.gamepad_manager.pads.len();
                    let badge = match self.gamepad_manager.active_pad_name() {
                        Some(name) => {
                            let short: String = name.chars().take(22).collect();
                            let extra = if pad_count > 1 {
                                format!(" +{}", pad_count - 1)
                            } else {
                                String::new()
                            };
                            RichText::new(format!("🎮 {}{}", short, extra))
                                .color(Color32::from_rgb(100, 220, 100))
                                .small()
                        }
                        None => RichText::new("🎮 No Controller").color(Color32::DARK_GRAY).small(),
                    };
                    let hover = if pad_count > 1 {
                        format!("{} controllers connected — click to choose Player 1 or remap", pad_count)
                    } else {
                        "Click to configure controllers and remap buttons".to_string()
                    };
                    // Status items only use the space the menus leave free:
                    // on a narrow window they drop out (ROM name first, then
                    // this badge) instead of drawing over the menus.
                    if ui.available_width() > 190.0
                        && ui.add(egui::Label::new(badge).sense(Sense::click()))
                            .on_hover_text(hover)
                            .clicked()
                    {
                        self.controls_dialog.open();
                    }

                    // Console badge: which core is actually executing.
                    let (badge, badge_col) = match self.gb {
                        Some(ref gb) if gb.is_cgb() => ("GBC", Color32::from_rgb(120, 200, 255)),
                        Some(_) => ("GB", Color32::from_rgb(150, 220, 150)),
                        None => ("GBA", Color32::from_rgb(190, 160, 255)),
                    };
                    if ui.available_width() > 40.0 {
                        ui.label(RichText::new(badge).color(badge_col).small().strong());
                    }

                    // ROM name: truncated with "…" to the space that's left.
                    let room = ui.available_width() - 8.0;
                    if room > 60.0 {
                        ui.scope(|ui| {
                            ui.set_max_width(room);
                            ui.add(egui::Label::new(RichText::new(&self.loaded_rom_name).color(Color32::LIGHT_GRAY).small()).truncate())
                                .on_hover_text(&self.loaded_rom_name);
                        });
                    }
                });
            });
        });
        }

        // Debug Floating Windows
        self.debug_windows.show(ctx, &mut self.gba);

        // Controllers & Key Mapping dialog (pad selection, profiles, remapping)
        {
            let mut dialog_toast: Option<String> = None;
            self.controls_dialog
                .show(ctx, &mut self.gamepad_manager, &mut dialog_toast);
            if let Some(t) = dialog_toast {
                self.set_toast(t);
            }
        }

        // AI Agent live spectator panel (must be declared before CentralPanel
        // so egui allocates the remaining space to the game viewport).
        self.ai_agent_dialog.show_panel(ctx, &self.ai_agent);

        // Central Panel (GBA LCD Viewport)
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::from_rgb(14, 15, 18)))
            .show(ctx, |ui| {
                let available_size = ui.available_size();
                let custom_aspect = if self.gb.is_none() && self.gba.is_widescreen_enabled() {
                    Some(self.gba.widescreen_config().aspect_ratio())
                } else {
                    None
                };
                let target_size = self.aspect_ratio.calculate_target_size_for_aspect(available_size, self.scale_mode, custom_aspect);

                let frame = if self.gb.is_some() {
                    &*self.gb_framebuffer
                } else {
                    self.gba.get_framebuffer()
                };
                let hd_frame = if self.gb.is_none()
                    && (self.hd_mode7_config.scale != crate::gba::ppu::hd_mode7::HdScale::Off
                        || self.gba.is_hd_pack_enabled()
                        || self.gba.is_widescreen_enabled())
                {
                    self.gba.render_hd_frame()
                } else {
                    None
                };
                let tex = self.screen_renderer.update_framebuffer(
                    ctx,
                    frame,
                    hd_frame.as_ref(),
                    self.hd_mode7_config.ssaa,
                    self.display_filter,
                    self.frame_blend_mode,
                    self.nvidia_sharpen,
                    self.nvidia_sharpness,
                    self.color_correction,
                    self.xbrz_factor,
                    self.accessibility.active.colorblind_mode,
                    self.accessibility.active.colorblind_intensity,
                );

                let x_offset = (available_size.x - target_size.x).max(0.0) / 2.0;
                let y_offset = (available_size.y - target_size.y).max(0.0) / 2.0;

                // Ultrawide Ambient Lighting / Edge Glow Backdrop
                if self.ultrawide_ambient_glow && x_offset > 16.0 {
                    let fb = frame;
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

                        // AI Agent HUD badge — shows the viewer that the game
                        // is under machine control and what it is doing.
                        if self.ai_agent.enabled {
                            let (text, fill, border) = match &self.ai_agent.status {
                                ai_agent::AgentStatus::Thinking => {
                                    let secs = self
                                        .ai_agent
                                        .thinking_elapsed()
                                        .map(|d| d.as_secs_f32())
                                        .unwrap_or(0.0);
                                    (
                                        format!("🤖 AI THINKING ({:.1}s)", secs),
                                        Color32::from_rgba_unmultiplied(190, 140, 20, 230),
                                        Color32::from_rgb(255, 210, 90),
                                    )
                                }
                                ai_agent::AgentStatus::Error(_) => (
                                    "🤖 AI OFFLINE".to_string(),
                                    Color32::from_rgba_unmultiplied(170, 40, 40, 230),
                                    Color32::from_rgb(255, 120, 120),
                                ),
                                _ => {
                                    let act = self
                                        .ai_agent
                                        .current_action_label()
                                        .unwrap_or_else(|| "standing by".to_string());
                                    (
                                        format!("🤖 AI PLAYING — {}", act),
                                        Color32::from_rgba_unmultiplied(30, 110, 170, 230),
                                        Color32::from_rgb(120, 200, 255),
                                    )
                                }
                            };

                            let badge_rect = Rect::from_min_size(
                                rect.min + Vec2::new(12.0, 86.0),
                                Vec2::new(250.0, 28.0),
                            );
                            ui.painter().rect_filled(badge_rect, 6.0, fill);
                            ui.painter().rect_stroke(
                                badge_rect,
                                6.0,
                                Stroke::new(1.0_f32, border),
                                egui::StrokeKind::Outside,
                            );
                            ui.painter().text(
                                badge_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                text,
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
                                    match self.load_active_slot(slot) {
                                        Ok(()) => action_toast = Some(format!("Loaded Slot {}", slot)),
                                        Err(e) => action_toast = Some(e),
                                    }
                                }
                                if ui.button("Overwrite").clicked() {
                                    match self.save_active_slot(slot) {
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
                                    match self.save_active_slot(slot) {
                                        Ok(()) => action_toast = Some(format!("Saved to Slot {}", slot)),
                                        Err(e) => action_toast = Some(e),
                                    }
                                }
                            }
                        });
                    }

                    ui.separator();
                    let status = if self.autosaver.enabled {
                        let left = self.autosaver.time_until_next().as_secs();
                        format!(
                            "every {} min of play · next in {}:{:02}",
                            self.autosaver.interval_minutes(),
                            left / 60,
                            left % 60
                        )
                    } else {
                        "off".to_string()
                    };
                    ui.label(RichText::new(format!("🕘 Auto-saves ({status})")).strong());
                    ui.label(
                        RichText::new("Kept apart from the slots above: the 3 newest, oldest replaced first.")
                            .weak(),
                    );
                    let list = self.autosaver.list(&self.loaded_rom_name);
                    for n in 1..=crate::autosave::ROTATION {
                        ui.horizontal(|ui| {
                            ui.label(format!("  Auto {n}"));
                            match list.iter().find(|e| e.number == n) {
                                Some(e) => {
                                    let newest = list.first().map(|f| f.number) == Some(n);
                                    let when = format!(
                                        "{} ({} KB){}",
                                        e.age_label(),
                                        e.size_bytes / 1024,
                                        if newest { " · newest" } else { "" }
                                    );
                                    ui.label(RichText::new(when).color(Color32::LIGHT_GREEN));
                                    if ui.button("Load").clicked() {
                                        match self.load_autosave(n) {
                                            Ok(()) => action_toast = Some(format!("Loaded auto-save {n}")),
                                            Err(err) => action_toast = Some(err),
                                        }
                                    }
                                }
                                None => {
                                    ui.label(RichText::new("[Empty]").weak());
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
        self.pokemon_companion.show(ctx, &self.gba, &self.memmap_dialog.map);
        self.audio_mixer_dialog.show(ctx, &mut self.gba, &mut dialog_toast);
        self.tas_dialog.show(ctx, &mut self.tas_engine, &mut self.gba, &mut dialog_toast);
        self.ai_agent_dialog.show_dialog(ctx, &mut self.ai_agent, &self.loaded_rom_name, &mut dialog_toast);
        {
            // Borrow the frame counter before handing out `&mut self.ai_agent`.
            let frame = self.active_frame_counter();
            // Drain any background guide download first so the progress bar
            // and the finished library render in the same frame.
            if let Some(res) = self.ai_agent.poll_web_import(frame) {
                dialog_toast = Some(match res {
                    Ok(msg) => msg,
                    Err(e) => format!("Guide import failed: {}", e),
                });
            }
            self.ai_agent_dialog
                .show_chat(ctx, &mut self.ai_agent, frame, &mut dialog_toast);
        }
        self.guide_dialog.show(ctx, &mut dialog_toast);
        self.updater_dialog.show(ctx, &self.updater, &mut dialog_toast);

        if self.accessibility_dialog.show(ctx, &mut self.accessibility, &self.key_bindings, &mut dialog_toast) {
            // Always flush: reset / "use for all games" change the store
            // without `commit` reporting it.
            self.accessibility.commit();
            self.config_dirty = true;
        }
        // Interface scale: applied once no mouse button is held, so dragging
        // the size slider doesn't rescale the UI under the pointer.
        let zoom = self.accessibility.active.ui_scale_clamped();
        if (ctx.zoom_factor() - zoom).abs() > 0.001 && !ctx.input(|i| i.pointer.any_down()) {
            ctx.set_zoom_factor(zoom);
        }

        self.memmap_dialog.show(ctx, &mut self.gba, &mut self.debug_windows.mem_state.watch_list, &mut dialog_toast);
        self.show_settings_window(ctx);

        let local_dirs = self.local_save_directories();
        self.save_sync_dialog.show(
            ctx,
            &mut self.save_sync_config,
            &mut self.config_dirty,
            &local_dirs,
            &self.loaded_rom_name,
            &mut dialog_toast,
        );

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
        // While frozen on inference no frames are being emulated, so the
        // accumulator-derived deadline would let repaints stop and freeze the
        // "thinking" timer. Poll at ~10 Hz instead to keep the HUD live and to
        // pick up the worker's response promptly.
        if ai_stall {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else {
            ctx.request_repaint_after(time_until_next);
        }
    }
}
