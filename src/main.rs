//! Game Boy Advance Simulator - Application Entry Point

pub mod gba;
pub mod ui;

use std::env;
use std::path::PathBuf;
use ui::GbaApp;

// Force NVIDIA Optimus and AMD PowerXpress drivers on Windows to run on the high-performance dedicated GPU
#[no_mangle]
#[used]
pub static NvOptimusEnablement: u32 = 1;

#[no_mangle]
#[used]
pub static AmdPowerXpressRequestHighPerformance: i32 = 1;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    log::info!("Starting CrabBoy Advance...");

    // Find ROM to load
    let args: Vec<String> = env::args().collect();
    let initial_rom: Option<PathBuf> = if args.len() > 1 {
        let p = PathBuf::from(&args[1]);
        if p.exists() {
            Some(p)
        } else {
            None
        }
    } else {
        None
    };

    let rom_title = if let Some(ref path) = initial_rom {
        format!(
            "CrabBoy Advance - {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        )
    } else {
        "CrabBoy Advance".to_string()
    };

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([760.0, 540.0])
            .with_min_inner_size([480.0, 360.0])
            .with_title(rom_title)
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "GBA Simulator",
        options,
        Box::new(|cc| Ok(Box::new(GbaApp::new(cc, initial_rom)))),
    )
}
