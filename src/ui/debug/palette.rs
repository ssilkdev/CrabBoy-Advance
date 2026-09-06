//! GBA Palette RAM Visualizer (BG & Sprite Palettes)

use crate::gba::ppu::blend::bgr555_to_rgb888;
use egui::{Color32, Ui, Vec2};

pub fn show_palette_viewer(ui: &mut Ui, palette_ram: &[u8]) {
    ui.heading("Palette RAM (512 Colors)");
    ui.separator();

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(egui::RichText::new("Background Palettes (16x16)").strong());
            render_palette_grid(ui, palette_ram, 0);
        });

        ui.add_space(20.0);

        ui.vertical(|ui| {
            ui.label(egui::RichText::new("Sprite (OBJ) Palettes (16x16)").strong());
            render_palette_grid(ui, palette_ram, 0x200);
        });
    });
}

fn render_palette_grid(ui: &mut Ui, palette_ram: &[u8], base_offset: usize) {
    let swatch_size = 14.0;
    egui::Grid::new(format!("pal_grid_{}", base_offset))
        .spacing([2.0, 2.0])
        .show(ui, |ui| {
            for pal_idx in 0..16 {
                ui.label(egui::RichText::new(format!("{:X}:", pal_idx)).monospace().small());
                for col_idx in 0..16 {
                    let addr = base_offset + (pal_idx * 16 + col_idx) * 2;
                    let (r, g, b, raw_color) = if addr + 1 < palette_ram.len() {
                        let c = (palette_ram[addr] as u16) | ((palette_ram[addr + 1] as u16) << 8);
                        let (r, g, b) = bgr555_to_rgb888(c);
                        (r, g, b, c)
                    } else {
                        (0, 0, 0, 0)
                    };

                    let (rect, response) = ui.allocate_exact_size(Vec2::splat(swatch_size), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 2.0, Color32::from_rgb(r, g, b));

                    response.on_hover_ui(|ui| {
                        ui.label(format!("Pal {}, Color {}", pal_idx, col_idx));
                        ui.label(format!("BGR555: 0x{:04X}", raw_color));
                        ui.label(format!("RGB: ({}, {}, {})", r, g, b));
                    });
                }
                ui.end_row();
            }
        });
}
