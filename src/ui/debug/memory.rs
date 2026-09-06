//! Interactive Memory Hex Editor, Delta Highlighting & Live Watchpoints

use crate::gba::mmu::Mmu;
use egui::{Color32, RichText, Ui};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchType {
    U8,
    U16,
    U32,
    Hex,
}

#[derive(Clone)]
pub struct WatchEntry {
    pub name: String,
    pub address: u32,
    pub watch_type: WatchType,
}

pub struct MemoryViewerState {
    pub base_addr: u32,
    pub prev_bytes: [u8; 256],
    pub has_prev: bool,

    // In-place byte editing
    pub edit_addr_str: String,
    pub edit_val_str: String,

    // Watch List
    pub watch_list: Vec<WatchEntry>,
    pub new_watch_name: String,
    pub new_watch_addr_str: String,
    pub new_watch_type: WatchType,
}

impl Default for MemoryViewerState {
    fn default() -> Self {
        Self {
            base_addr: 0x0200_0000, // EWRAM
            prev_bytes: [0; 256],
            has_prev: false,
            edit_addr_str: "0x02000000".to_string(),
            edit_val_str: "00".to_string(),
            watch_list: vec![
                WatchEntry { name: "Sample Memory Watch (0x0202402C)".to_string(), address: 0x0202_402C, watch_type: WatchType::U32 },
            ],
            new_watch_name: String::new(),
            new_watch_addr_str: String::new(),
            new_watch_type: WatchType::U16,
        }
    }
}

pub fn show_memory_viewer(ui: &mut Ui, mmu: &mut Mmu, state: &mut MemoryViewerState) {
    ui.heading("Interactive Memory Hex Editor & Watchpoints");
    ui.separator();

    // Quick region jumps
    ui.horizontal(|ui| {
        if ui.button("EWRAM (0x02000000)").clicked() { state.base_addr = 0x0200_0000; }
        if ui.button("IWRAM (0x03000000)").clicked() { state.base_addr = 0x0300_0000; }
        if ui.button("I/O (0x04000000)").clicked() { state.base_addr = 0x0400_0000; }
        if ui.button("VRAM (0x06000000)").clicked() { state.base_addr = 0x0600_0000; }
        if ui.button("ROM (0x08000000)").clicked() { state.base_addr = 0x0800_0000; }
    });

    ui.horizontal(|ui| {
        if ui.button("<< -256B").clicked() { state.base_addr = state.base_addr.saturating_sub(256); }
        if ui.button(">> +256B").clicked() { state.base_addr = state.base_addr.saturating_add(256); }
        ui.label(RichText::new(format!("Base: 0x{:08X}", state.base_addr)).monospace().strong());
    });

    ui.separator();

    // In-place byte editor controls
    ui.collapsing("✏ In-Place Byte Editor", |ui| {
        ui.horizontal(|ui| {
            ui.label("Target Address:");
            ui.text_edit_singleline(&mut state.edit_addr_str);
            ui.label("New Hex Byte:");
            ui.text_edit_singleline(&mut state.edit_val_str);

            if ui.button("⚡ Write Byte to RAM").clicked() {
                let addr_clean = state.edit_addr_str.trim().trim_start_matches("0x");
                let val_clean = state.edit_val_str.trim().trim_start_matches("0x");
                if let (Ok(addr), Ok(val)) = (u32::from_str_radix(addr_clean, 16), u8::from_str_radix(val_clean, 16)) {
                    mmu.write8(addr, val);
                }
            }
        });
    });

    ui.separator();

    // Hex Grid with Delta Highlighting
    ui.label(RichText::new("Memory Grid (Changed bytes highlighted in bright green):").strong());
    let mut current_bytes = [0u8; 256];

    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
        egui::Grid::new("mem_hex_grid").striped(true).show(ui, |ui| {
            for row in 0..16 {
                let row_addr = state.base_addr.wrapping_add(row * 16);
                ui.label(RichText::new(format!("0x{:08X}:", row_addr)).monospace().color(Color32::LIGHT_BLUE));

                let mut ascii_str = String::with_capacity(16);

                ui.horizontal(|ui| {
                    for b in 0..16 {
                        let byte_idx = (row * 16 + b) as usize;
                        let byte = mmu.read8(row_addr.wrapping_add(b));
                        current_bytes[byte_idx] = byte;

                        let is_changed = state.has_prev && (state.prev_bytes[byte_idx] != byte);
                        let byte_color = if is_changed {
                            Color32::from_rgb(50, 255, 50)
                        } else {
                            Color32::LIGHT_GRAY
                        };

                        let text = RichText::new(format!("{:02X} ", byte)).monospace().color(byte_color);
                        if is_changed {
                            ui.label(text.strong());
                        } else {
                            ui.label(text);
                        }

                        let ch = if (32..127).contains(&byte) { byte as char } else { '.' };
                        ascii_str.push(ch);
                    }
                });

                ui.label(RichText::new(ascii_str).monospace().color(Color32::from_rgb(180, 220, 180)));
                ui.end_row();
            }
        });
    });

    state.prev_bytes = current_bytes;
    state.has_prev = true;

    ui.separator();

    // Live Watch List
    ui.collapsing(format!("👁 Live Memory Watch List ({} watched)", state.watch_list.len()), |ui| {
        egui::Grid::new("watch_list_grid").striped(true).show(ui, |ui| {
            ui.label(RichText::new("Name").strong());
            ui.label(RichText::new("Address").strong());
            ui.label(RichText::new("Type").strong());
            ui.label(RichText::new("Live Value").strong());
            ui.label(RichText::new("Action").strong());
            ui.end_row();

            let mut remove_idx = None;
            for (idx, w) in state.watch_list.iter().enumerate() {
                ui.label(&w.name);
                ui.label(RichText::new(format!("0x{:08X}", w.address)).monospace().color(Color32::LIGHT_BLUE));
                let type_str = match w.watch_type {
                    WatchType::U8 => "u8",
                    WatchType::U16 => "u16",
                    WatchType::U32 => "u32",
                    WatchType::Hex => "hex",
                };
                ui.label(type_str);

                let val_str = match w.watch_type {
                    WatchType::U8 => format!("{}", mmu.read8(w.address)),
                    WatchType::U16 => format!("{}", mmu.read16(w.address & !1)),
                    WatchType::U32 => format!("{}", mmu.read32(w.address & !3)),
                    WatchType::Hex => format!("0x{:08X}", mmu.read32(w.address & !3)),
                };
                ui.label(RichText::new(val_str).monospace().strong().color(Color32::LIGHT_GREEN));

                if ui.button("🗑").clicked() {
                    remove_idx = Some(idx);
                }
                ui.end_row();
            }

            if let Some(i) = remove_idx {
                state.watch_list.remove(i);
            }
        });

        ui.horizontal(|ui| {
            ui.label("Name:");
            ui.text_edit_singleline(&mut state.new_watch_name);
            ui.label("Addr:");
            ui.text_edit_singleline(&mut state.new_watch_addr_str);

            if ui.button("➕ Add Watchpoint").clicked() {
                let clean = state.new_watch_addr_str.trim().trim_start_matches("0x");
                if let Ok(addr) = u32::from_str_radix(clean, 16) {
                    let name = if state.new_watch_name.is_empty() {
                        format!("Var 0x{:08X}", addr)
                    } else {
                        state.new_watch_name.clone()
                    };
                    state.watch_list.push(WatchEntry {
                        name,
                        address: addr,
                        watch_type: state.new_watch_type,
                    });
                    state.new_watch_name.clear();
                    state.new_watch_addr_str.clear();
                }
            }
        });
    });
}
