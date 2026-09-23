//! ROADMAP M11: the Memory Map dialog, driven like a player would.
//!
//! Runs the real `MemoryMapDialog` in a headless egui context against a real
//! (tiny, hand-assembled-free) GBA core, clicking buttons found by label
//! through egui's AccessKit tree and typing into its text fields:
//! record observations in two sessions, press Discover, answer "Ask me"
//! questions, save, and reload the map as a new game load would.

use egui::accesskit::{Node, NodeId, Role, TreeUpdate};
use gba_simulator::gba::memmap::{Location, MemoryMap};
use gba_simulator::gba::Gba;
use gba_simulator::ui::debug::memory::WatchEntry;
use gba_simulator::ui::memmap_dialog::MemoryMapDialog;

struct Harness {
    ctx: egui::Context,
    dialog: MemoryMapDialog,
    gba: Gba,
    watch: Vec<WatchEntry>,
    toast: Option<String>,
    tree: Vec<(NodeId, Node)>,
    events: Vec<egui::Event>,
}

impl Harness {
    fn new(gba: Gba) -> Self {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut dialog = MemoryMapDialog::new();
        dialog.is_open = true;
        let mut h = Self { ctx, dialog, gba, watch: Vec::new(), toast: None, tree: Vec::new(), events: Vec::new() };
        h.frame();
        h.frame();
        h
    }

    fn frame(&mut self) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 3000.0))),
            events: std::mem::take(&mut self.events),
            ..Default::default()
        };
        let (d, g, w, t) = (&mut self.dialog, &mut self.gba, &mut self.watch, &mut self.toast);
        let out = self.ctx.run(input, |ctx| d.show(ctx, g, w, t));
        let update: TreeUpdate = out.platform_output.accesskit_update.expect("accesskit enabled");
        // Updates are full trees each frame here.
        self.tree = update.nodes;
    }

    fn find(&self, role: Role, label: &str) -> Option<egui::Rect> {
        self.tree.iter().find_map(|(_, n)| {
            let name = n.label().or_else(|| n.value()).unwrap_or_default();
            (n.role() == role && name.contains(label)).then(|| {
                let b = n.bounds().expect("bounds");
                egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32))
            })
        })
    }

    /// Move + press + release in the same frame, as a quick real click is.
    /// (Hovering first for a frame opens tooltips that then take the click.)
    fn click_at(&mut self, p: egui::Pos2) {
        self.events.push(egui::Event::PointerMoved(p));
        for pressed in [true, false] {
            self.events.push(egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            });
            self.frame();
        }
        self.frame();
    }

    fn click(&mut self, label: &str) {
        let r = self
            .find(Role::Button, label)
            .or_else(|| self.find(Role::Tab, label))
            .unwrap_or_else(|| panic!("no button '{label}' in {:?}", self.labels()));
        self.click_at(r.center());
    }

    /// Type into the `nth` text field (0-based, top-to-bottom then
    /// left-to-right on screen).
    fn type_into(&mut self, nth: usize, text: &str) {
        let mut fields: Vec<egui::Rect> = self
            .tree
            .iter()
            .filter(|(_, n)| n.role() == Role::TextInput || n.role() == Role::MultilineTextInput)
            .map(|(_, n)| {
                let b = n.bounds().unwrap();
                egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32))
            })
            .collect();
        fields.sort_by(|a, b| (a.top() as i32 / 8, a.left() as i32).cmp(&(b.top() as i32 / 8, b.left() as i32)));
        let r = fields[nth];
        self.click_at(r.center());
        // Select all + replace.
        self.events.push(egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        });
        self.events.push(egui::Event::Text(text.into()));
        self.frame();
        self.events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        });
        self.frame();
    }

    fn labels(&self) -> Vec<String> {
        self.tree
            .iter()
            .map(|(_, n)| format!("{:?}:{:?}/{:?}", n.role(), n.label(), n.value()))
            .collect()
    }

    fn text_shown(&self, needle: &str) -> bool {
        self.tree.iter().any(|(_, n)| n.label().or_else(|| n.value()).is_some_and(|l| l.contains(needle)))
    }
}

/// One private config dir for the whole test binary (set before any test
/// touches it; both tests use different game codes' files).
fn config_dir() -> std::path::PathBuf {
    static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("crabboy-m11-ui-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &d);
        d
    })
    .clone()
}

/// A "game" with an HP counter in a block that moves between sessions (via
/// a pointer in IWRAM), plus decoy words that equal HP by coincidence.
fn set_game(gba: &mut Gba, session: u32, hp: u16) {
    let block = 0x0200_4000 + session * 0x40;
    gba.mmu.write32(0x0300_0100, block);
    gba.mmu.write16(block + 0x20, hp);
    gba.mmu.write16(0x0200_0800, hp); // decoy: tracks HP in session 0 only
    gba.mmu.write16(0x0200_0900 + session * 2, hp); // decoy: moves differently
}

fn new_gba() -> Gba {
    let mut gba = Gba::new_headless();
    let mut rom = vec![0u8; 0x400];
    rom[0xAC..0xB0].copy_from_slice(b"TEST");
    gba.load_rom_bytes(rom);
    gba
}

#[test]
fn player_teaches_hp_saves_and_the_map_reloads() {
    let dir = config_dir();

    let mut h = Harness::new(new_gba());
    let code = h.gba.mmu.cartridge.as_ref().unwrap().game_code.clone();
    let title = h.gba.mmu.cartridge.as_ref().unwrap().title.clone();
    assert!(h.dialog.on_game_loaded(&code, &title).is_none(), "no map saved yet");
    h.frame();
    assert!(h.text_shown("None yet"));

    // Session 1: HP 29, then 25.
    for hp in [29u16, 25] {
        set_game(&mut h.gba, 0, hp);
        h.type_into(1, &hp.to_string());
        h.click("Record");
    }
    // Reset -> session 2 (block moved): HP 29, 20.
    h.dialog.on_reset();
    let mut g2 = new_gba();
    set_game(&mut g2, 1, 29);
    g2.mmu.write16(0x0200_0800, 7); // decoy no longer matches
    h.gba = g2;
    h.type_into(1, "29");
    h.click("Record");
    set_game(&mut h.gba, 1, 20);
    h.gba.mmu.write16(0x0200_0800, 7);
    h.type_into(1, "20");
    h.click("Record");
    assert!(h.text_shown("4 snapshot(s), 2 session(s)"), "{:?}", h.labels());

    h.click("Discover");
    assert!(h.text_shown("HP: [0x03000100]+0x20"), "{:?}", h.labels());
    assert!(h.text_shown("(unsaved)"), "{:?}", h.labels());
    let hp = h.dialog.map.get("HP").expect("HP found").clone();
    assert_eq!(hp.location, Location::Pointer { base: 0x0300_0100, offset: 0x20 });

    // Live value is shown in the table, and Watch adds it to the hex editor.
    assert!(h.text_shown("20"));
    let r = h.find(Role::Button, "👁 Watch").expect("watch button");
    h.click_at(r.center());
    assert_eq!(h.watch.last().map(|w| w.address), Some(0x0200_4060));

    h.click("Save map");
    assert!(h.toast.as_deref().is_some_and(|t| t.contains("saved")), "{:?}", h.toast);
    assert!(!h.text_shown("(unsaved)"));

    // Next time the game loads, the map comes back by itself.
    let mut h2 = Harness::new(new_gba());
    let msg = h2.dialog.on_game_loaded(&code, &title);
    assert!(msg.is_some_and(|m| m.contains("1 variable")));
    assert_eq!(h2.dialog.map.get("HP"), Some(&hp));
    let _ = MemoryMap::default();
    let _ = std::fs::remove_dir_all(dir.join("memmaps"));
}

#[test]
fn ask_me_narrows_an_ambiguous_number_by_poking() {
    let _dir = config_dir();
    let mut gba = new_gba();
    // Money 500 at 0x02001000, plus two constant copies that also always
    // read 500 (a real game's cached/backup copies). Only the first one is
    // what the "screen" shows.
    for a in [0x0200_1000u32, 0x0200_2000, 0x0200_3000] {
        gba.mmu.write16(a, 500);
    }
    let mut h = Harness::new(gba);
    h.dialog.on_game_loaded("ASKM", "");
    h.frame();
    h.type_into(0, "Money");
    h.type_into(1, "500");
    h.click("Record");
    h.click("Record");
    h.dialog.on_reset();
    h.click("Record");
    h.click("Discover");
    assert!(h.text_shown("Ask me"), "{:?}", h.labels());
    assert!(h.text_shown("Money: 3 candidate(s)"), "{:?}", h.labels());

    // "It shows now: 500" -> Start. Each question: read the screen (here,
    // the real variable) and answer.
    h.type_into(2, "500");
    h.click("Start");
    let mut questions = 0;
    while h.text_shown("what does it show now") {
        let shown = h.gba.mmu.read16(0x0200_1000);
        let field = h
            .tree
            .iter()
            .filter(|(_, n)| n.role() == Role::TextInput)
            .count()
            - 1;
        h.type_into(field, &shown.to_string());
        h.click("Answer");
        questions += 1;
        assert!(questions < 5);
    }
    assert_eq!(h.dialog.map.get("Money").map(|v| v.location), Some(Location::Absolute { addr: 0x0200_1000 }));
    // Every experiment was undone.
    for a in [0x0200_1000u32, 0x0200_2000, 0x0200_3000] {
        assert_eq!(h.gba.mmu.read16(a), 500, "0x{a:08X} left poked");
    }
}
