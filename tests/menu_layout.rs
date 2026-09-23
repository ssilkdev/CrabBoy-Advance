//! Menu bar layout: every menu and settings window must fit on screen.
//!
//! Runs the whole desktop app headless (eframe's kittest constructors),
//! clicks menus through egui's AccessKit tree and measures what opens.

use egui::accesskit::{Node, NodeId, Role};
use gba_simulator::ui::GbaApp;

struct Harness {
    ctx: egui::Context,
    app: GbaApp,
    frame: eframe::Frame,
    size: egui::Vec2,
    tree: Vec<(NodeId, Node)>,
    events: Vec<egui::Event>,
}

fn rect(n: &Node) -> egui::Rect {
    let b = n.bounds().expect("bounds");
    egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32))
}

impl Harness {
    fn new(w: f32, h: f32) -> Self {
        let dir = std::env::temp_dir().join(format!("crabboy-menu-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let prev = gba_simulator::gba::apu::audio_output::set_headless(true);
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let app = GbaApp::new(&cc, None);
        gba_simulator::gba::apu::audio_output::set_headless(prev);
        let mut h = Self {
            ctx,
            app,
            frame: eframe::Frame::_new_kittest(),
            size: egui::vec2(w, h),
            tree: Vec::new(),
            events: Vec::new(),
        };
        h.step();
        h.step();
        h
    }

    fn step(&mut self) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)),
            events: std::mem::take(&mut self.events),
            ..Default::default()
        };
        let (app, frame) = (&mut self.app, &mut self.frame);
        let out = self.ctx.run(input, |ctx| eframe::App::update(app, ctx, frame));
        self.tree = out.platform_output.accesskit_update.expect("accesskit").nodes;
    }

    fn find(&self, label: &str) -> Option<egui::Rect> {
        self.tree.iter().find_map(|(_, n)| {
            let name = n.label().or_else(|| n.value()).unwrap_or_default();
            (name == label && matches!(n.role(), Role::Button | Role::MenuItem | Role::Tab)).then(|| rect(n))
        })
    }

    fn click(&mut self, label: &str) {
        let r = self.find(label).unwrap_or_else(|| panic!("no '{label}': {:?}", self.labels()));
        let p = r.center();
        self.events.push(egui::Event::PointerMoved(p));
        for pressed in [true, false] {
            self.events.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
            self.step();
        }
        self.step();
    }

    /// Hover a button (opens a submenu) without clicking.
    fn hover(&mut self, label: &str) {
        let r = self.find(label).unwrap_or_else(|| panic!("no '{label}': {:?}", self.labels()));
        self.events.push(egui::Event::PointerMoved(r.center()));
        self.step();
        self.step();
        self.step();
    }

    /// Open ☰ Menu > `category` (the category's submenu shows).
    fn open_category(&mut self, category: &str) {
        self.click("☰ Menu");
        self.hover(&category_label(category));
    }

    fn labels(&self) -> Vec<String> {
        self.tree.iter().filter_map(|(_, n)| n.label().or_else(|| n.value()).map(|s| s.to_string())).collect()
    }

    /// Vertical extent (top, bottom) of every item in the open menus.
    /// A menu too tall for the screen is pushed up, so its first items end
    /// up above y=0 even though its last one fits: both edges matter.
    fn menu_extent(&self) -> (f32, f32) {
        self.tree
            .iter()
            .filter(|(_, n)| matches!(n.role(), Role::Button | Role::CheckBox | Role::RadioButton | Role::Label | Role::MenuItem | Role::Slider))
            .filter_map(|(_, n)| n.bounds())
            .fold((f32::MAX, 0.0), |(t, b), r| (t.min(r.y0 as f32), b.max(r.y1 as f32)))
    }
}

/// Category names as shown in ☰ Menu.
const CATEGORIES: [&str; 9] = ["File", "Emulation", "Video", "Audio", "Game Boy", "Tools", "Controls", "Help", "Debug"];

fn category_label(c: &str) -> String {
    let icon = match c {
        "File" => "📂",
        "Emulation" => "⏱",
        "Video" => "🖥",
        "Audio" => "🔊",
        "Game Boy" => "🕹",
        "Tools" => "🛠",
        "Controls" => "🎮",
        "Help" => "📖",
        "Debug" => "🔬",
        _ => "",
    };
    format!("{icon} {c}")
}

fn window_rect(h: &Harness, title: &str) -> Option<egui::Rect> {
    h.tree.iter().find_map(|(_, n)| (n.role() == Role::Window && n.label() == Some(title)).then(|| rect(n)))
}

fn on_screen(h: &Harness, r: egui::Rect) -> bool {
    r.min.x >= -0.5 && r.min.y >= -0.5 && r.max.x <= h.size.x + 0.5 && r.max.y <= h.size.y + 0.5
}

#[test]
fn settings_window_fits_and_works_on_small_screens() {
    for (w, hgt) in [(1280.0, 720.0), (1024.0, 600.0)] {
        let mut h = Harness::new(w, hgt);
        h.click("⚙ Settings");
        let win = window_rect(&h, "Settings").expect("window opens");
        assert!(on_screen(&h, win), "{w}x{hgt}: window {win:?} off screen");
        for page in ["🎨 Picture", "✨ Enhance", "🖼 HD & Widescreen", "🖥 Screen & Frame", "⏱ Speed & Latency", "🔊 Sound"] {
            h.click(page);
            let win = window_rect(&h, "Settings").unwrap();
            assert!(on_screen(&h, win), "{w}x{hgt} {page}: window {win:?} off screen");
            // Every control on the page is inside the window (scrolled
            // content is clipped to it, never drawn below the screen).
            for (_, n) in &h.tree {
                if matches!(n.role(), Role::Button | Role::CheckBox | Role::ComboBox | Role::Slider) {
                    let r = rect(n);
                    if r.center().x > win.min.x && r.center().x < win.max.x {
                        assert!(r.max.y <= hgt + 0.5, "{w}x{hgt} {page}: {:?} at {r:?} below the screen", n.label());
                    }
                }
            }
        }

        // The settings still work: HD Mode 7 4x through the window.
        h.click("🖼 HD & Widescreen");
        h.click("4×");
        assert_eq!(h.app.gba.mmu.ppu.hd_config.scale, gba_simulator::gba::ppu::hd_mode7::HdScale::X4);
        // Aspect ratio through its combo box.
        h.click("🖥 Screen & Frame");
        let before = h.app.aspect_ratio;
        let combo = h
            .tree
            .iter()
            .find(|(_, n)| n.role() == Role::ComboBox && n.label() == Some(before.display_name()))
            .map(|(_, n)| rect(n))
            .expect("aspect combo");
        let p = combo.center();
        h.events.push(egui::Event::PointerMoved(p));
        for pressed in [true, false] {
            h.events.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
            h.step();
        }
        h.step();
        let target = gba_simulator::ui::screen::AspectRatio::ALL[4];
        h.click(target.display_name());
        assert_eq!(h.app.aspect_ratio, target, "aspect combo didn't apply");
    }
}

#[test]
fn every_menu_fits_on_a_720p_screen() {
    let mut h = Harness::new(1280.0, 720.0);
    let mut report = Vec::new();
    let mut failures = Vec::new();
    for m in CATEGORIES {
        h.open_category(m);
        let (top, bottom) = h.menu_extent();
        report.push(format!("{m}: items span y={top:.0}..{bottom:.0}"));
        if top < 0.0 || bottom > h.size.y {
            failures.push(format!("{m} runs off the screen (y={top:.0}..{bottom:.0})"));
        }
        // Close it again.
        h.events.push(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
        h.step();
        h.step();
    }
    println!("{}", report.join("\n"));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Every menu's "⚙ … settings…" opens the Settings window at its section,
/// and settings from the new pages take effect.
#[test]
fn menus_open_settings_at_their_section_and_settings_apply() {
    let mut h = Harness::new(1280.0, 720.0);
    for (menu, item, heading) in [
        ("Video", "⚙ Display settings…", "🎨 Picture"),
        ("Emulation", "⚙ Emulation settings…", "⏱ Speed & Latency"),
        ("Audio", "⚙ Audio settings…", "🔊 Sound"),
    ] {
        h.open_category(menu);
        h.click(item);
        let shown = h.tree.iter().any(|(_, n)| n.role() == Role::Label && n.value() == Some(heading));
        assert!(shown, "{menu} > {item} should open {heading}");
    }
    // Speed page: 2x fast.
    h.click("⏱ Speed & Latency");
    h.click("2×");
    assert_eq!(h.app.speed_multiplier, 2);
    // Run-ahead 1 frame.
    h.click("1 frame");
    assert_eq!(h.app.run_ahead.frames, 1);
    // Sound page: surround mode through its combo box.
    h.click("🔊 Sound");
    let combo = h
        .tree
        .iter()
        .find(|(_, n)| n.role() == Role::ComboBox)
        .map(|(_, n)| rect(n))
        .expect("surround combo");
    let p = combo.center();
    h.events.push(egui::Event::PointerMoved(p));
    for pressed in [true, false] {
        h.events.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
        h.step();
    }
    h.step();
    h.click("Stereo");
    assert_eq!(h.app.gba.mmu.apu.audio_output.surround_mode(), gba_simulator::gba::apu::audio_output::SurroundMode::Stereo);
}

/// Menu items show their keyboard shortcut, right-aligned. egui paints the
/// shortcut as separate text, so look for it among the painted shapes.
#[test]
fn menu_items_show_shortcuts() {
    let mut h = Harness::new(1280.0, 720.0);
    h.open_category("Tools");
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, h.size)),
        ..Default::default()
    };
    let (app, frame) = (&mut h.app, &mut h.frame);
    let out = h.ctx.run(input, |ctx| eframe::App::update(app, ctx, frame));
    let mut texts: Vec<(String, egui::Pos2)> = Vec::new();
    for cs in &out.shapes {
        collect(&cs.shape, &mut texts);
    }
    fn collect(s: &egui::epaint::Shape, out: &mut Vec<(String, egui::Pos2)>) {
        match s {
            egui::epaint::Shape::Text(t) => out.push((t.galley.text().to_string(), t.pos)),
            egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
            _ => {}
        }
    }
    let item = texts.iter().find(|(t, _)| t.contains("Cheats")).expect("Cheats item painted");
    let sc = texts
        .iter()
        .find(|(t, p)| t == "Ctrl+C" && (p.y - item.1.y).abs() < 4.0)
        .expect("Ctrl+C painted on the same line as Cheats");
    assert!(sc.1.x > item.1.x + 150.0, "shortcut is right-aligned: {:?} vs {:?}", sc.1, item.1);
}

/// The status text on the right of the menu bar never overlaps the menus,
/// at any window width.
#[test]
fn menu_bar_status_never_overlaps_the_menus() {
    for w in [640.0, 760.0, 900.0, 1280.0] {
        let mut h = Harness::new(w, 600.0);
        h.app.loaded_rom_name = "Pokemon - Emerald Version (USA, Europe).gba".into();
        h.step();
        h.step();
        let menus: Vec<egui::Rect> = ["☰ Menu", "📂 Open", "⏸ Pause", "⚙ Settings"]
            .iter()
            .filter_map(|m| h.find(m))
            .collect();
        assert_eq!(menus.len(), 4, "{w}px: bar should have the menu and three actions: {:?}", h.labels());
        let right_edge = menus.iter().map(|r| r.max.x).fold(0.0, f32::max);
        // Everything else in the bar (y < 24) that isn't a menu button.
        for (_, n) in &h.tree {
            let Some(b) = n.bounds() else { continue };
            if b.y1 > 26.0 || b.x1 <= b.x0 {
                continue;
            }
            let r = rect(n);
            let is_menu = menus.iter().any(|m| (m.center() - r.center()).length() < 1.0);
            if is_menu || !matches!(n.role(), Role::Label | Role::Button) {
                continue;
            }
            assert!(r.min.x >= right_edge - 0.5, "{w}px: {:?} {:?} at {r:?} overlaps the menus (end at {right_edge})", n.label(), n.value());
            assert!(r.max.x <= w + 0.5, "{w}px: {:?} runs past the window edge", n.value());
        }
    }
}

/// The bar is simplified: one menu with every category, plus Open, Pause
/// and Settings. The categories themselves are not on the bar any more.
#[test]
fn toolbar_is_one_menu_plus_quick_actions() {
    let mut h = Harness::new(1280.0, 720.0);
    let bar: Vec<String> = h
        .tree
        .iter()
        .filter(|(_, n)| n.role() == Role::Button && n.bounds().is_some_and(|b| b.y1 < 26.0))
        .filter_map(|(_, n)| n.label().map(String::from))
        .collect();
    for c in CATEGORIES {
        assert!(!bar.iter().any(|b| b == c), "{c} is still on the bar: {bar:?}");
    }
    for want in ["☰ Menu", "📂 Open", "⏸ Pause", "⚙ Settings"] {
        assert!(bar.iter().any(|b| b == want), "{want} missing from the bar: {bar:?}");
    }
    // Every category is inside the menu.
    h.click("☰ Menu");
    for c in CATEGORIES {
        assert!(h.find(&category_label(c)).is_some(), "{c} missing from ☰ Menu");
    }
    // Settings button toggles the window.
    h.events.push(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    h.step();
    h.click("⚙ Settings");
    assert!(h.app.settings_window.is_open);
}
