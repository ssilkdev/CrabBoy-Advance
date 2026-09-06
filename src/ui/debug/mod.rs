//! Debug Tools Integration

pub mod disasm;
pub mod memory;
pub mod palette;
pub mod ppu_inspector;

use crate::gba::Gba;
use disasm::show_cpu_inspector;
use memory::{show_memory_viewer, MemoryViewerState};
use palette::show_palette_viewer;
use ppu_inspector::show_ppu_inspector;

pub struct DebugWindows {
    pub show_cpu: bool,
    pub show_palette: bool,
    pub show_memory: bool,
    pub show_ppu: bool,
    pub mem_state: MemoryViewerState,
}

impl Default for DebugWindows {
    fn default() -> Self {
        Self {
            show_cpu: false,
            show_palette: false,
            show_memory: false,
            show_ppu: false,
            mem_state: MemoryViewerState::default(),
        }
    }
}

impl DebugWindows {
    pub fn show(&mut self, ctx: &egui::Context, gba: &mut Gba) {
        if self.show_cpu {
            egui::Window::new("CPU & Disassembly")
                .open(&mut self.show_cpu)
                .resizable(true)
                .default_width(450.0)
                .show(ctx, |ui| {
                    show_cpu_inspector(ui, &gba.cpu, &gba.mmu);
                });
        }

        if self.show_palette {
            egui::Window::new("Palette Viewer")
                .open(&mut self.show_palette)
                .resizable(false)
                .show(ctx, |ui| {
                    show_palette_viewer(ui, &gba.mmu.ppu.palette_ram[..]);
                });
        }

        if self.show_memory {
            let state = &mut self.mem_state;
            egui::Window::new("Memory Viewer & Watchpoints")
                .open(&mut self.show_memory)
                .resizable(true)
                .default_width(600.0)
                .show(ctx, |ui| {
                    show_memory_viewer(ui, &mut gba.mmu, state);
                });
        }

        if self.show_ppu {
            egui::Window::new("PPU Layers & OAM Inspector")
                .open(&mut self.show_ppu)
                .resizable(true)
                .default_width(550.0)
                .show(ctx, |ui| {
                    show_ppu_inspector(ui, gba);
                });
        }
    }
}
