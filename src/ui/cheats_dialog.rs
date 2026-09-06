//! In-Game Cheats Manager & RAM Searcher GUI

use crate::gba::cheats::{CompareType, SearchSize};
use crate::gba::Gba;
use egui::{Color32, RichText, Ui, Window};

#[derive(Default)]
pub struct CheatsDialog {
    pub is_open: bool,
    active_tab: usize, // 0 = Cheats List, 1 = RAM Searcher

    // Add cheat form inputs
    new_cheat_name: String,
    new_cheat_code: String,

    // RAM Search inputs
    search_value_text: String,
    search_size: SearchSizeUi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
enum SearchSizeUi {
    U8,
    #[default]
    U16,
    U32,
}


impl CheatsDialog {
    pub fn new() -> Self {
        Self {
            is_open: false,
            active_tab: 0,
            new_cheat_name: String::new(),
            new_cheat_code: String::new(),
            search_value_text: String::new(),
            search_size: SearchSizeUi::U16,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, gba: &mut Gba, toast: &mut Option<String>) {
        if !self.is_open {
            return;
        }

        let mut open = self.is_open;
        Window::new("Cheats & RAM Searcher")
            .open(&mut open)
            .default_width(620.0)
            .default_height(480.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.selectable_label(self.active_tab == 0, "📜 Active Cheats").clicked() {
                        self.active_tab = 0;
                    }
                    if ui.selectable_label(self.active_tab == 1, "🔍 RAM Value Searcher").clicked() {
                        self.active_tab = 1;
                    }
                });
                ui.separator();

                match self.active_tab {
                    0 => self.render_cheats_tab(ui, gba, toast),
                    1 => self.render_ram_search_tab(ui, gba, toast),
                    _ => {}
                }
            });
        self.is_open = open;
    }

    fn render_cheats_tab(&mut self, ui: &mut Ui, gba: &mut Gba, toast: &mut Option<String>) {
        ui.heading("Active Cheats (GameShark / Action Replay / CodeBreaker)");
        ui.label(RichText::new("Cheats are injected automatically during VBlank every frame.").weak().small());
        ui.separator();

        // Cheats list scroll area
        egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
            if gba.cheats.cheats.is_empty() {
                ui.label(RichText::new("No cheats added yet. Add a code below!").italics());
            } else {
                let mut remove_idx = None;
                for (idx, cheat) in gba.cheats.cheats.iter_mut().enumerate() {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut cheat.enabled, "");
                            ui.label(RichText::new(&cheat.name).strong());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button(RichText::new("🗑 Delete").color(Color32::from_rgb(240, 100, 100))).clicked() {
                                    remove_idx = Some(idx);
                                }
                            });
                        });
                        ui.label(RichText::new(&cheat.code).monospace().weak().small());
                        ui.label(RichText::new(format!("{} memory write ops", cheat.ops.len())).weak().small());
                    });
                }
                if let Some(i) = remove_idx {
                    gba.cheats.remove_cheat(i);
                    *toast = Some("Cheat removed".to_string());
                }
            }
        });

        ui.separator();
        ui.heading("Add New Cheat Code");
        ui.horizontal(|ui| {
            ui.label("Name:");
            ui.text_edit_singleline(&mut self.new_cheat_name);
        });
        ui.label("Code (Supports 'XXXXXXXX YYYYYYYY', CodeBreaker '8XXXXXXX YYYY', or '02XXXXXX:YY'):");
        ui.text_edit_multiline(&mut self.new_cheat_code);

        ui.horizontal(|ui| {
            if ui.button("➕ Add Cheat").clicked()
                && !self.new_cheat_name.is_empty() && !self.new_cheat_code.is_empty() {
                    gba.cheats.add_cheat(&self.new_cheat_name, &self.new_cheat_code);
                    *toast = Some(format!("Added cheat: {}", self.new_cheat_name));
                    self.new_cheat_name.clear();
                    self.new_cheat_code.clear();
                }

            if ui.button("⚡ Max Money Preset (Gen 3 RPG)").clicked() {
                gba.cheats.add_cheat("Max Money Preset", "0202402C:000F423F");
                *toast = Some("Added Max Money Cheat".to_string());
            }

            if ui.button("💾 Export to .cht").clicked() {
                let path = std::path::Path::new("saves/cheats.cht");
                if gba.cheats.save_to_file(path).is_ok() {
                    *toast = Some("Exported cheats to saves/cheats.cht".to_string());
                }
            }
        });
    }

    fn render_ram_search_tab(&mut self, ui: &mut Ui, gba: &mut Gba, toast: &mut Option<String>) {
        ui.heading("Real-Time RAM Value Searcher");
        ui.label(RichText::new("Scan EWRAM (0x02000000) and IWRAM (0x03000000) for changing game variables.").weak().small());
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Data Size:");
            ui.radio_value(&mut self.search_size, SearchSizeUi::U8, "8-bit (Byte)");
            ui.radio_value(&mut self.search_size, SearchSizeUi::U16, "16-bit (Word)");
            ui.radio_value(&mut self.search_size, SearchSizeUi::U32, "32-bit (DWord)");
        });

        gba.cheats.ram_searcher.search_size = match self.search_size {
            SearchSizeUi::U8 => SearchSize::U8,
            SearchSizeUi::U16 => SearchSize::U16,
            SearchSizeUi::U32 => SearchSize::U32,
        };

        ui.horizontal(|ui| {
            ui.label("Target Value (Dec or Hex):");
            ui.text_edit_singleline(&mut self.search_value_text);

            let parsed_target = if self.search_value_text.starts_with("0x") {
                u32::from_str_radix(self.search_value_text.trim_start_matches("0x"), 16).ok()
            } else {
                self.search_value_text.parse::<u32>().ok()
            };

            if ui.button("🔎 First Exact Scan").clicked() {
                if let Some(target) = parsed_target {
                    gba.cheats.ram_searcher.initial_search(&mut gba.mmu, Some(target));
                    *toast = Some(format!("Found {} candidates", gba.cheats.ram_searcher.candidates.len()));
                }
            }

            if ui.button("🌐 Unknown Initial Value").clicked() {
                gba.cheats.ram_searcher.initial_search(&mut gba.mmu, None);
                *toast = Some(format!("Indexed {} candidates", gba.cheats.ram_searcher.candidates.len()));
            }
        });

        ui.horizontal(|ui| {
            let parsed_target = if self.search_value_text.starts_with("0x") {
                u32::from_str_radix(self.search_value_text.trim_start_matches("0x"), 16).ok()
            } else {
                self.search_value_text.parse::<u32>().ok()
            };

            if ui.button("= Equal Target").clicked() {
                if let Some(t) = parsed_target {
                    gba.cheats.ram_searcher.filter_search(&mut gba.mmu, CompareType::Exact(t));
                }
            }
            if ui.button("== Unchanged").clicked() {
                gba.cheats.ram_searcher.filter_search(&mut gba.mmu, CompareType::Unchanged);
            }
            if ui.button("!= Changed").clicked() {
                gba.cheats.ram_searcher.filter_search(&mut gba.mmu, CompareType::Changed);
            }
            if ui.button("> Greater").clicked() {
                gba.cheats.ram_searcher.filter_search(&mut gba.mmu, CompareType::GreaterThanPrevious);
            }
            if ui.button("< Less").clicked() {
                gba.cheats.ram_searcher.filter_search(&mut gba.mmu, CompareType::LessThanPrevious);
            }
            if ui.button("🔄 Reset").clicked() {
                gba.cheats.ram_searcher.candidates.clear();
                gba.cheats.ram_searcher.has_searched = false;
                *toast = Some("Search reset".to_string());
            }
        });

        let total_cands = gba.cheats.ram_searcher.candidates.len();
        ui.label(RichText::new(format!("Candidates Found: {}", total_cands)).strong().color(Color32::LIGHT_GREEN));

        // Candidates list
        egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
            egui::Grid::new("ram_search_grid").striped(true).show(ui, |ui| {
                ui.label(RichText::new("Address").strong());
                ui.label(RichText::new("Prev Val").strong());
                ui.label(RichText::new("Current Val").strong());
                ui.label(RichText::new("Action").strong());
                ui.end_row();

                let mut freeze_addr = None;
                for cand in gba.cheats.ram_searcher.candidates.iter().take(50) {
                    ui.label(RichText::new(format!("0x{:08X}", cand.address)).monospace().color(Color32::LIGHT_BLUE));
                    ui.label(RichText::new(format!("{}", cand.previous_value)).monospace());
                    ui.label(RichText::new(format!("{} (0x{:X})", cand.current_value, cand.current_value)).monospace().strong());

                    if ui.button("❄ Freeze as Cheat").clicked() {
                        freeze_addr = Some((cand.address, cand.current_value));
                    }
                    ui.end_row();
                }

                if let Some((addr, val)) = freeze_addr {
                    let code = format!("0x{:08X}:{:X}", addr, val);
                    gba.cheats.add_cheat(format!("Freeze 0x{:08X}", addr), &code);
                    *toast = Some(format!("Created freeze cheat for 0x{:08X}", addr));
                }
            });
        });
    }
}
