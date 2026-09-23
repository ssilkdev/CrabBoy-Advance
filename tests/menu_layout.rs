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

fn window_rect(h: &Harness, title: &str) -> Option<egui::Rect> {
    h.tree.iter().find_map(|(_, n)| (n.role() == Role::Window && n.label() == Some(title)).then(|| rect(n)))
}

fn on_screen(h: &Harness, r: egui::Rect) -> bool {
    r.min.x >= -0.5 && r.min.y >= -0.5 && r.max.x <= h.size.x + 0.5 && r.max.y <= h.size.y + 0.5
}

#[test]
fn display_settings_fit_and_work_on_small_screens() {
    for (w, hgt) in [(1280.0, 720.0), (1024.0, 600.0)] {
        let mut h = Harness::new(w, hgt);
        h.click("Video");
        h.click("⚙ Display Settings…");
        let win = window_rect(&h, "Display Settings").expect("window opens");
        assert!(on_screen(&h, win), "{w}x{hgt}: window {win:?} off screen");
        for page in ["🎨 Picture", "✨ Enhance", "🖼 HD & Widescreen", "🖥 Screen & Frame"] {
            h.click(page);
            let win = window_rect(&h, "Display Settings").unwrap();
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
    let menus = ["File", "Emulation", "Video", "Audio", "Game Boy", "Tools", "Controls", "Help"];
    let mut report = Vec::new();
    let mut failures = Vec::new();
    for m in menus {
        h.click(m);
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
