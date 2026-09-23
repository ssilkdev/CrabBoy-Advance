//! The Android application: game library, emulation loop, in-game menu.

use crate::{gamepad, platform, touch};

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, ColorImage, RichText, TextureHandle, TextureOptions};
use gba_simulator::dmg::mmu::GbKey;
use gba_simulator::dmg::GameBoy;
use gba_simulator::gba::keypad::Key;
use gba_simulator::gba::Gba;
use winit::platform::android::activity::AndroidApp;

use touch::{Buttons, TouchPad};

/// GBA refresh rate (16.78 MHz / 280,896 cycles per frame).
const GBA_FPS: f64 = 59.7275;
/// Never emulate more than this many frames per UI update, so a long stall
/// (app switch, GC pause) does not turn into a burst of fast-forward.
const MAX_CATCHUP_FRAMES: u32 = 3;
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

    fn set_audio(&mut self, muted: bool, fast_forward: bool) {
        let out = match self {
            Core::Gba(g) => &mut g.mmu.apu.audio_output,
            Core::GameBoy(g) => &mut g.mmu.apu.audio_output,
        };
        out.muted = muted;
        out.set_fast_forwarding(fast_forward);
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
            frame_accum: 0.0,
            fast_forward: false,
            muted: false,
            was_focused: true,
            toast: None,
            insets: platform::Insets::default(),
            last_inset_poll: Instant::now() - Duration::from_secs(10),
            stats: (0, 0),
            stats_since: Instant::now(),
        };
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

    fn start_game(&mut self, path: PathBuf) {
        // Persist the outgoing game's battery save before its core is dropped.
        if let Some(g) = self.game.as_mut() {
            g.core.flush_save();
        }
        match Core::load(&path) {
            Ok(mut core) => {
                core.set_audio(self.muted, false);
                let title = display_name(&path);
                self.toast(format!("Loaded {title}"));
                self.game = Some(Game { core, rom_path: path, title });
                self.screen = Screen::Playing;
                self.menu_open = false;
                self.fast_forward = false;
                self.frame_accum = 0.0;
                self.last_tick = Instant::now();
            }
            Err(e) => self.toast(format!("Could not load {}: {e}", display_name(&path))),
        }
    }

    fn close_game(&mut self) {
        if let Some(mut g) = self.game.take() {
            g.core.flush_save();
        }
        self.screen = Screen::Library;
        self.menu_open = false;
        self.texture = None;
    }

    fn state_path(&self) -> Option<PathBuf> {
        let g = self.game.as_ref()?;
        let stem = g.rom_path.file_stem()?.to_string_lossy().to_string();
        Some(self.states_dir.join(format!("{stem}.state")))
    }

    fn save_state(&mut self) {
        let (Some(path), Some(g)) = (self.state_path(), self.game.as_ref()) else { return };
        match std::fs::write(&path, g.core.save_state()) {
            Ok(()) => self.toast("State saved"),
            Err(e) => self.toast(format!("Save state failed: {e}")),
        }
    }

    fn load_state(&mut self) {
        let Some(path) = self.state_path() else { return };
        let Ok(data) = std::fs::read(&path) else {
            self.toast("No saved state for this game yet");
            return;
        };
        let ok = self.game.as_mut().is_some_and(|g| g.core.load_state(&data));
        self.toast(if ok { "State loaded" } else { "Saved state is incompatible" });
    }

    /// Poll the Java side for a freshly imported ROM or import error.
    fn poll_imports(&mut self) {
        if let Some(err) = platform::take_import_error() {
            self.toast(err);
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
        let Some(game) = self.game.as_mut() else { return };
        if self.menu_open {
            self.frame_accum = 0.0;
            return;
        }

        game.core.set_buttons(buttons);
        let speed = if self.fast_forward { FAST_FORWARD_SPEED } else { 1 };
        self.frame_accum = (self.frame_accum + dt * GBA_FPS * speed as f64)
            .min((MAX_CATCHUP_FRAMES * speed) as f64);
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
                "perf: {:.1} emulated fps, {:.1} UI fps, last batch {:.2} ms",
                self.stats.0 as f64 / secs,
                self.stats.1 as f64 / secs,
                started.elapsed().as_secs_f64() * 1000.0
            );
            self.stats = (0, 0);
            self.stats_since = Instant::now();
        }
    }

    fn upload_frame(&mut self, ctx: &egui::Context) {
        let Some(game) = self.game.as_ref() else { return };
        let size = game.core.frame_rgba(&mut self.rgba);
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
            .frame(egui::Frame::NONE.fill(Color32::from_rgb(12, 12, 16)))
            .show(ctx, |ui| {
                let painter = ui.painter();
                if let Some(tex) = &self.texture {
                    painter.image(
                        tex.id(),
                        layout.screen,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                if !gamepad::recently_used() {
                    touch::paint(painter, layout, self.touch.held(), self.fast_forward);
                }
                // The menu button stays visible even when a controller hides the rest.
                touch::paint_menu_button(painter, layout);
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
                    let ff = if self.fast_forward { "Fast forward: ON" } else { "Fast forward: off" };
                    if ui.button(ff).clicked() {
                        self.fast_forward = !self.fast_forward;
                        self.menu_open = false;
                    }
                    let mute = if self.muted { "Sound: muted" } else { "Sound: on" };
                    if ui.button(mute).clicked() {
                        self.muted = !self.muted;
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
    }
}

impl eframe::App for CrabBoyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.last_inset_poll.elapsed() > Duration::from_secs(1) {
            self.insets = platform::safe_insets();
            self.last_inset_poll = Instant::now();
        }
        self.poll_imports();

        // Leaving the app (home button, incoming call, file picker): flush
        // saves right away, since Android may kill a backgrounded process
        // without warning. Emulation pauses on its own while backgrounded
        // because the app stops receiving frames.
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if self.was_focused && !focused {
            if let Some(g) = self.game.as_mut() {
                g.core.flush_save();
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
            self.menu_open = !self.menu_open;
        }

        match self.screen {
            Screen::Library => self.library_ui(ctx),
            Screen::Playing if self.game.is_some() => {
                let safe = self.insets.to_points(ctx).shrink(ctx.screen_rect());
                let layout = touch::Layout::compute(safe, self.texture.as_ref().map(|t| t.size()));
                let touch = if self.menu_open {
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
                let buttons = Buttons(touch.0 | gamepad::buttons().0);

                let (muted, ff) = (self.muted, self.fast_forward);
                if let Some(g) = self.game.as_mut() {
                    g.core.set_audio(muted, ff);
                }
                self.step_emulation(buttons);
                self.upload_frame(ctx);
                // The texture size is known now; recompute for the first frame.
                let layout = touch::Layout::compute(safe, self.texture.as_ref().map(|t| t.size()));
                self.game_ui(ctx, &layout);
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

        ctx.request_repaint();
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
