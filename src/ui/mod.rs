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
pub mod web_guide;

use accessibility_dialog::AccessibilityDialog;
use ai_agent::AiAgent;
use ai_agent_dialog::AiAgentDialog;
use audio_mixer_dialog::AudioMixerDialog;
use bezels::{BezelMode, BezelRenderer};
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
use crate::gba::apu::SurroundMode;
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
    /// Colorblind filter, slow motion, sticky buttons, one-handed layout and
    /// UI scale, per game (ROADMAP M10).
    pub accessibility: crate::gba::accessibility::AccessibilityManager,

    // Settings
    pub run_ahead: crate::gba::run_ahead::RunAhead,
    pub run_ahead_config: config::RunAheadSettings,
    pub save_sync_config: config::SaveSyncConfig,
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
            accessibility,
            run_ahead,
            run_ahead_config,
            save_sync_config: config.save_sync.clone(),
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
                        self.widescreen_config = self.gba.widescreen_config().clone();
                        self.update_run_ahead_for_rom();
                        if let Some(cart) = self.gba.mmu.cartridge.as_ref() {
                            let (code, title) = (cart.game_code.clone(), cart.title.clone());
                            self.accessibility.load_for_game(&code, &title);
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
            None => self.gba.reset(),
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

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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

        // Fixed-Time Accumulator for hardware-accurate 59.7275 Hz / 60 FPS frame pacing
        let now = Instant::now();
        let delta = now.duration_since(self.last_frame_instant).min(Duration::from_millis(100));
        self.last_frame_instant = now;

        let target_frame_duration = Duration::from_secs_f64(1.0 / (59.7275 * effective_speed));
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
                            .add_filter("Game ROM", &["gba", "gb", "gbc", "bin"])
                            .add_filter("GBA ROM", &["gba", "bin"])
                            .add_filter("Game Boy / Color", &["gb", "gbc"])
                            .pick_file()
                        {
                            self.load_rom_from_path(&path);
                        }
                    }
                    if ui.button("Reset (Ctrl+R)").clicked() {
                        ui.close_menu();
                        self.reset_active();
                        self.rewind_manager.clear();
                        self.set_toast("Emulation Reset");
                    }
                    ui.separator();
                    if ui.button(format!("Quick Save State - Slot {} (F5)", self.save_manager.active_slot)).clicked() {
                        ui.close_menu();
                        let slot = self.save_manager.active_slot;
                        match self.save_active_slot(slot) {
                            Ok(()) => self.set_toast(format!("Saved to Slot {}", slot)),
                            Err(e) => self.set_toast(e),
                        }
                    }
                    if ui.button(format!("Quick Load State - Slot {} (F8)", self.save_manager.active_slot)).clicked() {
                        ui.close_menu();
                        let slot = self.save_manager.active_slot;
                        match self.load_active_slot(slot) {
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
                    if ui.button("Cloud & Folder Save Sync...").clicked() {
                        ui.close_menu();
                        self.save_sync_dialog.is_open = true;
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
                            self.run_active_frame();
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
                    let ra_label = if self.run_ahead.frames == 0 {
                        "Run-Ahead: Disabled".to_string()
                    } else {
                        format!("Run-Ahead: {} frame{}", self.run_ahead.frames, if self.run_ahead.frames > 1 { "s" } else { "" })
                    };
                    ui.menu_button(ra_label, |ui| {
                        ui.label(RichText::new("Input Latency Reduction").strong());
                        let scope = if self.loaded_rom_name != "No ROM Loaded" {
                            format!("Configured for: {}", self.loaded_rom_name)
                        } else {
                            "Global default setting".to_string()
                        };
                        ui.label(RichText::new(scope).color(Color32::GRAY).small());
                        ui.separator();
                        for f in 0..=2 {
                            let label = match f {
                                0 => "Off (Normal Latency)",
                                1 => "1 Frame Run-Ahead",
                                2 => "2 Frames Run-Ahead",
                                _ => unreachable!(),
                            };
                            if ui.radio(self.run_ahead.frames == f, label).clicked() {
                                self.set_run_ahead_frames(f);
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        let mut second = self.run_ahead.second_instance;
                        if ui.checkbox(&mut second, "Second Instance (Glitchless Audio)").clicked() {
                            self.set_run_ahead_second_instance(second);
                            ui.close_menu();
                        }
                        if let Some(ref reason) = self.run_ahead.fallback_reason {
                            ui.label(RichText::new(format!("⚠ Shadow core fallback: {reason}")).color(Color32::YELLOW).small());
                        }
                    });
                    ui.separator();
                    ui.label("Speed:");
                    if ui.radio_value(&mut self.speed_multiplier, 1, "1x (Normal 60 FPS)").clicked() { ui.close_menu(); }
                    if ui.radio_value(&mut self.speed_multiplier, 2, "2x Fast Forward").clicked() { ui.close_menu(); }
                    if ui.radio_value(&mut self.speed_multiplier, 4, "4x Turbo").clicked() { ui.close_menu(); }
                    ui.separator();
                    ui.label("Slow motion (saved per game):");
                    {
                        let sm = &mut self.accessibility.active.slow_motion;
                        let mut pick = if sm.enabled { (sm.speed_factor * 100.0).round() as u32 } else { 100 };
                        let before = pick;
                        for (v, label) in [(100, "Off"), (75, "75%"), (50, "50%"), (25, "25%"), (10, "10%")] {
                            ui.radio_value(&mut pick, v, label);
                        }
                        if pick != before {
                            sm.enabled = pick < 100;
                            if pick < 100 {
                                sm.speed_factor = pick as f32 / 100.0;
                            }
                            if self.accessibility.commit() {
                                self.config_dirty = true;
                            }
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    if ui.button("Accessibility... (Ctrl+U)").clicked() {
                        self.accessibility_dialog.is_open = true;
                        ui.close_menu();
                    }
                });

                ui.menu_button("Video", |ui| {
                    ui.label(RichText::new("Frame Blending (LCD Ghosting):").strong());
                    if ui.radio_value(&mut self.frame_blend_mode, crate::gba::frame_blend::FrameBlendMode::Off, "Off (Instant 60 Hz)").clicked() { self.config_dirty = true; }
                    if ui.radio_value(&mut self.frame_blend_mode, crate::gba::frame_blend::FrameBlendMode::Simple50, "50/50 Blend (Smooth Transparency)").clicked() { self.config_dirty = true; }
                    if ui.radio_value(&mut self.frame_blend_mode, crate::gba::frame_blend::FrameBlendMode::SmartDeFlicker, "Smart De-Flicker (Motion-Preserving)").clicked() { self.config_dirty = true; }
                    if ui.radio_value(&mut self.frame_blend_mode, crate::gba::frame_blend::FrameBlendMode::LcdGhosting { decay: 0.65 }, "Authentic LCD Ghosting (AGB-001)").clicked() { self.config_dirty = true; }
                    ui.separator();
                    ui.label(RichText::new("Filter / Shader Preset:").strong());
                    if ui.radio_value(&mut self.display_filter, DisplayFilter::Crisp, "Crisp Pixel (Nearest)").clicked() { self.config_dirty = true; }
                    if ui.radio_value(&mut self.display_filter, DisplayFilter::Linear, "Smooth (Bilinear)").clicked() { self.config_dirty = true; }
                    if ui.radio_value(&mut self.display_filter, DisplayFilter::LcdGrid, "Retro LCD Grid").clicked() { self.config_dirty = true; }
                    if ui.radio_value(&mut self.display_filter, DisplayFilter::LcdSubpixel, "Authentic GBA LCD Subpixels").clicked() { self.config_dirty = true; }
                    if ui.radio_value(&mut self.display_filter, DisplayFilter::CrtScanlines, "CRT Scanlines").clicked() { self.config_dirty = true; }
                    if ui.radio_value(&mut self.display_filter, DisplayFilter::CrtGeom, "CRT Aperture Grille & Bloom").clicked() { self.config_dirty = true; }
                    if ui.radio_value(&mut self.display_filter, DisplayFilter::Xbrz, "xBRZ High-Definition (AI Edge Smoothing)").clicked() { self.config_dirty = true; }
                    if self.display_filter == DisplayFilter::Xbrz {
                        ui.indent("xbrz_selector", |ui| {
                            ui.label("xBRZ Scale Factor:");
                            if ui.radio_value(&mut self.xbrz_factor, 2, "2x (480p)").clicked() { self.config_dirty = true; }
                            if ui.radio_value(&mut self.xbrz_factor, 3, "3x (720p)").clicked() { self.config_dirty = true; }
                            if ui.radio_value(&mut self.xbrz_factor, 4, "4x (960p - Recommended)").clicked() { self.config_dirty = true; }
                            if ui.radio_value(&mut self.xbrz_factor, 5, "5x (1200p)").clicked() { self.config_dirty = true; }
                            if ui.radio_value(&mut self.xbrz_factor, 6, "6x (1440p HD)").clicked() { self.config_dirty = true; }
                        });
                    }
                    let custom_label = if let Some(ref path) = self.custom_shader_path {
                        format!("Custom Shader ({})", path.file_name().and_then(|n| n.to_str()).unwrap_or("active"))
                    } else {
                        "Custom User Shader".to_string()
                    };
                    if ui.radio_value(&mut self.display_filter, DisplayFilter::Custom, custom_label).clicked() { self.config_dirty = true; }
                    if ui.button("Load Custom Shader (.shader / .json)...").clicked() {
                        if let Some(file) = rfd::FileDialog::new()
                            .add_filter("Shader Profile", &["shader", "json", "txt"])
                            .pick_file()
                        {
                            if let Ok(content) = std::fs::read_to_string(&file) {
                                match crate::gba::shader::CustomShaderParams::parse(&content) {
                                    Ok(params) => {
                                        self.set_toast(format!("Loaded shader: {}", params.name));
                                        self.screen_renderer.custom_shader = Some(params);
                                        self.custom_shader_path = Some(file);
                                        self.display_filter = DisplayFilter::Custom;
                                        self.config_dirty = true;
                                    }
                                    Err(e) => {
                                        self.set_toast(format!("Failed to parse shader: {e}"));
                                    }
                                }
                            }
                        }
                    }
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
                    ui.label("HD Mode 7 (High-Res Affine Rendering):");
                    let prev_hd = self.hd_mode7_config;
                    for &scale in &crate::gba::ppu::hd_mode7::HdScale::ALL {
                        ui.radio_value(&mut self.hd_mode7_config.scale, scale, scale.display_name());
                    }
                    if self.hd_mode7_config.scale != crate::gba::ppu::hd_mode7::HdScale::Off {
                        ui.checkbox(&mut self.hd_mode7_config.perspective_interpolation, "Perspective Scanline Interpolation");
                        ui.checkbox(&mut self.hd_mode7_config.ssaa, "Supersampled Anti-Aliasing (SSAA Native)");
                    }
                    if prev_hd != self.hd_mode7_config {
                        self.gba.set_hd_mode7_config(self.hd_mode7_config);
                        self.config_dirty = true;
                    }
                    ui.separator();
                    ui.label("HD Sprite & Tile Packs (Mesen-style):");
                    let mut pack_enabled = self.gba.is_hd_pack_enabled();
                    if ui.checkbox(&mut pack_enabled, "Enable HD Pack Replacement").changed() {
                        self.gba.set_hd_pack_enabled(pack_enabled);
                        self.hd_pack_enabled = pack_enabled;
                        self.config_dirty = true;
                    }
                    if let Some(pack) = self.gba.hd_pack() {
                        ui.label(format!("Active Pack: {} ({}x HD)", pack.name, pack.scale));
                        ui.label(format!("Replacements: {} sprites, {} tiles", pack.sprite_count(), pack.tile_count()));
                    } else {
                        ui.label("No HD Pack loaded");
                    }
                    if ui.button("Load HD Pack Folder...").clicked() {
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
                                Err(e) => {
                                    self.set_toast(format!("Failed to load HD Pack: {}", e));
                                }
                            }
                        }
                        ui.close_menu();
                    }
                    if ui.button("Dump Tiles & Sprites to Folder...").clicked() {
                        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                            match self.gba.dump_tiles_and_sprites(&dir) {
                                Ok(manifest) => {
                                    self.set_toast(format!("Dumped {} items to '{}'", manifest.replacements.len(), dir.display()));
                                }
                                Err(e) => {
                                    self.set_toast(format!("Failed to dump tiles/sprites: {}", e));
                                }
                            }
                        }
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.label("Per-Game Widescreen (16:9 / 16:10):");
                    let mut ws_enabled = self.gba.is_widescreen_enabled();
                    if ui.checkbox(&mut ws_enabled, "Enable Widescreen Expansion").changed() {
                        self.gba.set_widescreen_enabled(ws_enabled);
                        self.widescreen_config.enabled = ws_enabled;
                        self.config_dirty = true;
                    }
                    if let Some(ref cart) = self.gba.mmu.cartridge {
                        if let Some(prof) = crate::gba::widescreen::WidescreenDatabase::lookup(&cart.game_code, &cart.title) {
                            ui.label(format!("Profile: {} [{}]", prof.title, prof.game_code));
                            ui.label(format!("Safe Settings: {}", prof.notes));
                        } else {
                            ui.label("Profile: Generic 16:9 Expansion");
                        }
                    } else {
                        ui.label("Profile: Generic 16:9 Expansion (No ROM loaded)");
                    }

                    let mut ws_mode = self.widescreen_config.mode;
                    let mut ws_changed = false;
                    ui.horizontal(|ui| {
                        ws_changed |= ui.radio_value(&mut ws_mode, crate::gba::widescreen::WidescreenMode::Ratio16_9, "16:9 (284x160)").changed();
                        ws_changed |= ui.radio_value(&mut ws_mode, crate::gba::widescreen::WidescreenMode::TileAligned16_9, "16:9 Aligned (288x160)").changed();
                        ws_changed |= ui.radio_value(&mut ws_mode, crate::gba::widescreen::WidescreenMode::Ratio16_10, "16:10 Steam Deck").changed();
                    });
                    if ws_changed {
                        self.widescreen_config.mode = ws_mode;
                        self.gba.set_widescreen_config(self.widescreen_config.clone());
                        self.config_dirty = true;
                    }

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
                    ui.label("📐 Aspect Ratio (F3 to cycle):");
                    for &ar in &AspectRatio::ALL {
                        if ui.radio_value(&mut self.aspect_ratio, ar, ar.display_name()).clicked() {
                            self.set_toast(format!("📐 Aspect Ratio: {}", ar.display_name()));
                        }
                    }
                    ui.separator();
                    ui.checkbox(&mut self.screenshot_enhanced, "Screenshot: Capture Enhanced (xBRZ/NIS) vs Raw 1x");
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

                ui.menu_button("Game Boy", |ui| {
                    match self.gb {
                        Some(ref gb) => {
                            let model = if gb.is_cgb() { "Game Boy Color" } else { "Game Boy (DMG)" };
                            ui.label(RichText::new(format!("Running: {}", model)).strong());
                            ui.label(format!("Cartridge: {}", gb.mmu.cart.title));
                            ui.label(format!("Mapper: {:?}", gb.mmu.cart.mbc));
                            ui.label(format!(
                                "ROM banks: {} | RAM: {} KiB{}",
                                gb.mmu.cart.rom_banks,
                                gb.mmu.cart.ram.len() / 1024,
                                if gb.mmu.cart.has_battery { " (battery)" } else { "" }
                            ));
                            if gb.mmu.double_speed {
                                ui.label(RichText::new("CGB double-speed active").weak());
                            }
                        }
                        None => {
                            ui.label(RichText::new("No Game Boy ROM loaded").weak());
                            ui.label("Open a .gb or .gbc file to switch cores.");
                        }
                    }
                    ui.separator();
                    // Boot model is fixed when the system is constructed, so
                    // changing this only affects the next load.
                    if ui
                        .checkbox(&mut self.gb_force_dmg, "Force original Game Boy mode")
                        .on_hover_text(
                            "Run CGB-enhanced cartridges through their DMG code path.\n\
                             Applies on the next ROM load.",
                        )
                        .changed()
                    {
                        self.set_toast(if self.gb_force_dmg {
                            "DMG mode: reload the ROM to apply"
                        } else {
                            "CGB mode: reload the ROM to apply"
                        });
                    }
                    if ui.button("Reload current ROM").clicked() {
                        ui.close_menu();
                        self.set_toast("Use File > Open ROM to reload with the new mode");
                    }
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
                                Ok((_p, name)) => self.set_toast(format!("🎥 Encoding GIF: {}…", name)),
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
                    if ui.button("🤖 AI Agent Player — Watch an AI Play (Ctrl+A)...").clicked() {
                        self.ai_agent_dialog.is_open = true;
                        ui.close_menu();
                    }
                    if ui
                        .button("💬 AI Coach — Upload Game Guide & Give Orders (Ctrl+G)...")
                        .clicked()
                    {
                        self.ai_agent_dialog.chat_open = true;
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
                        self.controls_dialog.open();
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
                    let lock = self.updater.status.lock().unwrap_or_else(|e| e.into_inner());
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
                    if ui.add(egui::Label::new(badge).sense(Sense::click()))
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
                    ui.label(RichText::new(badge).color(badge_col).small().strong());

                    ui.label(RichText::new(&self.loaded_rom_name).color(Color32::LIGHT_GRAY).small());
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
