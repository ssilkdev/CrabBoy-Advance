//! The Android application: game library, emulation loop, in-game menu.

use crate::orientation::Orientation;
use crate::skin::{self, Control, SkinImage, SkinSettings};
use crate::{gamepad, platform, touch};

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, ColorImage, RichText, TextureHandle, TextureOptions};
use gba_simulator::dmg::mmu::GbKey;
use gba_simulator::dmg::GameBoy;
use gba_simulator::gba::accessibility::{
    AccessibilityManager, AccessibilityStore, ColorblindMode, Handedness, SlowMotionAudio, ALL_KEYS,
};
use gba_simulator::gba::frame_blend::{FrameBlendMode, FrameBlender};
use gba_simulator::gba::keypad::Key;
use gba_simulator::gba::ppu::hd_mode7::{HdMode7Config, HdScale};
use gba_simulator::gba::shader::{apply_shader, ShaderPreset};
use gba_simulator::gba::Gba;
use winit::platform::android::activity::AndroidApp;

use touch::{Buttons, TouchPad};

/// Never emulate more than this many frames per UI update, so a long stall
/// (app switch, GC pause) does not turn into a burst of fast-forward.
const MAX_CATCHUP_FRAMES: u32 = 3;
/// Redraw interval while nothing moves (library, menus): picks up ROM
/// imports and expiring toasts without redrawing 60 times a second.
const IDLE_TICK: Duration = Duration::from_millis(500);
const FAST_FORWARD_SPEED: u32 = 3;

#[no_mangle]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("CrabBoy"),
    );
    gamepad::install();
    platform::init(&app);

    let files_dir = app
        .internal_data_path()
        .unwrap_or_else(|| PathBuf::from("/data/local/tmp/crabboy"));

    let options = eframe::NativeOptions {
        android_app: Some(app),
        vsync: true,
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "CrabBoy Advance",
        options,
        Box::new(move |cc| Ok(Box::new(CrabBoyApp::new(cc, files_dir)))),
    ) {
        log::error!("eframe exited with error: {e}");
    }
}

/// The running console.
enum Core {
    Gba(Box<Gba>),
    GameBoy(Box<GameBoy>),
}

impl Core {
    fn load(path: &Path) -> Result<Self, String> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "gb" | "gbc" => GameBoy::from_file(path, false)
                .map(|gb| Core::GameBoy(Box::new(gb)))
                .map_err(|e| e.to_string()),
            "gba" => {
                let mut gba = Box::new(Gba::new());
                gba.load_rom(path).map_err(|e| e.to_string())?;
                Ok(Core::Gba(gba))
            }
            other => Err(format!("Unsupported file type '.{other}'")),
        }
    }

    fn run_frame(&mut self) {
        match self {
            Core::Gba(g) => g.run_frame(),
            Core::GameBoy(g) => g.run_frame(),
        }
    }

    /// Game code and header title, for per-game settings.
    fn game_id(&self) -> (String, String) {
        match self {
            Core::Gba(g) => g
                .mmu
                .cartridge
                .as_ref()
                .map(|c| (c.game_code.clone(), c.title.clone()))
                .unwrap_or_default(),
            Core::GameBoy(g) => (String::new(), g.mmu.cart.title.clone()),
        }
    }

    fn set_buttons(&mut self, b: Buttons) {
        const MAP: [(u16, Key, Option<GbKey>); 10] = [
            (Buttons::A, Key::A, Some(GbKey::A)),
            (Buttons::B, Key::B, Some(GbKey::B)),
            (Buttons::SELECT, Key::Select, Some(GbKey::Select)),
            (Buttons::START, Key::Start, Some(GbKey::Start)),
            (Buttons::RIGHT, Key::Right, Some(GbKey::Right)),
            (Buttons::LEFT, Key::Left, Some(GbKey::Left)),
            (Buttons::UP, Key::Up, Some(GbKey::Up)),
            (Buttons::DOWN, Key::Down, Some(GbKey::Down)),
            (Buttons::R, Key::R, None),
            (Buttons::L, Key::L, None),
        ];
        for (bit, key, gb_key) in MAP {
            let pressed = b.0 & bit != 0;
            match self {
                Core::Gba(g) => g.mmu.keypad.set_key_state(key, pressed),
                Core::GameBoy(g) => {
                    if let Some(k) = gb_key {
                        g.set_key(k, pressed);
                    }
                }
            }
        }
    }

    /// Screen size and RGBA pixels (the cores store 0xAABBGGRR, i.e. RGBA
    /// bytes in little-endian order).
    fn frame_rgba(&self, out: &mut Vec<u8>) -> [usize; 2] {
        let (fb, size): (&[u32], [usize; 2]) = match self {
            Core::Gba(g) => (&g.get_framebuffer()[..], [240, 160]),
            Core::GameBoy(g) => (&g.get_framebuffer()[..], [160, 144]),
        };
        out.clear();
        out.reserve(fb.len() * 4);
        for &px in fb {
            let [r, g, b, _] = px.to_le_bytes();
            out.extend_from_slice(&[r, g, b, 0xFF]);
        }
        size
    }

    /// Write battery-backed save RAM to disk now.
    fn flush_save(&mut self) {
        match self {
            Core::Gba(g) => {
                if let Some(cart) = g.mmu.cartridge.as_mut() {
                    cart.save.sync_to_disk();
                }
            }
            Core::GameBoy(g) => g.mmu.cart.sync_to_disk(),
        }
    }

    fn save_state(&self) -> Vec<u8> {
        match self {
            Core::Gba(g) => g.save_state(),
            Core::GameBoy(g) => g.save_state(),
        }
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        match self {
            Core::Gba(g) => g.load_state(data),
            Core::GameBoy(g) => g.load_state(data),
        }
    }

    /// Run or stop the audio device stream (see `AudioOutput::set_stream_active`).
    fn set_stream_active(&mut self, active: bool) {
        match self {
            Core::Gba(g) => g.mmu.apu.audio_output.set_stream_active(active),
            Core::GameBoy(g) => g.mmu.apu.audio_output.set_stream_active(active),
        }
    }

    /// Cheap fingerprint of the displayed frame, to skip re-uploading a
    /// picture that hasn't changed (menus, text boxes, pauses in-game).
    fn frame_fingerprint(&self) -> u64 {
        let fb: &[u32] = match self {
            Core::Gba(g) => &g.get_framebuffer()[..],
            Core::GameBoy(g) => &g.get_framebuffer()[..],
        };
        // FNV-1a over 64-bit words: ~20 µs for a GBA frame.
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for w in fb.chunks_exact(2) {
            h = (h ^ (w[0] as u64 | (w[1] as u64) << 32)).wrapping_mul(0x100_0000_01b3);
        }
        h
    }

    fn set_audio(&mut self, muted: bool, fast_forward: bool, slow: f32, slow_audio: SlowMotionAudio) {
        let out = match self {
            Core::Gba(g) => &mut g.mmu.apu.audio_output,
            Core::GameBoy(g) => &mut g.mmu.apu.audio_output,
        };
        out.muted = muted;
        out.set_fast_forwarding(fast_forward);
        out.set_slow_motion(if fast_forward { 1.0 } else { slow }, slow_audio);
    }
}

struct Game {
    core: Core,
    rom_path: PathBuf,
    title: String,
}

#[derive(PartialEq)]
enum Screen {
    Library,
    Playing,
}

struct CrabBoyApp {
    roms_dir: PathBuf,
    states_dir: PathBuf,
    library: Vec<PathBuf>,
    game: Option<Game>,
    screen: Screen,
    menu_open: bool,
    texture: Option<TextureHandle>,
    rgba: Vec<u8>,
    touch: TouchPad,
    last_tick: Instant,
    pacer: gba_simulator::frame_pacing::FramePacer,
    frame_accum: f64,
    fast_forward: bool,
    muted: bool,
    was_focused: bool,
    toast: Option<(String, Instant)>,
    insets: platform::Insets,
    last_inset_poll: Instant,
    /// Frames emulated / UI frames drawn since `stats_since` (logged for tuning).
    stats: (u32, u32),
    stats_since: Instant,
    frame_blender: FrameBlender,
    blend_mode: FrameBlendMode,
    shader_preset: ShaderPreset,
    hd_mode7: HdMode7Config,
    /// Per-game accessibility settings (ROADMAP M10), saved to
    /// `accessibility.json` in the app's files directory.
    accessibility: AccessibilityManager,
    accessibility_path: PathBuf,
    accessibility_menu: bool,
    /// Zoom factor the UI was last set to.
    applied_zoom: f32,
    /// Screen rotation preference, saved to `orientation.txt`.
    orientation: Orientation,
    orientation_path: PathBuf,
    /// Periodic auto-saves (3-file rotation in `states/`), separate from
    /// the manual state. Settings saved to `autosave.txt`.
    autosaver: gba_simulator::autosave::AutoSaver,
    autosave_path: PathBuf,
    autosave_menu: bool,
    /// Fingerprint + settings of the picture last uploaded to the GPU.
    uploaded: Option<(u64, u64)>,
    /// Whether the 60 Hz display-mode request is in effect.
    game_refresh: bool,
    /// Time spent converting/uploading pictures since `stats_since`.
    upload_time: Duration,
    /// On-screen control skin, layout and visibility (`skin.json`).
    skin: SkinSettings,
    skin_path: PathBuf,
    skins_dir: PathBuf,
    /// The active skin's textures, loaded on demand.
    skin_textures: std::collections::BTreeMap<SkinImage, TextureHandle>,
    /// Animated skin images and their settings.
    skin_animations: std::collections::BTreeMap<SkinImage, skin::Animation>,
    /// Clock for skin animations.
    skin_clock: Instant,
    /// The active custom skin, if any (its colours, images and layout).
    custom_skin: Option<skin::InstalledSkin>,
    skin_textures_for: String,
    skin_menu: bool,
    /// Layout editor: which control is selected, and whether it's open.
    layout_editor: Option<Option<Control>>,
    installed_skins: Vec<skin::InstalledSkin>,
    /// Pending quick-load confirmation timestamp for accidental-tap protection.
    load_confirm: Option<Instant>,
}

impl CrabBoyApp {
    fn new(cc: &eframe::CreationContext<'_>, files_dir: PathBuf) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        cc.egui_ctx.style_mut(|s| {
            s.spacing.button_padding = egui::vec2(14.0, 10.0);
            s.spacing.item_spacing = egui::vec2(10.0, 10.0);
            s.spacing.interact_size.y = 44.0;
            for (_, font) in s.text_styles.iter_mut() {
                font.size *= 1.25;
            }
        });

        let roms_dir = files_dir.join("roms");
        let states_dir = files_dir.join("states");
        for dir in [&roms_dir, &states_dir] {
            if let Err(e) = std::fs::create_dir_all(dir) {
                log::error!("Cannot create {}: {e}", dir.display());
            }
        }
        let accessibility_path = files_dir.join("accessibility.json");
        let store = std::fs::read_to_string(&accessibility_path)
            .map(|s| AccessibilityStore::from_json(&s))
            .unwrap_or_default();
        let accessibility = AccessibilityManager::with_store(store);
        let orientation_path = files_dir.join("orientation.txt");
        let orientation = std::fs::read_to_string(&orientation_path)
            .map(|s| Orientation::parse(&s))
            .unwrap_or_default();
        platform::set_orientation(orientation.activity_info());
        let skin_path = files_dir.join("skin.json");
        let skin_settings = SkinSettings::parse(&std::fs::read_to_string(&skin_path).unwrap_or_default());
        let skins_dir = files_dir.join("skins");
        let _ = std::fs::create_dir_all(&skins_dir);
        let autosave_path = files_dir.join("autosave.txt");
        let (as_on, as_min) = crate::autosave_settings::parse(&std::fs::read_to_string(&autosave_path).unwrap_or_default());
        let autosaver = gba_simulator::autosave::AutoSaver::new(states_dir.clone(), as_on, as_min);
        let mut app = Self {
            roms_dir,
            states_dir,
            library: Vec::new(),
            game: None,
            screen: Screen::Library,
            menu_open: false,
            texture: None,
            rgba: Vec::with_capacity(240 * 160 * 4),
            touch: TouchPad::default(),
            last_tick: Instant::now(),
            pacer: Default::default(),
            frame_accum: 0.0,
            fast_forward: false,
            muted: false,
            was_focused: true,
            toast: None,
            insets: platform::Insets::default(),
            last_inset_poll: Instant::now() - Duration::from_secs(10),
            stats: (0, 0),
            stats_since: Instant::now(),
            frame_blender: FrameBlender::new(),
            blend_mode: FrameBlendMode::Off,
            shader_preset: ShaderPreset::Crisp,
            hd_mode7: HdMode7Config::default(),
            accessibility,
            accessibility_path,
            accessibility_menu: false,
            applied_zoom: 0.0,
            orientation,
            orientation_path,
            autosaver,
            autosave_path,
            autosave_menu: false,
            uploaded: None,
            game_refresh: false,
            upload_time: Duration::ZERO,
            skin: skin_settings,
            skin_path,
            skins_dir,
            skin_textures: Default::default(),
            skin_animations: Default::default(),
            skin_clock: Instant::now(),
            custom_skin: None,
            skin_textures_for: String::new(),
            skin_menu: false,
            layout_editor: None,
            installed_skins: Vec::new(),
            load_confirm: None,
        };
        app.refresh_skins();
        app.refresh_library();
        app
    }

    fn refresh_library(&mut self) {
        let mut roms: Vec<PathBuf> = std::fs::read_dir(&self.roms_dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| is_rom(p))
                    .collect()
            })
            .unwrap_or_default();
        roms.sort_by_key(|p| display_name(p).to_lowercase());
        self.library = roms;
    }

    fn toast(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        log::info!("{msg}");
        self.toast = Some((msg, Instant::now()));
    }

    /// Step to the next rotation mode, apply it and remember it.
    fn cycle_orientation(&mut self) {
        self.orientation = self.orientation.next();
        platform::set_orientation(self.orientation.activity_info());
        if let Err(e) = std::fs::write(&self.orientation_path, self.orientation.as_str()) {
            log::warn!("Could not save rotation setting: {e}");
        }
    }

    /// Save accessibility edits for the current game and write them out.
    fn commit_accessibility(&mut self) {
        // Always write: "use for all games" / "reset" change the store
        // without `commit` reporting it.
        self.accessibility.commit();
        {
            if let Err(e) = std::fs::write(&self.accessibility_path, self.accessibility.store.to_json()) {
                log::warn!("Could not save accessibility settings: {e}");
            }
        }
    }

    fn start_game(&mut self, path: PathBuf) {
        // Persist the outgoing game's battery save before its core is dropped.
        if let Some(g) = self.game.as_mut() {
            g.core.flush_save();
        }
        match Core::load(&path) {
            Ok(mut core) => {
                if let Core::Gba(ref mut gba) = core {
                    gba.set_hd_mode7_config(self.hd_mode7);
                }
                let (code, header_title) = core.game_id();
                self.accessibility.load_for_game(&code, &header_title);
                let sm = self.accessibility.active.slow_motion;
                core.set_audio(self.muted, false, sm.effective_multiplier(), sm.audio);
                let title = display_name(&path);
                self.toast(format!("Loaded {title}"));
                self.game = Some(Game { core, rom_path: path, title });
                self.screen = Screen::Playing;
                self.menu_open = false;
                self.fast_forward = false;
                self.frame_accum = 0.0;
                self.last_tick = Instant::now();
                self.autosaver.restart_timer();
            }
            Err(e) => self.toast(format!("Could not load {}: {e}", display_name(&path))),
        }
    }

    fn close_game(&mut self) {
        if let Some(mut g) = self.game.take() {
            g.core.flush_save();
        }
        self.accessibility.unload_game();
        self.screen = Screen::Library;
        self.menu_open = false;
        self.texture = None;
        self.uploaded = None;
    }

    fn state_path(&self) -> Option<PathBuf> {
        let g = self.game.as_ref()?;
        let stem = g.rom_path.file_stem()?.to_string_lossy().to_string();
        Some(self.states_dir.join(format!("{stem}.state")))
    }

    fn save_state(&mut self) {
        self.load_confirm = None;
        let (Some(path), Some(g)) = (self.state_path(), self.game.as_ref()) else { return };
        match std::fs::write(&path, g.core.save_state()) {
            Ok(()) => self.toast("State saved"),
            Err(e) => self.toast(format!("Save state failed: {e}")),
        }
    }

    /// On-screen touch Quick Load: requires a double-tap within 2.5 seconds to
    /// prevent catastrophic accidental loads during gameplay.
    fn quick_load_with_confirm(&mut self) {
        let Some(path) = self.state_path() else { return };
        if !path.exists() {
            self.toast("No saved state for this game yet");
            return;
        }
        let now = Instant::now();
        if let Some(t) = self.load_confirm {
            if now.duration_since(t) < Duration::from_millis(2500) {
                self.load_confirm = None;
                self.load_state();
                return;
            }
        }
        self.load_confirm = Some(now);
        self.toast("Tap LOAD again to confirm");
    }

    fn load_state(&mut self) {
        self.load_confirm = None;
        let Some(path) = self.state_path() else { return };
        let Ok(data) = std::fs::read(&path) else {
            self.toast("No saved state for this game yet");
            return;
        };
        let ok = self.game.as_mut().is_some_and(|g| g.core.load_state(&data));
        self.toast(if ok { "State loaded" } else { "Saved state is incompatible" });
    }

    /// Name the auto-saves are filed under: the ROM file's stem.
    fn autosave_key(&self) -> Option<String> {
        let g = self.game.as_ref()?;
        Some(g.rom_path.file_stem()?.to_string_lossy().to_string())
    }

    fn write_autosave(&mut self) -> Result<usize, String> {
        let key = self.autosave_key().ok_or("No game running")?;
        let data = self.game.as_ref().map(|g| g.core.save_state()).unwrap_or_default();
        self.autosaver.write(&key, &data).map_err(|e| format!("Auto-save failed: {e}"))
    }

    fn load_autosave(&mut self, number: usize) {
        let Some(key) = self.autosave_key() else { return };
        let Ok(data) = self.autosaver.read(&key, number) else {
            self.toast("That auto-save is gone");
            return;
        };
        let ok = self.game.as_mut().is_some_and(|g| g.core.load_state(&data));
        if ok {
            self.autosaver.restart_timer();
        }
        self.toast(if ok { format!("Loaded auto-save {number}") } else { "Auto-save is incompatible".into() });
    }

    fn store_autosave_settings(&self) {
        let body = crate::autosave_settings::format(self.autosaver.enabled, self.autosaver.interval_minutes());
        if let Err(e) = std::fs::write(&self.autosave_path, body) {
            log::warn!("Could not save auto-save settings: {e}");
        }
    }

    /// A sheet over the game (settings, editor) is open: the game pauses.
    fn any_sheet_open(&self) -> bool {
        self.accessibility_menu || self.autosave_menu || self.skin_menu || self.layout_editor.is_some()
    }

    fn store_skin_settings(&self) {
        if let Err(e) = std::fs::write(&self.skin_path, self.skin.to_json()) {
            log::warn!("Could not save skin settings: {e}");
        }
    }

    /// Rescan installed skins and re-resolve the active one.
    fn refresh_skins(&mut self) {
        self.installed_skins = skin::list_installed(&self.skins_dir);
        self.custom_skin = self.installed_skins.iter().find(|s| s.id == self.skin.skin).cloned();
        if self.custom_skin.is_none() && skin::built_in(&self.skin.skin).is_none() {
            self.skin.skin = "classic".into();
        }
        self.skin_textures_for.clear(); // reload textures
    }

    fn theme(&self) -> skin::Theme {
        match &self.custom_skin {
            Some(c) => c.manifest.theme(),
            None => skin::built_in(&self.skin.skin).map_or(skin::Theme::CLASSIC, |b| b.theme),
        }
    }

    /// Load the active skin's images as textures (once per skin).
    fn ensure_skin_textures(&mut self, ctx: &egui::Context) {
        if self.skin_textures_for == self.skin.skin {
            return;
        }
        self.skin_textures.clear();
        self.skin_animations.clear();
        self.skin_textures_for = self.skin.skin.clone();
        let Some(custom) = &self.custom_skin else { return };
        self.skin_animations = custom.animations();
        let animations = self.skin_animations.clone();
        for (what, path) in custom.images() {
            let loaded = std::fs::read(&path)
                .ok()
                .and_then(|b| image::load_from_memory_with_format(&b, image::ImageFormat::Png).ok());
            match loaded {
                Some(img) => {
                    let rgba = img.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    let ci = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                    let name = format!("skin-{what:?}");
                    // Scrolling images repeat; everything else clamps.
                    let scrolls = animations.get(&what).is_some_and(|a| a.scroll_x != 0.0 || a.scroll_y != 0.0);
                    let options = if scrolls {
                        TextureOptions { wrap_mode: egui::TextureWrapMode::Repeat, ..TextureOptions::LINEAR }
                    } else {
                        TextureOptions::LINEAR
                    };
                    self.skin_textures.insert(what, ctx.load_texture(name, ci, options));
                }
                None => log::warn!("Skin image {} could not be loaded", path.display()),
            }
        }
    }

    /// Touch layout for this frame: the default spots with the skin's and
    /// the user's placements applied.
    fn control_layout(&self, safe: egui::Rect, hand: touch::Hand) -> touch::Layout {
        let mut layout = touch::Layout::compute_for(safe, self.texture.as_ref().map(|t| t.size()), hand);
        let portrait = safe.height() > safe.width();
        // Moves made in the editor are for the standard two-handed layout;
        // one-handed layouts (Accessibility) stack every control in one
        // column, where those offsets would pile buttons on top of each
        // other. They still get the skin's look and the global size.
        let placements = if hand != touch::Hand::Both {
            Default::default()
        } else {
            let user = self.skin.layout.for_orientation(portrait);
            match &self.custom_skin {
                Some(c) => skin::combined(c.manifest.layout.for_orientation(portrait), user),
                None => user.clone(),
            }
        };
        skin::apply(&mut layout, safe, &placements, self.skin.scale);
        layout
    }

    fn import_skin(&mut self, path: &Path) {
        let result = std::fs::read(path)
            .map_err(|e| format!("Could not read the skin: {e}"))
            .and_then(|bytes| skin::read_pack(&bytes));
        let _ = std::fs::remove_file(path);
        match result {
            Ok(pack) => {
                let fallback = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                match skin::install(&pack, &self.skins_dir, &fallback) {
                    Ok(id) => {
                        self.skin.skin = id;
                        self.store_skin_settings();
                        self.refresh_skins();
                        let name = self.custom_skin.as_ref().map(|c| c.display_name().to_string()).unwrap_or_default();
                        self.toast(format!("Skin \"{name}\" installed"));
                    }
                    Err(e) => self.toast(format!("Could not install the skin: {e}")),
                }
            }
            Err(e) => self.toast(e),
        }
    }

    /// Poll the Java side for a freshly imported ROM or import error.
    fn poll_imports(&mut self) {
        if let Some(err) = platform::take_import_error() {
            self.toast(err);
        }
        if let Some(path) = platform::take_imported_skin() {
            self.import_skin(Path::new(&path));
        }
        if let Some(path) = platform::take_imported_rom() {
            self.refresh_library();
            self.start_game(PathBuf::from(path));
        }
    }

    /// Run as many emulated frames as real time calls for.
    fn step_emulation(&mut self, buttons: Buttons) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;
        let playing = self.game.is_some() && !(self.menu_open || self.any_sheet_open());
        if self.autosaver.tick(Duration::from_secs_f64(dt), playing) {
            match self.write_autosave() {
                Ok(_) => self.toast("Auto-saved"),
                Err(e) => self.toast(e),
            }
        }
        let paused = self.menu_open || self.any_sheet_open();
        let Some(game) = self.game.as_mut() else { return };
        if paused {
            self.frame_accum = 0.0;
            return;
        }

        // Toggle-instead-of-hold (ROADMAP M10). The GBA button bits in
        // `Buttons` match `Key` numbering, so the mask passes straight through.
        let game_bits = buttons.0 & 0x3FF;
        let latched = self.accessibility.process_mask(game_bits);
        game.core.set_buttons(Buttons((buttons.0 & !0x3FF) | latched));
        let speed = if self.fast_forward {
            FAST_FORWARD_SPEED as f64
        } else {
            self.accessibility.active.slow_motion.effective_multiplier() as f64
        };
        // Lock to a ~60 Hz display: one frame per vsync (see FramePacer).
        let base_rate = self.pacer.base_rate(Duration::from_secs_f64(dt));
        let dt = if base_rate == 60.0 && (0.75 / 60.0..1.25 / 60.0).contains(&dt) { 1.0 / 60.0 } else { dt };
        self.frame_accum = (self.frame_accum + dt * base_rate * speed).min(MAX_CATCHUP_FRAMES as f64 * speed.max(1.0));
        let started = Instant::now();
        while self.frame_accum >= 1.0 {
            game.core.run_frame();
            self.frame_accum -= 1.0;
            self.stats.0 += 1;
        }
        self.stats.1 += 1;
        if self.stats_since.elapsed() >= Duration::from_secs(5) {
            let secs = self.stats_since.elapsed().as_secs_f64();
            log::info!(
                "perf: {:.1} emulated fps, {:.1} UI fps, last batch {:.2} ms, picture {:.3} ms/UI frame",
                self.stats.0 as f64 / secs,
                self.stats.1 as f64 / secs,
                started.elapsed().as_secs_f64() * 1000.0,
                self.upload_time.as_secs_f64() * 1000.0 / self.stats.1.max(1) as f64
            );
            self.stats = (0, 0);
            self.upload_time = Duration::ZERO;
            self.stats_since = Instant::now();
        }
    }

    fn upload_frame(&mut self, ctx: &egui::Context) {
        let Some(game) = self.game.as_ref() else { return };

        // Same picture, same settings, nothing time-dependent in the
        // pipeline: the GPU texture is already right. Skips the colour
        // conversion, shader pass and texture upload (a big share of the
        // per-frame CPU work) whenever the game's picture is still.
        let static_pipeline = self.blend_mode == FrameBlendMode::Off
            && self.hd_mode7.scale == HdScale::Off
            && !matches!(&game.core, Core::Gba(g) if g.is_hd_pack_enabled());
        let settings = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&format!("{:?}{:?}", self.shader_preset, self.accessibility.active), &mut h);
            std::hash::Hasher::finish(&h)
        };
        let key = (game.core.frame_fingerprint(), settings);
        if static_pipeline && self.texture.is_some() && self.uploaded == Some(key) {
            return;
        }
        self.uploaded = if static_pipeline { Some(key) } else { None };

        // HD Mode 7 & HD Pack Rendering (ROADMAP M6, M7)
        if let Core::Gba(ref gba) = game.core {
            if self.hd_mode7.scale != HdScale::Off || gba.is_hd_pack_enabled() {
                if let Some(hd) = gba.render_hd_frame() {
                    if !self.hd_mode7.ssaa {
                        let hd_size = [hd.width, hd.height];
                        let mut hd_px = hd.pixels.clone();
                        self.accessibility.filter_framebuffer(&mut hd_px);
                        let mut hd_bytes = Vec::with_capacity(hd.width * hd.height * 4);
                        for &pixel in &hd_px {
                            hd_bytes.extend_from_slice(&pixel.to_le_bytes());
                        }
                        let image = ColorImage::from_rgba_unmultiplied(hd_size, &hd_bytes);
                        match self.texture.as_mut() {
                            Some(t) if t.size() == hd_size => t.set(image, TextureOptions::NEAREST),
                            _ => self.texture = Some(ctx.load_texture("screen", image, TextureOptions::NEAREST)),
                        }
                        return;
                    } else {
                        let mut ssaa_words = hd.downsample_ssaa();
                        self.accessibility.filter_framebuffer(&mut ssaa_words[..]);
                        let blended = self.frame_blender.blend(&ssaa_words, self.blend_mode);
                        let mut post_shader = [0u32; 240 * 160];
                        apply_shader(blended, &mut post_shader, self.shader_preset, None);
                        self.rgba.resize(240 * 160 * 4, 0);
                        for (i, &word) in post_shader.iter().enumerate() {
                            self.rgba[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
                        }
                        let image = ColorImage::from_rgba_unmultiplied([240, 160], &self.rgba);
                        match self.texture.as_mut() {
                            Some(t) if t.size() == [240, 160] => t.set(image, TextureOptions::NEAREST),
                            _ => self.texture = Some(ctx.load_texture("screen", image, TextureOptions::NEAREST)),
                        }
                        return;
                    }
                }
            }
        }

        let size = game.core.frame_rgba(&mut self.rgba);
        if size != [240, 160] && self.accessibility.active.colorblind_mode != ColorblindMode::None {
            // Game Boy frames skip the GBA shader path below; filter here.
            let mut words: Vec<u32> = self.rgba.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            self.accessibility.filter_framebuffer(&mut words);
            for (dst, w) in self.rgba.chunks_exact_mut(4).zip(words) {
                dst.copy_from_slice(&w.to_le_bytes());
            }
        }

        // Apply frame blending and shader pipeline on 240x160 GBA framebuffers (ROADMAP M5)
        if size == [240, 160] {
            let mut words = [0u32; 240 * 160];
            for (i, chunk) in self.rgba.chunks_exact(4).enumerate() {
                words[i] = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            }

            self.accessibility.filter_framebuffer(&mut words);
            let blended = self.frame_blender.blend(&words, self.blend_mode);
            let mut post_shader = [0u32; 240 * 160];
            apply_shader(blended, &mut post_shader, self.shader_preset, None);

            for (i, &word) in post_shader.iter().enumerate() {
                let bytes = word.to_le_bytes();
                self.rgba[i * 4..i * 4 + 4].copy_from_slice(&bytes);
            }
        }

        let image = ColorImage::from_rgba_unmultiplied(size, &self.rgba);
        match self.texture.as_mut() {
            Some(t) if t.size() == size => t.set(image, TextureOptions::NEAREST),
            _ => self.texture = Some(ctx.load_texture("screen", image, TextureOptions::NEAREST)),
        }
    }

    fn library_ui(&mut self, ctx: &egui::Context) {
        let insets = self.insets.to_points(ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(insets.margin(12.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(RichText::new("CrabBoy Advance").strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Import ROM").clicked() {
                            platform::pick_rom();
                        }
                        if self.game.is_some() && ui.button("Resume").clicked() {
                            self.screen = Screen::Playing;
                        }
                    });
                });
                ui.separator();

                if self.library.is_empty() {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        ui.label("No games yet.");
                        ui.label("Tap \"Import ROM\" and pick a .gba, .gb or .gbc file from your phone.");
                        ui.label("Have a save from another emulator? Import its .sav the same way.");
                        ui.add_space(12.0);
                        if ui.button(RichText::new("Import ROM").size(22.0)).clicked() {
                            platform::pick_rom();
                        }
                    });
                    return;
                }

                let mut launch = None;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for path in &self.library {
                        let tag = match path.extension().and_then(|e| e.to_str()) {
                            Some(e) if e.eq_ignore_ascii_case("gba") => "GBA",
                            Some(e) if e.eq_ignore_ascii_case("gbc") => "GBC",
                            _ => "GB",
                        };
                        let text = RichText::new(format!("{tag}   {}", display_name(path))).size(20.0);
                        let button = egui::Button::new(text).min_size(egui::vec2(ui.available_width(), 56.0));
                        if ui.add(button).clicked() {
                            launch = Some(path.clone());
                        }
                    }
                });
                if let Some(p) = launch {
                    self.start_game(p);
                }
            });
    }

    fn game_ui(&mut self, ctx: &egui::Context, layout: &touch::Layout) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(self.theme().colors(1.0).background))
            .show(ctx, |ui| {
                let painter = ui.painter();
                let full = ctx.screen_rect();
                let portrait = full.height() > full.width();
                let t = self.skin_clock.elapsed().as_secs_f64();
                let full_uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                let bg_key = SkinImage::Background { portrait };
                if let Some(bg) = self.skin_textures.get(&bg_key) {
                    let uv = self.skin_animations.get(&bg_key).map_or(full_uv, |a| a.uv(t));
                    painter.image(bg.id(), full, uv, Color32::WHITE);
                }
                if let Some(tex) = &self.texture {
                    painter.image(
                        tex.id(),
                        layout.screen,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                let colors = self.theme().colors(self.skin.opacity);
                let textures = &self.skin_textures;
                let animations = &self.skin_animations;
                let images = |c: Control, pressed: bool| {
                    let key = SkinImage::Control(c, pressed);
                    let uv = animations.get(&key).map_or(full_uv, |a| a.uv(t));
                    textures.get(&key).map(|tex| (tex.id(), uv))
                };
                let look = touch::Look { colors, images: &images };
                let show_controls = match self.skin.visibility {
                    skin::Visibility::Always => true,
                    skin::Visibility::Auto => !gamepad::recently_used(),
                    skin::Visibility::MenuOnly => false,
                } || self.layout_editor.is_some();
                if show_controls {
                    touch::paint(painter, layout, self.touch.held(), self.fast_forward, &look);
                }
                // The menu button stays visible even when a controller hides the rest.
                touch::paint_menu_button(painter, layout, &look);
            });

        if self.menu_open {
            self.menu_ui(ctx);
        }
    }

    fn menu_ui(&mut self, ctx: &egui::Context) {
        let title = self.game.as_ref().map(|g| g.title.clone()).unwrap_or_default();
        egui::Window::new("Menu")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(260.0);
                // Taller than a landscape phone: scroll rather than clip.
                let max_h = (ctx.screen_rect().height() - 32.0).max(120.0);
                egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
                    ui.vertical_centered_justified(|ui| {
                        ui.label(RichText::new(title).strong());
                        ui.separator();
                        if ui.button("Resume").clicked() {
                            self.menu_open = false;
                        }
                        if ui.button("Save state").clicked() {
                            self.save_state();
                            self.menu_open = false;
                        }
                        if ui.button("Load state").clicked() {
                            self.load_state();
                            self.menu_open = false;
                        }
                        if ui.button("Auto-saves...").clicked() {
                            self.autosave_menu = true;
                        }
                        let ff = if self.fast_forward { "Fast forward: ON" } else { "Fast forward: off" };
                        if ui.button(ff).clicked() {
                            self.fast_forward = !self.fast_forward;
                            self.menu_open = false;
                        }
                        let mute = if self.muted { "Sound: muted" } else { "Sound: on" };
                        if ui.button(mute).clicked() {
                            self.muted = !self.muted;
                        }
                        if let Some(ref mut g) = self.game {
                            match g.core {
                                Core::Gba(ref mut gba) => {
                                    let sur_label = match gba.mmu.apu.audio_output.surround_mode() {
                                        gba_simulator::gba::apu::SurroundMode::Stereo => "Output: Pure Stereo",
                                        gba_simulator::gba::apu::SurroundMode::Headphone3D => "Output: 3D Headphones",
                                        gba_simulator::gba::apu::SurroundMode::Surround51 => "Output: 5.1 Surround",
                                    };
                                    if ui.button(sur_label).clicked() {
                                        let next = match gba.mmu.apu.audio_output.surround_mode() {
                                            gba_simulator::gba::apu::SurroundMode::Stereo => gba_simulator::gba::apu::SurroundMode::Headphone3D,
                                            gba_simulator::gba::apu::SurroundMode::Headphone3D => gba_simulator::gba::apu::SurroundMode::Surround51,
                                            gba_simulator::gba::apu::SurroundMode::Surround51 => gba_simulator::gba::apu::SurroundMode::Stereo,
                                        };
                                        gba.mmu.apu.audio_output.set_surround_mode(next);
                                    }

                                    let audio_label = match gba.hd_audio_mode() {
                                        gba_simulator::gba::m4a::AudioEngineMode::HardwareOnly => "Audio: Native APU",
                                        gba_simulator::gba::m4a::AudioEngineMode::HdReSynthesis => "Audio: HD Re-Synthesis",
                                    };
                                    if ui.button(audio_label).clicked() {
                                        let next = match gba.hd_audio_mode() {
                                            gba_simulator::gba::m4a::AudioEngineMode::HardwareOnly => gba_simulator::gba::m4a::AudioEngineMode::HdReSynthesis,
                                            gba_simulator::gba::m4a::AudioEngineMode::HdReSynthesis => gba_simulator::gba::m4a::AudioEngineMode::HardwareOnly,
                                        };
                                        gba.set_hd_audio_mode(next);
                                    }
                                }
                                Core::GameBoy(ref mut gb) => {
                                    let sur_label = match gb.mmu.apu.audio_output.surround_mode() {
                                        gba_simulator::gba::apu::SurroundMode::Stereo => "Output: Pure Stereo",
                                        gba_simulator::gba::apu::SurroundMode::Headphone3D => "Output: 3D Headphones",
                                        gba_simulator::gba::apu::SurroundMode::Surround51 => "Output: 5.1 Surround",
                                    };
                                    if ui.button(sur_label).clicked() {
                                        let next = match gb.mmu.apu.audio_output.surround_mode() {
                                            gba_simulator::gba::apu::SurroundMode::Stereo => gba_simulator::gba::apu::SurroundMode::Headphone3D,
                                            gba_simulator::gba::apu::SurroundMode::Headphone3D => gba_simulator::gba::apu::SurroundMode::Surround51,
                                            gba_simulator::gba::apu::SurroundMode::Surround51 => gba_simulator::gba::apu::SurroundMode::Stereo,
                                        };
                                        gb.mmu.apu.audio_output.set_surround_mode(next);
                                    }
                                }
                            }
                        }
                        let blend_label = match self.blend_mode {
                            FrameBlendMode::Off => "Blend: Off (Instant)",
                            FrameBlendMode::Simple50 => "Blend: 50/50",
                            FrameBlendMode::SmartDeFlicker => "Blend: De-Flicker",
                            FrameBlendMode::LcdGhosting { .. } => "Blend: LCD Ghosting",
                        };
                        if ui.button(blend_label).clicked() {
                            self.blend_mode = match self.blend_mode {
                                FrameBlendMode::Off => FrameBlendMode::Simple50,
                                FrameBlendMode::Simple50 => FrameBlendMode::SmartDeFlicker,
                                FrameBlendMode::SmartDeFlicker => FrameBlendMode::LcdGhosting { decay: 0.65 },
                                FrameBlendMode::LcdGhosting { .. } => FrameBlendMode::Off,
                            };
                        }
                        let shader_label = match self.shader_preset {
                            ShaderPreset::Crisp => "Shader: Crisp",
                            ShaderPreset::Linear => "Shader: Bilinear",
                            ShaderPreset::LcdGrid => "Shader: LCD Grid",
                            ShaderPreset::LcdSubpixel => "Shader: LCD Subpixels",
                            ShaderPreset::CrtScanlines => "Shader: CRT Scanlines",
                            ShaderPreset::CrtGeom => "Shader: CRT Aperture",
                            _ => "Shader: Crisp",
                        };
                        if ui.button(shader_label).clicked() {
                            self.shader_preset = match self.shader_preset {
                                ShaderPreset::Crisp => ShaderPreset::LcdGrid,
                                ShaderPreset::LcdGrid => ShaderPreset::LcdSubpixel,
                                ShaderPreset::LcdSubpixel => ShaderPreset::CrtScanlines,
                                ShaderPreset::CrtScanlines => ShaderPreset::CrtGeom,
                                ShaderPreset::CrtGeom => ShaderPreset::Linear,
                                _ => ShaderPreset::Crisp,
                            };
                        }
                        let hd_label = match self.hd_mode7.scale {
                            HdScale::Off => "Mode 7: Native (Off)",
                            HdScale::X2 => "Mode 7: 2x HD",
                            HdScale::X4 => "Mode 7: 4x HD",
                            HdScale::X8 => "Mode 7: 8x Ultra HD",
                        };
                        if ui.button(hd_label).clicked() {
                            self.hd_mode7.scale = match self.hd_mode7.scale {
                                HdScale::Off => HdScale::X2,
                                HdScale::X2 => HdScale::X4,
                                HdScale::X4 => HdScale::X8,
                                HdScale::X8 => HdScale::Off,
                            };
                            if let Some(ref mut g) = self.game {
                                if let Core::Gba(ref mut gba) = g.core {
                                    gba.set_hd_mode7_config(self.hd_mode7);
                                }
                            }
                        }
                        if ui.button(self.orientation.label()).clicked() {
                            self.cycle_orientation();
                        }
                        if ui.button("Skin & controls...").clicked() {
                            self.refresh_skins();
                            self.skin_menu = true;
                        }
                        if ui.button("Accessibility...").clicked() {
                            self.accessibility_menu = true;
                        }
                        if ui.button("Reset").clicked() {
                            if let Some(p) = self.game.as_ref().map(|g| g.rom_path.clone()) {
                                self.start_game(p);
                            }
                        }
                        if ui.button("Game library").clicked() {
                            if let Some(g) = self.game.as_mut() {
                                g.core.flush_save();
                            }
                            self.menu_open = false;
                            self.screen = Screen::Library;
                            self.refresh_library();
                        }
                        if ui.button("Close game").clicked() {
                            self.close_game();
                        }
                    });
                });
            });
    }
}

impl CrabBoyApp {
    /// Accessibility settings sheet (ROADMAP M10). Every change is saved for
    /// the running game right away.
    fn autosave_ui(&mut self, ctx: &egui::Context) {
        use gba_simulator::autosave::{MAX_INTERVAL_MINUTES, MIN_INTERVAL_MINUTES, ROTATION};
        let list = self.autosave_key().map(|k| self.autosaver.list(&k)).unwrap_or_default();
        let mut load = None;
        let mut save_now = false;
        let mut changed = false;
        let mut close = false;
        egui::Window::new("Auto-saves")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(280.0);
                let max_h = (ctx.screen_rect().height() - 32.0).max(120.0);
                egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
                    ui.vertical_centered_justified(|ui| {
                        ui.label(RichText::new("Auto-saves").strong());
                        ui.separator();
                        for n in 1..=ROTATION {
                            match list.iter().find(|e| e.number == n) {
                                Some(e) => {
                                    let newest = list.first().map(|f| f.number) == Some(n);
                                    let label = format!(
                                        "Load auto-save {n} · {}{}",
                                        e.age_label(),
                                        if newest { " (newest)" } else { "" }
                                    );
                                    if ui.button(label).clicked() {
                                        load = Some(n);
                                    }
                                }
                                None => {
                                    ui.add_enabled(false, egui::Button::new(format!("Auto-save {n}: empty")));
                                }
                            }
                        }
                        if ui.button("Auto-save now").clicked() {
                            save_now = true;
                        }
                        ui.separator();
                        let on = if self.autosaver.enabled { "Auto-save: ON" } else { "Auto-save: off" };
                        if ui.button(on).clicked() {
                            self.autosaver.enabled = !self.autosaver.enabled;
                            changed = true;
                        }
                        let m = self.autosaver.interval_minutes();
                        ui.add_enabled_ui(self.autosaver.enabled, |ui| {
                            ui.horizontal(|ui| {
                                if ui.add_enabled(m > MIN_INTERVAL_MINUTES, egui::Button::new("  −  ")).clicked() {
                                    self.autosaver.set_interval_minutes(m - 1);
                                    changed = true;
                                }
                                ui.label(format!("Every {} min", self.autosaver.interval_minutes()));
                                if ui.add_enabled(m < MAX_INTERVAL_MINUTES, egui::Button::new("  +  ")).clicked() {
                                    self.autosaver.set_interval_minutes(m + 1);
                                    changed = true;
                                }
                            });
                        });
                        ui.label(
                            RichText::new(format!(
                                "Keeps the last {ROTATION}; the oldest is replaced. Separate from Save state."
                            ))
                            .weak(),
                        );
                        if ui.button("Back").clicked() {
                            close = true;
                        }
                    });
                });
            });
        if changed {
            self.store_autosave_settings();
        }
        if save_now {
            match self.write_autosave() {
                Ok(n) => self.toast(format!("Auto-saved (auto-save {n})")),
                Err(e) => self.toast(e),
            }
        }
        if let Some(n) = load {
            self.load_autosave(n);
            self.autosave_menu = false;
        }
        if close {
            self.autosave_menu = false;
        }
    }

    fn skin_ui(&mut self, ctx: &egui::Context) {
        let mut close = false;
        let mut changed = false;
        let mut edit = false;
        let mut remove: Option<String> = None;
        let custom: Vec<(String, String)> =
            self.installed_skins.iter().map(|s| (s.id.clone(), s.display_name().to_string())).collect();
        egui::Window::new("Skin")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(290.0);
                let max_h = (ctx.screen_rect().height() - 32.0).max(120.0);
                egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
                    ui.vertical_centered_justified(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Skin & controls").strong());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("Back").clicked() {
                                    close = true;
                                }
                            });
                        });
                        ui.separator();
                        ui.label(RichText::new("Skin").weak());
                        for b in skin::BUILT_IN.iter() {
                            let on = self.skin.skin == b.id;
                            if ui.selectable_label(on, if on { format!("✔ {}", b.name) } else { b.name.to_string() }).clicked() {
                                self.skin.skin = b.id.to_string();
                                changed = true;
                            }
                        }
                        for (id, name) in &custom {
                            let on = &self.skin.skin == id;
                            ui.horizontal(|ui| {
                                let label = if on { format!("✔ {name}") } else { name.clone() };
                                if ui.selectable_label(on, label).clicked() {
                                    self.skin.skin = id.clone();
                                    changed = true;
                                }
                                if ui.small_button("Remove").clicked() {
                                    remove = Some(id.clone());
                                }
                            });
                        }
                        if ui.button("Import skin (.zip)...").clicked() {
                            platform::pick_skin();
                        }
                        ui.separator();
                        // "−  Size 100%  +" steppers across the full width.
                        let stepper = |ui: &mut egui::Ui, label: String| -> i32 {
                            let mut step = 0;
                            ui.columns(3, |c| {
                                if c[0].add_sized([c[0].available_width(), 44.0], egui::Button::new("−")).clicked() {
                                    step = -1;
                                }
                                c[1].centered_and_justified(|ui| ui.label(label));
                                if c[2].add_sized([c[2].available_width(), 44.0], egui::Button::new("+")).clicked() {
                                    step = 1;
                                }
                            });
                            step
                        };
                        let step = stepper(ui, format!("Size {:.0}%", self.skin.scale * 100.0));
                        if step != 0 {
                            self.skin.scale = (self.skin.scale + 0.1 * step as f32).clamp(skin::MIN_SCALE, skin::MAX_SCALE);
                            changed = true;
                        }
                        let step = stepper(ui, format!("Opacity {:.0}%", self.skin.opacity * 100.0));
                        if step != 0 {
                            self.skin.opacity = (self.skin.opacity + 0.1 * step as f32).clamp(skin::MIN_OPACITY, 1.0);
                            changed = true;
                        }
                        if ui.button(self.skin.visibility.label()).clicked() {
                            self.skin.visibility = self.skin.visibility.next();
                            changed = true;
                        }
                        ui.separator();
                        if ui.button("Move & resize buttons...").clicked() {
                            edit = true;
                        }
                        if !self.skin.layout.is_empty() && ui.button("Reset button layout").clicked() {
                            self.skin.layout = Default::default();
                            changed = true;
                        }
                        if ui.button("Back").clicked() {
                            close = true;
                        }
                    });
                });
            });
        if let Some(id) = remove {
            match skin::uninstall(&self.skins_dir, &id) {
                Ok(()) => self.toast("Skin removed"),
                Err(e) => self.toast(format!("Could not remove the skin: {e}")),
            }
            if self.skin.skin == id {
                self.skin.skin = "classic".into();
            }
            changed = true;
        }
        if changed {
            self.skin.sanitize();
            self.store_skin_settings();
            self.refresh_skins();
        }
        if edit {
            self.skin_menu = false;
            self.layout_editor = Some(None);
        }
        if close {
            self.skin_menu = false;
            self.menu_open = true;
        }
    }

    /// Drag controls to move them; the bar at the top resizes the selected
    /// one. Saved per orientation.
    fn layout_editor_ui(&mut self, ctx: &egui::Context, safe: egui::Rect, hand: touch::Hand, layout: &touch::Layout) {
        let portrait = safe.height() > safe.width();
        let Some(mut selected) = self.layout_editor else { return };
        if hand != touch::Hand::Both {
            egui::Window::new("One-handed")
                .title_bar(false)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.set_max_width(280.0);
                    ui.label("One-handed controls are on for this game (Accessibility). Buttons can be moved in the standard layout only.");
                    if ui.button("OK").clicked() {
                        self.layout_editor = None;
                        self.skin_menu = true;
                    }
                });
            return;
        }
        let mut changed = false;

        // Drag handling on a full-screen layer under the toolbar.
        let resp = egui::Area::new(egui::Id::new("layout-editor-drag"))
            .fixed_pos(safe.min)
            .order(egui::Order::Middle)
            .show(ctx, |ui| ui.allocate_rect(egui::Rect::from_min_size(safe.min, safe.size()), egui::Sense::click_and_drag()))
            .inner;
        if resp.drag_started() || resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                selected = skin::control_at(layout, p);
            }
        }
        if resp.dragged() {
            if let Some(c) = selected {
                skin::drag(self.skin.layout.for_orientation_mut(portrait), c, resp.drag_delta(), safe);
                changed = true;
            }
        }
        // Highlight every control; the selected one strongly.
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("layout-editor-paint")));
        for c in Control::ALL {
            let r = skin::control_rect(layout, c);
            let (w, col) = if Some(c) == selected { (3.0_f32, Color32::YELLOW) } else { (1.5_f32, Color32::from_white_alpha(120)) };
            painter.rect_stroke(r, 6.0, egui::Stroke::new(w, col), egui::StrokeKind::Outside);
        }

        let mut done = false;
        egui::Area::new(egui::Id::new("layout-editor-bar"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, safe.top() + 8.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let name = selected.map_or("Tap a button", |c| c.label());
                        ui.label(RichText::new(name).strong());
                        ui.add_enabled_ui(selected.is_some(), |ui| {
                            if ui.button(" − ").clicked() {
                                skin::resize(self.skin.layout.for_orientation_mut(portrait), selected.unwrap(), 1.0 / 1.1);
                                changed = true;
                            }
                            if ui.button(" + ").clicked() {
                                skin::resize(self.skin.layout.for_orientation_mut(portrait), selected.unwrap(), 1.1);
                                changed = true;
                            }
                            if ui.button("Reset").clicked() {
                                self.skin.layout.for_orientation_mut(portrait).remove(&selected.unwrap());
                                changed = true;
                            }
                        });
                        if ui.button("Done").clicked() {
                            done = true;
                        }
                    });
                    ui.label(
                        RichText::new(if portrait { "Portrait layout. Drag to move." } else { "Landscape layout. Drag to move." })
                            .weak(),
                    );
                });
            });
        if changed {
            self.skin.sanitize();
            self.store_skin_settings();
        }
        self.layout_editor = if done { None } else { Some(selected) };
        if done {
            self.skin_menu = true;
        }
    }

    fn accessibility_ui(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut changed = false;
        let title = self.accessibility.current_game_id.clone().unwrap_or_else(|| "All games".into());
        // Keep the sheet on screen at every interface size.
        let max_w = (ctx.screen_rect().width() - 24.0).max(200.0);
        egui::Window::new("Accessibility")
            .collapsible(false)
            .resizable(false)
            .max_width(max_w)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .open(&mut open)
            .show(ctx, |ui| {
                let max_h = ctx.screen_rect().height() * 0.8;
                egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
                    ui.set_min_width(280.0f32.min(max_w));
                    ui.set_max_width(max_w);
                    let p = &mut self.accessibility.active;
                    ui.label(RichText::new(format!("Saved for: {title}")).weak());
                    ui.separator();
                    ui.vertical_centered_justified(|ui| {
                        if ui.button(format!("Colors: {}", p.colorblind_mode.display_name())).clicked() {
                            p.colorblind_mode = p.colorblind_mode.next();
                            changed = true;
                        }
                        if p.colorblind_mode != ColorblindMode::None {
                            changed |= ui
                                .add(egui::Slider::new(&mut p.colorblind_intensity, 0.1..=1.0).text("strength"))
                                .changed();
                        }
                        let sm = &mut p.slow_motion;
                        let sm_label = if sm.enabled {
                            format!("Slow motion: {:.0}%", sm.speed_factor * 100.0)
                        } else {
                            "Slow motion: off".to_string()
                        };
                        if ui.button(sm_label).clicked() {
                            sm.cycle();
                            changed = true;
                        }
                        if sm.enabled && ui.button(format!("Slow-motion sound: {}", sm.audio.display_name())).clicked() {
                            sm.audio = match sm.audio {
                                SlowMotionAudio::PitchPreserved => SlowMotionAudio::Tape,
                                SlowMotionAudio::Tape => SlowMotionAudio::Mute,
                                SlowMotionAudio::Mute => SlowMotionAudio::PitchPreserved,
                            };
                            changed = true;
                        }
                        if ui.button(format!("Touch layout: {}", p.one_handed_touch.display_name())).clicked() {
                            p.one_handed_touch = p.one_handed_touch.next();
                            changed = true;
                        }
                        if ui.button(format!("Interface size: {:.0}%", p.ui_scale_clamped() * 100.0)).clicked() {
                            let steps = [1.0, 1.25, 1.5, 1.75, 2.0, 0.75];
                            let i = steps.iter().position(|&v| (v - p.ui_scale).abs() < 0.01).map_or(0, |i| i + 1);
                            p.ui_scale = steps[i % steps.len()];
                            changed = true;
                        }
                    });
                    ui.separator();
                    ui.label("Toggle instead of hold:");
                    ui.horizontal_wrapped(|ui| {
                        for key in ALL_KEYS {
                            let st = &mut p.sticky_buttons;
                            let mut on = st.is_key_toggle_enabled(key);
                            if ui.checkbox(&mut on, gba_simulator::gba::accessibility::key_name(key)).changed() {
                                st.set_key_toggle_enabled(key, on);
                                changed = true;
                            }
                        }
                    });
                    ui.separator();
                    ui.vertical_centered_justified(|ui| {
                        if self.accessibility.current_game_id.is_some() {
                            if ui.button("Use for all games").clicked() {
                                self.accessibility.set_as_global_default();
                                changed = true;
                            }
                            if ui.button("Reset this game to default").clicked() {
                                self.accessibility.reset_game_to_default();
                                changed = true;
                            }
                        }
                        if ui.button("Done").clicked() {
                            self.accessibility_menu = false;
                        }
                    });
                });
            });
        if !open {
            self.accessibility_menu = false;
        }
        if changed {
            self.commit_accessibility();
        }
    }
}

impl eframe::App for CrabBoyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.last_inset_poll.elapsed() > Duration::from_secs(1) {
            self.insets = platform::safe_insets();
            self.last_inset_poll = Instant::now();
        }
        self.poll_imports();

        // Interface scale (ROADMAP M10).
        let zoom = self.accessibility.active.ui_scale_clamped();
        if (zoom - self.applied_zoom).abs() > 0.001 {
            ctx.set_zoom_factor(zoom);
            self.applied_zoom = zoom;
        }

        // Leaving the app (home button, incoming call, file picker): flush
        // saves right away, since Android may kill a backgrounded process
        // without warning. Emulation pauses on its own while backgrounded
        // because the app stops receiving frames.
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if self.was_focused && !focused {
            if let Some(g) = self.game.as_mut() {
                g.core.flush_save();
                g.core.set_stream_active(false);
            }
        }
        if !self.was_focused && focused {
            // Don't fast-forward through the time spent in the background.
            self.last_tick = Instant::now();
        }
        self.was_focused = focused;

        // Back button / controller Select+Start toggle the in-game menu.
        let back = gamepad::take_back_request() || ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let pad_menu = gamepad::take_menu_request();
        if (back || pad_menu) && self.screen == Screen::Playing {
            if self.layout_editor.is_some() {
                // Back from the editor returns to the skin sheet.
                self.layout_editor = None;
                self.skin_menu = true;
            } else if self.autosave_menu || self.skin_menu {
                // Back from a sheet returns to the menu.
                self.autosave_menu = false;
                self.skin_menu = false;
                self.menu_open = true;
            } else {
                self.menu_open = !self.menu_open;
            }
        }

        match self.screen {
            Screen::Library => self.library_ui(ctx),
            Screen::Playing if self.game.is_some() => {
                let safe = self.insets.to_points(ctx).shrink(ctx.screen_rect());
                let hand = match self.accessibility.active.one_handed_touch {
                    Handedness::Standard => touch::Hand::Both,
                    Handedness::LeftHand => touch::Hand::Left,
                    Handedness::RightHand => touch::Hand::Right,
                };
                self.ensure_skin_textures(ctx);
                let layout = self.control_layout(safe, hand);
                let touch = if self.menu_open || self.any_sheet_open() {
                    self.touch.clear();
                    Buttons::default()
                } else {
                    self.touch.update(ctx, &layout)
                };
                if self.touch.just_pressed(Buttons::MENU) {
                    self.menu_open = true;
                }
                if self.touch.just_pressed(Buttons::FAST) {
                    self.fast_forward = !self.fast_forward;
                }
                if self.touch.just_pressed(Buttons::QUICK_SAVE) {
                    self.save_state();
                }
                if self.touch.just_pressed(Buttons::QUICK_LOAD) {
                    self.quick_load_with_confirm();
                }
                let buttons = Buttons(touch.0 | gamepad::buttons().0);

                let (muted, ff) = (self.muted, self.fast_forward);
                let sm = self.accessibility.active.slow_motion;
                if let Some(g) = self.game.as_mut() {
                    g.core.set_audio(muted, ff, sm.effective_multiplier(), sm.audio);
                }
                self.step_emulation(buttons);
                let t = Instant::now();
                self.upload_frame(ctx);
                self.upload_time += t.elapsed();
                // The texture size is known now; recompute for the first frame.
                let layout = self.control_layout(safe, hand);
                self.game_ui(ctx, &layout);
                if self.layout_editor.is_some() {
                    self.menu_open = false;
                    self.layout_editor_ui(ctx, safe, hand, &layout);
                }
                if self.skin_menu {
                    self.menu_open = false;
                    self.skin_ui(ctx);
                }
                if self.accessibility_menu {
                    self.menu_open = false;
                    self.accessibility_ui(ctx);
                }
                if self.autosave_menu {
                    self.menu_open = false;
                    self.autosave_ui(ctx);
                }
            }
            Screen::Playing => self.screen = Screen::Library,
        }

        if let Some((msg, at)) = &self.toast {
            if at.elapsed() < Duration::from_secs(3) {
                let insets = self.insets.to_points(ctx);
                egui::Area::new(egui::Id::new("toast"))
                    .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0 - insets.bottom))
                    .interactable(false)
                    .show(ctx, |ui| {
                        let max_w = (ctx.screen_rect().width() - 48.0).max(120.0);
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.set_max_width(max_w);
                            ui.add(egui::Label::new(msg.as_str()).wrap_mode(egui::TextWrapMode::Wrap))
                        });
                    });
            } else {
                self.toast = None;
            }
        }

        // Battery: only redraw continuously while a game is running. The
        // library and menus redraw on touch (egui does that by itself),
        // plus a slow tick for ROM imports, toasts and screen insets.
        let emulating = self.screen == Screen::Playing
            && self.game.is_some()
            && !(self.menu_open || self.any_sheet_open());
        let animating = self.screen == Screen::Playing && !self.skin_animations.is_empty();
        if emulating {
            ctx.request_repaint();
        } else if animating {
            // Animated skin behind a menu: ~30 fps is plenty.
            ctx.request_repaint_after(Duration::from_millis(33));
        } else {
            let toast_left = self
                .toast
                .as_ref()
                .map(|(_, at)| Duration::from_secs(3).saturating_sub(at.elapsed()))
                .filter(|d| !d.is_zero());
            ctx.request_repaint_after(toast_left.unwrap_or(IDLE_TICK).min(IDLE_TICK));
        }
        if emulating != self.game_refresh {
            self.game_refresh = emulating;
            platform::set_game_refresh_rate(emulating);
        }
        // No sound to play: stop the audio device so it can sleep.
        let sound = emulating && focused && !self.muted;
        if let Some(g) = self.game.as_mut() {
            g.core.set_stream_active(sound);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(g) = self.game.as_mut() {
            g.core.flush_save();
        }
    }
}

fn is_rom(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("gba" | "gb" | "gbc")
    )
}

fn display_name(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

