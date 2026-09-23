//! Memory Map dialog (ROADMAP M11).
//!
//! Guided variable discovery for the loaded game:
//!
//! 1. **Record.** While playing, the player snapshots RAM and says what the
//!    screen shows: a number ("HP 27"), a step ("moved right"), a yes/no
//!    ("text box open") or which message is up. Each game reset starts a new
//!    *session*; sessions are what make results trustworthy (see
//!    `memmap::discover`).
//! 2. **Discover.** Runs the searches and learners and shows what was found.
//! 3. **Ask me.** For a number with several candidates, pokes half of them,
//!    asks what the game shows now, restores the state. Repeat until one is
//!    left.
//! 4. **Save.** The map goes to `<config dir>/memmaps/<GAME>.json` and is
//!    loaded automatically next time the game starts.

use std::path::PathBuf;

use crate::gba::memmap::discover::{discover_all, discover_number, Fact, Finding, Observation, PokeRefiner};
use crate::gba::memmap::{MemoryMap, RamSnapshot, VarKind};
use crate::gba::Gba;
use crate::ui::debug::memory::{WatchEntry, WatchType};
use egui::{Color32, RichText};

/// Where maps are stored.
pub fn memmap_dir() -> Option<PathBuf> {
    crate::ui::config::config_dir().map(|d| d.join("memmaps"))
}

#[derive(Clone, Copy, PartialEq)]
enum FactKind {
    Number,
    Step,
    Flag,
    Text,
}

struct Pending {
    finding: Finding,
    refiner: Option<PokeRefiner>,
    /// State saved before the running poke experiment.
    saved_state: Option<Vec<u8>>,
    shown_before: String,
    shown_after: String,
}

pub struct MemoryMapDialog {
    pub is_open: bool,
    pub map: MemoryMap,
    /// Whether `map` has unsaved changes.
    dirty: bool,
    observations: Vec<Observation>,
    session: u32,
    last_report: Vec<String>,
    // Fact entry
    kind: FactKind,
    var_name: String,
    number_text: String,
    message_no: u32,
    pending: Vec<Pending>,
}

impl Default for MemoryMapDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryMapDialog {
    pub fn new() -> Self {
        Self {
            is_open: false,
            map: MemoryMap::default(),
            dirty: false,
            observations: Vec::new(),
            session: 0,
            last_report: Vec::new(),
            kind: FactKind::Number,
            var_name: "HP".into(),
            number_text: String::new(),
            message_no: 1,
            pending: Vec::new(),
        }
    }

    /// A game was loaded: load its map (if saved) and start over recording.
    pub fn on_game_loaded(&mut self, game_code: &str, title: &str) -> Option<String> {
        self.observations.clear();
        self.pending.clear();
        self.last_report.clear();
        self.session = 0;
        self.dirty = false;
        let loaded = memmap_dir().and_then(|d| MemoryMap::load(&d, game_code, title));
        let msg = loaded.as_ref().map(|m| format!("🧠 Memory map loaded: {} variable(s)", m.variables.len()));
        self.map = loaded.unwrap_or_else(|| MemoryMap::new(game_code, title));
        msg
    }

    /// Facts recorded so far, one line per snapshot (for tests and the log).
    pub fn describe_observations(&self) -> Vec<String> {
        self.observations.iter().map(|o| format!("session {} {:?}", o.session, o.facts)).collect()
    }

    /// The game was reset: later snapshots belong to a new session.
    pub fn on_reset(&mut self) {
        if self.observations.iter().any(|o| o.session == self.session) {
            self.session += 1;
        }
    }

    fn record(&mut self, gba: &Gba, fact: Option<Fact>) {
        let facts = fact.into_iter().collect();
        self.observations.push(Observation { session: self.session, snap: RamSnapshot::capture(&gba.mmu), facts });
    }

    fn save(&mut self) -> Result<String, String> {
        let dir = memmap_dir().ok_or("no config directory")?;
        let path = self.map.save(&dir).map_err(|e| e.to_string())?;
        self.dirty = false;
        Ok(format!("Memory map saved to {}", path.display()))
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        gba: &mut Gba,
        watch_list: &mut Vec<WatchEntry>,
        toast: &mut Option<String>,
    ) {
        if !self.is_open {
            return;
        }
        let mut open = self.is_open;
        egui::Window::new("🧠 Memory Map")
            .open(&mut open)
            .default_width(560.0)
            .resizable(true)
            .show(ctx, |ui| {
                if gba.mmu.cartridge.is_none() {
                    ui.label("Load a GBA game first.");
                    return;
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.variables_ui(ui, gba, watch_list, toast);
                    ui.separator();
                    self.record_ui(ui, gba, toast);
                    ui.separator();
                    self.pending_ui(ui, gba, toast);
                });
            });
        self.is_open = open;
    }

    fn variables_ui(&mut self, ui: &mut egui::Ui, gba: &Gba, watch_list: &mut Vec<WatchEntry>, toast: &mut Option<String>) {
        ui.horizontal(|ui| {
            ui.heading(format!("Variables for {}", if self.map.game_code.is_empty() { &self.map.title } else { &self.map.game_code }));
            if self.dirty {
                ui.label(RichText::new("(unsaved)").color(Color32::YELLOW));
            }
        });
        if self.map.variables.is_empty() {
            ui.label(RichText::new("None yet. Record a few observations below, then press Discover.").weak());
        }
        let mut remove = None;
        egui::Grid::new("memmap_vars").striped(true).show(ui, |ui| {
            for v in &self.map.variables {
                ui.label(RichText::new(&v.name).strong());
                let value = v.read_live(&gba.mmu).map(|x| x.to_string()).unwrap_or_else(|| "—".into());
                ui.label(RichText::new(value).monospace().color(Color32::LIGHT_GREEN));
                ui.label(RichText::new(v.describe()).monospace().small());
                ui.label(RichText::new(format!("{} session(s)", v.sessions_confirmed)).small().weak());
                ui.horizontal(|ui| {
                    if ui.small_button("👁 Watch").on_hover_text("Add to the hex editor's watch list").clicked() {
                        if let Some(addr) = v.current_address(&gba.mmu) {
                            let watch_type = match v.width {
                                1 => WatchType::U8,
                                2 => WatchType::U16,
                                _ => WatchType::U32,
                            };
                            watch_list.push(WatchEntry { name: v.name.clone(), address: addr, watch_type });
                            *toast = Some(format!("Watching {} at 0x{addr:08X}", v.name));
                        }
                    }
                    if ui.small_button("🗑").clicked() {
                        remove = Some(v.name.clone());
                    }
                });
                ui.end_row();
            }
        });
        if let Some(n) = remove {
            self.map.remove(&n);
            self.dirty = true;
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(self.dirty, egui::Button::new("💾 Save map")).clicked() {
                *toast = Some(self.save().unwrap_or_else(|e| format!("Save failed: {e}")));
            }
            if let Some(dir) = memmap_dir() {
                ui.label(RichText::new(dir.display().to_string()).small().weak());
            }
        });
    }

    fn record_ui(&mut self, ui: &mut egui::Ui, gba: &mut Gba, toast: &mut Option<String>) {
        ui.heading("Teach it");
        ui.label(
            RichText::new(
                "Play normally. Whenever the screen shows something, record it. Reset the game (Ctrl+R) now and \
                 then: each reset is a new session, and results must hold in every session.",
            )
            .weak()
            .small(),
        );
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.kind, FactKind::Number, "Number");
            ui.selectable_value(&mut self.kind, FactKind::Step, "Step");
            ui.selectable_value(&mut self.kind, FactKind::Flag, "Yes/no");
            ui.selectable_value(&mut self.kind, FactKind::Text, "Message");
        });
        let mut fact = None;
        ui.horizontal_wrapped(|ui| match self.kind {
            FactKind::Number => {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut self.var_name).desired_width(90.0));
                for preset in ["HP", "Money"] {
                    if ui.small_button(preset).clicked() {
                        self.var_name = preset.into();
                    }
                }
                ui.label("shows");
                ui.add(egui::TextEdit::singleline(&mut self.number_text).desired_width(70.0));
                if ui.button("📸 Record").clicked() {
                    match self.number_text.trim().parse::<u32>() {
                        Ok(value) => fact = Some(Fact::Value { var: self.var_name.trim().to_string(), value }),
                        Err(_) => *toast = Some("Type the number the game shows".into()),
                    }
                }
            }
            FactKind::Step => {
                ui.label("Record after each step:");
                for (label, d) in [("⬅ Left", (-1, 0)), ("➡ Right", (1, 0)), ("⬆ Up", (0, -1)), ("⬇ Down", (0, 1)), ("Stood still", (0, 0))] {
                    if ui.button(label).clicked() {
                        fact = Some(Fact::Moved { dx: d.0, dy: d.1 });
                    }
                }
                if ui.button("📍 Starting spot").on_hover_text("Record before the first step").clicked() {
                    self.record(gba, None);
                }
            }
            FactKind::Flag => {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut self.var_name).desired_width(120.0));
                if ui.small_button("Text box open").clicked() {
                    self.var_name = "Text box open".into();
                }
                ui.label("is");
                if ui.button("✅ Yes").clicked() {
                    fact = Some(Fact::Flag { var: self.var_name.trim().to_string(), on: true });
                }
                if ui.button("❌ No").clicked() {
                    fact = Some(Fact::Flag { var: self.var_name.trim().to_string(), on: false });
                }
            }
            FactKind::Text => {
                ui.label("Message #");
                ui.add(egui::DragValue::new(&mut self.message_no).range(1..=999));
                if ui.button("📸 Same message").clicked() {
                    fact = Some(Fact::Text { var: "Text box text".into(), message: self.message_no });
                }
                if ui.button("📸 New message").clicked() {
                    let used = self.observations.iter().flat_map(|o| &o.facts).filter_map(|f| match f {
                        Fact::Text { message, .. } => Some(*message),
                        _ => None,
                    });
                    self.message_no = used.max().unwrap_or(0) + 1;
                    fact = Some(Fact::Text { var: "Text box text".into(), message: self.message_no });
                }
            }
        });
        if let Some(f) = fact {
            self.record(gba, Some(f));
        }

        let sessions = self.observations.iter().map(|o| o.session).collect::<std::collections::BTreeSet<_>>().len();
        ui.horizontal(|ui| {
            ui.label(format!("{} snapshot(s), {} session(s); current session #{}", self.observations.len(), sessions, self.session + 1));
            if ui.small_button("New session").on_hover_text("Use after resetting the game yourself").clicked() {
                self.on_reset();
            }
            if ui.small_button("Clear").clicked() {
                self.observations.clear();
                self.session = 0;
            }
        });
        ui.horizontal(|ui| {
            let can = !self.observations.is_empty();
            if ui.add_enabled(can, egui::Button::new(RichText::new("🔍 Discover").strong())).clicked() {
                let rom = gba.mmu.cartridge.as_ref().map(|c| c.rom.clone()).unwrap_or_default();
                let (found, report) = discover_all(&self.observations, &rom, &self.map.game_code, &self.map.title);
                self.pending.clear();
                for v in found.variables {
                    // Ambiguous numbers go to "Ask me"; everything else is in.
                    let finding = match &v.kind {
                        VarKind::Number => discover_number(&self.observations, &v.name).ok(),
                        _ => None,
                    };
                    match finding {
                        Some(f) if !f.is_unique() => self.pending.push(Pending {
                            finding: f,
                            refiner: None,
                            saved_state: None,
                            shown_before: String::new(),
                            shown_after: String::new(),
                        }),
                        _ => {
                            self.map.set(v);
                            self.dirty = true;
                        }
                    }
                }
                self.last_report = report;
            }
            if sessions < 2 {
                ui.label(RichText::new("Tip: two sessions or more give far fewer false hits.").small().weak());
            }
        });
        for line in &self.last_report {
            ui.label(RichText::new(line).monospace().small());
        }
    }

    fn pending_ui(&mut self, ui: &mut egui::Ui, gba: &mut Gba, toast: &mut Option<String>) {
        if self.pending.is_empty() {
            return;
        }
        ui.heading("Ask me");
        ui.label(
            RichText::new(
                "These fit every observation in more than one place. Each question writes a test value into half \
                 the candidates; open whatever screen shows the number, type what it says, and the game is \
                 restored afterwards.",
            )
            .weak()
            .small(),
        );
        let mut accept = None;
        for (i, p) in self.pending.iter_mut().enumerate() {
            let mut cancel = false;
            ui.group(|ui| {
                let left = p.refiner.as_ref().map_or(1 + p.finding.alternatives.len(), |r| r.remaining());
                ui.label(RichText::new(format!("{}: {left} candidate(s)", p.finding.variable.name)).strong());
                match &mut p.refiner {
                    None => {
                        ui.horizontal(|ui| {
                            ui.label("It shows now:");
                            ui.add(egui::TextEdit::singleline(&mut p.shown_before).desired_width(70.0));
                            if ui.button("Start").clicked() {
                                if let Ok(v) = p.shown_before.trim().parse::<u32>() {
                                    let mut r = PokeRefiner::new(&p.finding, &gba.mmu);
                                    p.saved_state = Some(gba.save_state());
                                    r.poke(&mut gba.mmu, v);
                                    p.refiner = Some(r);
                                } else {
                                    *toast = Some("Type the value the game shows right now".into());
                                }
                            }
                            if ui.button("Use best guess").clicked() {
                                accept = Some((i, p.finding.variable.clone()));
                            }
                        });
                    }
                    Some(r) if !r.done() => {
                        ui.horizontal(|ui| {
                            ui.label(format!("Question {}: what does it show now?", r.questions + 1));
                            ui.add(egui::TextEdit::singleline(&mut p.shown_after).desired_width(70.0));
                            if ui.button("Answer").clicked() {
                                if let Ok(v) = p.shown_after.trim().parse::<u32>() {
                                    r.answer(v);
                                    p.shown_after.clear();
                                    if let Some(s) = &p.saved_state {
                                        gba.load_state(s);
                                    }
                                    if !r.done() {
                                        let base = p.shown_before.trim().parse().unwrap_or(0);
                                        p.saved_state = Some(gba.save_state());
                                        r.poke(&mut gba.mmu, base);
                                    }
                                }
                            }
                            if ui.button("Cancel").clicked() {
                                if let Some(s) = p.saved_state.take() {
                                    gba.load_state(&s);
                                }
                                cancel = true;
                            }
                        });
                        ui.label(RichText::new(format!("(test value {} if you see a change)", r.test_value())).small().weak());
                    }
                    Some(r) => {
                        if let Some(v) = r.result() {
                            accept = Some((i, v.clone()));
                        } else {
                            ui.label("No candidate explained the answers. Record more observations.");
                        }
                    }
                }
            });
            if cancel {
                p.refiner = None;
            }
        }
        if let Some((i, v)) = accept {
            *toast = Some(format!("{} = {}", v.name, v.describe()));
            self.map.set(v);
            self.dirty = true;
            self.pending.remove(i);
        }
    }
}
