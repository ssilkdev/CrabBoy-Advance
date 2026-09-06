//! PPU Layer Inspector & OAM Sprite Visualizer

use crate::gba::Gba;
use egui::{Color32, RichText, Ui};

pub fn show_ppu_inspector(ui: &mut Ui, gba: &mut Gba) {
    ui.heading("PPU Layer Inspector & OAM Gallery");
    ui.label(RichText::new("Toggle hardware background layers, inspect sprites, and view DISPCNT status.").weak().small());
    ui.separator();

    // 1. Layer Visibility Toggles
    ui.label(RichText::new("Hardware Layer Toggles:").strong());
    let mut mask = gba.mmu.ppu.layer_mask;

    ui.horizontal(|ui| {
        let mut bg0 = (mask & (1 << 0)) != 0;
        let mut bg1 = (mask & (1 << 1)) != 0;
        let mut bg2 = (mask & (1 << 2)) != 0;
        let mut bg3 = (mask & (1 << 3)) != 0;
        let mut obj = (mask & (1 << 4)) != 0;

        if ui.checkbox(&mut bg0, "BG 0").changed() { mask ^= 1 << 0; }
        if ui.checkbox(&mut bg1, "BG 1").changed() { mask ^= 1 << 1; }
        if ui.checkbox(&mut bg2, "BG 2").changed() { mask ^= 1 << 2; }
        if ui.checkbox(&mut bg3, "BG 3").changed() { mask ^= 1 << 3; }
        if ui.checkbox(&mut obj, "OBJ (Sprites)").changed() { mask ^= 1 << 4; }
    });
    gba.mmu.ppu.layer_mask = mask;

    ui.horizontal(|ui| {
        if ui.button("Show All Layers").clicked() {
            gba.mmu.ppu.layer_mask = 0x1F;
        }
        if ui.button("Hide All Layers").clicked() {
            gba.mmu.ppu.layer_mask = 0x00;
        }
    });

    ui.separator();

    // 2. DISPCNT & Background Registers
    let dispcnt = gba.mmu.ppu.dispcnt;
    let mode = dispcnt & 7;
    let forced_blank = (dispcnt & (1 << 7)) != 0;
    ui.label(RichText::new(format!("DISPCNT: 0x{:04X} | Mode: {} | Forced Blank: {}", dispcnt, mode, forced_blank)).strong());

    ui.collapsing("Background Control (BGCNT 0-3)", |ui| {
        egui::Grid::new("bgcnt_grid").striped(true).show(ui, |ui| {
            ui.label(RichText::new("Layer").strong());
            ui.label(RichText::new("Priority").strong());
            ui.label(RichText::new("Char Block").strong());
            ui.label(RichText::new("Screen Block").strong());
            ui.label(RichText::new("Scroll (X, Y)").strong());
            ui.end_row();

            for i in 0..4 {
                let cnt = gba.mmu.ppu.bgcnt[i];
                let prio = cnt & 3;
                let char_base = (cnt >> 2) & 3;
                let screen_base = (cnt >> 8) & 0x1F;
                let scroll_x = gba.mmu.ppu.bghofs[i];
                let scroll_y = gba.mmu.ppu.bgvofs[i];

                ui.label(format!("BG {}", i));
                ui.label(format!("{}", prio));
                ui.label(format!("{} (0x{:05X})", char_base, char_base as usize * 0x4000));
                ui.label(format!("{} (0x{:05X})", screen_base, screen_base as usize * 0x800));
                ui.label(format!("{}, {}", scroll_x, scroll_y));
                ui.end_row();
            }
        });
    });

    ui.separator();

    // 3. OAM Sprite Gallery
    ui.heading("Active OAM Hardware Sprites (128 total)");
    egui::ScrollArea::vertical().max_height(250.0).show(ui, |ui| {
        egui::Grid::new("oam_sprites_grid").striped(true).show(ui, |ui| {
            ui.label(RichText::new("#").strong());
            ui.label(RichText::new("Pos (X, Y)").strong());
            ui.label(RichText::new("Size (WxH)").strong());
            ui.label(RichText::new("Tile").strong());
            ui.label(RichText::new("Palette").strong());
            ui.label(RichText::new("Prio").strong());
            ui.label(RichText::new("Flips").strong());
            ui.end_row();

            let oam = &gba.mmu.ppu.oam;
            for i in 0..128 {
                let off = i * 8;
                let attr0 = (oam[off] as u16) | ((oam[off + 1] as u16) << 8);
                let attr1 = (oam[off + 2] as u16) | ((oam[off + 3] as u16) << 8);
                let attr2 = (oam[off + 4] as u16) | ((oam[off + 5] as u16) << 8);

                let is_affine = (attr0 & (1 << 8)) != 0;
                let is_disabled = !is_affine && ((attr0 & (1 << 9)) != 0);

                if is_disabled {
                    continue; // Skip disabled sprites
                }

                let y = (attr0 & 0xFF) as usize;
                let x = (attr1 & 0x1FF) as usize;
                let shape = (attr0 >> 14) & 3;
                let size_code = (attr1 >> 14) & 3;
                let tile = attr2 & 0x3FF;
                let prio = (attr2 >> 10) & 3;
                let pal = (attr2 >> 12) & 0xF;
                let flip_h = !is_affine && ((attr1 & (1 << 12)) != 0);
                let flip_v = !is_affine && ((attr1 & (1 << 13)) != 0);

                let (w, h) = match (shape, size_code) {
                    (0, 0) => (8, 8), (0, 1) => (16, 16), (0, 2) => (32, 32), (0, 3) => (64, 64),
                    (1, 0) => (16, 8), (1, 1) => (32, 8), (1, 2) => (32, 16), (1, 3) => (64, 32),
                    (2, 0) => (8, 16), (2, 1) => (8, 32), (2, 2) => (16, 32), (2, 3) => (32, 64),
                    _ => (8, 8),
                };

                let flip_str = match (flip_h, flip_v) {
                    (true, true) => "HV",
                    (true, false) => "H",
                    (false, true) => "V",
                    (false, false) => "-",
                };

                ui.label(RichText::new(format!("{:03}", i)).color(Color32::LIGHT_BLUE).monospace());
                ui.label(format!("{}, {}", x, y));
                ui.label(format!("{}x{}", w, h));
                ui.label(RichText::new(format!("0x{:03X}", tile)).monospace());
                ui.label(format!("{}", pal));
                ui.label(format!("{}", prio));
                ui.label(flip_str);
                ui.end_row();
            }
        });
    });
}
