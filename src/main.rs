//! Game Boy Advance Simulator - Application Entry Point
//! Supports interactive GUI and headless AI Agent diagnostic execution.

pub mod gba;
pub mod ui;

use gba::diagnostics::{HealthGrade, OverallHealthGrade};
use gba::Gba;
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

fn print_help() {
    println!(
        r#"🦀 CrabBoy Advance - Next-Generation Game Boy Advance Emulator

USAGE:
    crabboy-advance [ROM_PATH]                                  Launch GUI emulator
    crabboy-advance --diagnose <ROM> [--frames N] [--output P]  Run headless diagnostics and export report
    crabboy-advance --dump-frame <ROM> [--frame N] [--output P] Run headless to frame N and save PNG screenshot
    crabboy-advance --audit-audio <ROM> [--frames N]            Run headless audio health audit
    crabboy-advance --help                                      Show this help message

OPTIONS:
    --frames <N>   Number of frames to emulate (default: 300)
    --frame <N>    Target frame to capture (default: 60)
    --output <P>   Output file path for JSON report or PNG frame
"#
    );
}

fn get_arg_val(args: &[String], flag: &str) -> Option<String> {
    if let Some(pos) = args.iter().position(|a| a == flag) {
        if pos + 1 < args.len() {
            return Some(args[pos + 1].clone());
        }
    }
    None
}

fn handle_headless_cli(args: &[String]) {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        std::process::exit(0);
    }

    // 1. Headless Diagnostic Mode
    if let Some(pos) = args.iter().position(|a| a == "--diagnose") {
        if pos + 1 >= args.len() {
            eprintln!("Error: --diagnose requires a ROM path.");
            std::process::exit(1);
        }
        let rom_path = PathBuf::from(&args[pos + 1]);
        let frames: u64 = get_arg_val(args, "--frames")
            .and_then(|s| s.parse().ok())
            .unwrap_or(300);
        let output_path = get_arg_val(args, "--output").map(PathBuf::from);

        let mut gba = Gba::new();
        if let Err(e) = gba.load_rom(&rom_path) {
            eprintln!("Failed to load ROM '{}': {}", rom_path.display(), e);
            std::process::exit(1);
        }

        println!(
            "Running diagnostics on '{}' for {} frames (~{:.1}s)...",
            rom_path.display(),
            frames,
            frames as f32 / 60.0
        );
        let report = gba.run_diagnostics(frames);
        println!("{}", report.format_cli_summary());

        if let Some(ref out_path) = output_path {
            if let Err(e) = report.save_json(out_path) {
                eprintln!(
                    "Failed to save JSON report to '{}': {}",
                    out_path.display(),
                    e
                );
                std::process::exit(1);
            } else {
                println!("Report successfully saved to '{}'.", out_path.display());
            }
        }

        if report.overall_grade == OverallHealthGrade::Critical {
            std::process::exit(1);
        } else {
            std::process::exit(0);
        }
    }

    // 2. Headless Frame Dump Mode
    if let Some(pos) = args.iter().position(|a| a == "--dump-frame") {
        if pos + 1 >= args.len() {
            eprintln!("Error: --dump-frame requires a ROM path.");
            std::process::exit(1);
        }
        let rom_path = PathBuf::from(&args[pos + 1]);
        let target_frame: u64 = get_arg_val(args, "--frame")
            .or_else(|| get_arg_val(args, "--frames"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(60);
        let output_path = get_arg_val(args, "--output")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("frame_{}.png", target_frame)));

        let mut gba = Gba::new();
        if let Err(e) = gba.load_rom(&rom_path) {
            eprintln!("Failed to load ROM '{}': {}", rom_path.display(), e);
            std::process::exit(1);
        }

        println!(
            "Stepping to frame {} on '{}'...",
            target_frame,
            rom_path.display()
        );
        for _ in 0..target_frame {
            gba.run_frame();
        }

        if let Err(e) = gba.dump_frame_png(&output_path) {
            eprintln!(
                "Failed to save frame PNG to '{}': {}",
                output_path.display(),
                e
            );
            std::process::exit(1);
        }
        println!(
            "Saved frame {} screenshot to '{}'.",
            target_frame,
            output_path.display()
        );
        std::process::exit(0);
    }

    // 3. Headless Audio Quality Audit
    if let Some(pos) = args.iter().position(|a| a == "--audit-audio") {
        if pos + 1 >= args.len() {
            eprintln!("Error: --audit-audio requires a ROM path.");
            std::process::exit(1);
        }
        let rom_path = PathBuf::from(&args[pos + 1]);
        let frames: u64 = get_arg_val(args, "--frames")
            .and_then(|s| s.parse().ok())
            .unwrap_or(300);

        let mut gba = Gba::new();
        if let Err(e) = gba.load_rom(&rom_path) {
            eprintln!("Failed to load ROM '{}': {}", rom_path.display(), e);
            std::process::exit(1);
        }

        println!(
            "Auditing audio on '{}' for {} frames...",
            rom_path.display(),
            frames
        );
        let report = gba.run_diagnostics(frames);
        println!("Audio Grade:       {:?}", report.audio.grade);
        println!("Peak Amplitude:    {:.3}", report.audio.peak_amplitude);
        println!("DC Bias Offset:    {:.4}", report.audio.dc_bias);
        println!(
            "Clipping Samples:  {} ({:.2}%)",
            report.audio.clipping_samples,
            report.audio.clipping_rate * 100.0
        );
        println!("Silence Ratio:     {:.1}%", report.audio.silence_ratio * 100.0);
        println!(
            "Channel Triggers:  DS-A: {}, DS-B: {}, SQ1: {}, SQ2: {}, Wave: {}, Noise: {}",
            report.audio.channel_triggers[0],
            report.audio.channel_triggers[1],
            report.audio.channel_triggers[2],
            report.audio.channel_triggers[3],
            report.audio.channel_triggers[4],
            report.audio.channel_triggers[5],
        );
        if !report.audio.stuck_notes_detected.is_empty() {
            println!(
                "Stuck Notes:       {}",
                report.audio.stuck_notes_detected.join(", ")
            );
        }
        if !report.audio.anomalies.is_empty() {
            println!("Audio Anomalies:   {}", report.audio.anomalies.join("; "));
        }

        if report.audio.grade == HealthGrade::Critical {
            std::process::exit(1);
        } else {
            std::process::exit(0);
        }
    }
}

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = env::args().collect();
    handle_headless_cli(&args);

    log::info!("Starting CrabBoy Advance...");

    // Find ROM to load
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

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([760.0, 540.0])
        .with_min_inner_size([480.0, 360.0])
        .with_title(rom_title)
        .with_drag_and_drop(true);

    if let Ok(img) = image::load_from_memory(include_bytes!("../assets/icon_256.png")) {
        let rgba = img.to_rgba8();
        let (width, height) = rgba.dimensions();
        viewport = viewport.with_icon(eframe::egui::IconData {
            rgba: rgba.into_raw(),
            width,
            height,
        });
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "GBA Simulator",
        options,
        Box::new(|cc| Ok(Box::new(GbaApp::new(cc, initial_rom)))),
    )
}
