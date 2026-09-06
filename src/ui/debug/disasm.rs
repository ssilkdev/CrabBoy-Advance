//! CPU Registers & Disassembly Debug View

use crate::gba::cpu::{Arm7Tdmi, FLAG_C, FLAG_N, FLAG_T, FLAG_V, FLAG_Z};
use crate::gba::mmu::Mmu;
use egui::{Color32, RichText, Ui};

pub fn show_cpu_inspector(ui: &mut Ui, cpu: &Arm7Tdmi, mmu: &Mmu) {
    ui.heading("CPU Registers (ARM7TDMI)");
    ui.separator();

    egui::Grid::new("cpu_regs_grid").striped(true).show(ui, |ui| {
        for row in 0..4 {
            for col in 0..4 {
                let r = row * 4 + col;
                let name = match r {
                    13 => "SP",
                    14 => "LR",
                    15 => "PC",
                    _ => "",
                };
                let label = if name.is_empty() {
                    format!("R{:02}:", r)
                } else {
                    format!("R{:02}({}):", r, name)
                };
                ui.label(RichText::new(label).monospace().strong());
                ui.label(RichText::new(format!("0x{:08X}", cpu.regs[r])).monospace().color(Color32::from_rgb(100, 200, 255)));
            }
            ui.end_row();
        }
    });

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("CPSR: 0x{:08X}", cpu.cpsr)).monospace().strong());
        ui.separator();
        ui.label(format!("Mode: {:?}", cpu.get_mode()));
        ui.separator();
        ui.label(if cpu.is_thumb() { "State: THUMB" } else { "State: ARM" });
    });

    ui.horizontal(|ui| {
        let flag_badge = |ui: &mut Ui, name: &str, set: bool| {
            let color = if set { Color32::GREEN } else { Color32::GRAY };
            ui.label(RichText::new(name).monospace().color(color).strong());
        };
        ui.label("Flags:");
        flag_badge(ui, "N", cpu.get_flag(FLAG_N));
        flag_badge(ui, "Z", cpu.get_flag(FLAG_Z));
        flag_badge(ui, "C", cpu.get_flag(FLAG_C));
        flag_badge(ui, "V", cpu.get_flag(FLAG_V));
        flag_badge(ui, "T", cpu.get_flag(FLAG_T));
    });

    ui.add_space(8.0);
    ui.heading("Disassembly (around PC)");
    ui.separator();

    let pc = cpu.regs[15];
    let is_thumb = cpu.is_thumb();
    let step = if is_thumb { 2 } else { 4 };

    let start_addr = pc.saturating_sub(step * 4);
    egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
        for i in 0..10 {
            let addr = start_addr.wrapping_add(i * step);
            let is_current = addr == pc;

            let text = if is_thumb {
                let code = mmu.read16(addr);
                format!("0x{:08X}: {:04X}", addr, code)
            } else {
                let code = mmu.read32(addr);
                format!("0x{:08X}: {:08X}", addr, code)
            };

            let row_text = if is_current {
                RichText::new(format!("=> {}", text)).monospace().color(Color32::YELLOW).strong()
            } else {
                RichText::new(format!("   {}", text)).monospace()
            };
            ui.label(row_text);
        }
    });
}
