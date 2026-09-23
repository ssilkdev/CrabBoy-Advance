//! Game Boy Advance Simulator - Application Entry Point
//! Supports interactive GUI and headless AI Agent diagnostic execution.

pub mod dmg;
pub mod gba;
pub mod ui;

use dmg::GameBoy;
use gba::diagnostics::{HealthGrade, OverallHealthGrade};
use gba::Gba;
use ui::emu_core::ConsoleKind;
use std::env;
use std::path::PathBuf;
use ui::GbaApp;

// Force NVIDIA Optimus and AMD PowerXpress drivers to run on the high-performance
// dedicated GPU. These are exported-symbol protocols the Windows drivers look up in
// the process image; they have no meaning on Linux/macOS, where GPU selection is
// handled by the system (PRIME / DRI_PRIME env vars, or the compositor).
//
// They MUST stay Windows-only: `#[no_mangle]` puts these unmangled names in the
// dynamic symbol table, where on Linux they can collide with other objects at link
// or load time.
#[cfg(target_os = "windows")]
#[no_mangle]
#[used]
pub static NvOptimusEnablement: u32 = 1;

#[cfg(target_os = "windows")]
#[no_mangle]
#[used]
pub static AmdPowerXpressRequestHighPerformance: i32 = 1;

fn print_help() {
    println!(
        r#"🦀 CrabBoy Advance - Next-Generation Game Boy Advance Emulator

USAGE:
    crabboy-advance [ROM_PATH]                                  Launch GUI emulator (.gba, .gb, .gbc)
    crabboy-advance <ROM> --ai-play [--ai-endpoint URL]         Launch GUI with the AI Agent already playing
    crabboy-advance <ROM> --hd-pack <DIR>                       Launch GUI with HD sprite & tile pack loaded
    crabboy-advance --diagnose <ROM> [--frames N] [--output P]  Run headless diagnostics and export report
    crabboy-advance --dump-frame <ROM> [--frame N] [--output P] Run headless to frame N and save PNG screenshot
    crabboy-advance --dump-tiles <ROM> [--frame N] [--output D] Run headless to frame N and dump tiles/sprites pack
    crabboy-advance --audit-audio <ROM> [--frames N]            Run headless audio health audit
    crabboy-advance --gb-serial <GB_ROM> [--frames N]           Run a Game Boy ROM headless and print its link-port output
    crabboy-advance --help                                      Show this help message

GAME BOY / GAME BOY COLOR:
    .gb and .gbc ROMs are detected by extension and run on the SM83 core.
    --force-dmg    Run a CGB-enhanced cartridge in original Game Boy mode

OPTIONS:
    --frames <N>   Number of frames to emulate (default: 300)
    --frame <N>    Target frame to capture (default: 60)
    --output <P>   Output file path for JSON report or PNG frame

AI AGENT PLAYER (watch an AI play):
    --ai-play              Start the AI Agent immediately on launch
    --ai-endpoint <URL>    OpenAI-compatible vision endpoint
                           (default: http://127.0.0.1:8080/v1/chat/completions)
    --ai-model <NAME>      Model name sent in the request (default: local-vision)
    --ai-brain <KIND>      'vision' (default) or 'heuristic' (offline, no server)
    --ai-objective <TEXT>  Standing objective handed to the agent
    --ai-no-pause          Keep emulating while the model thinks (smoother to watch)
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

/// AI Agent options parsed from the command line, applied once the GUI boots.
#[derive(Default)]
pub struct AiLaunchOptions {
    pub autostart: bool,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub brain: Option<String>,
    pub objective: Option<String>,
    pub no_pause: bool,
}

fn parse_ai_options(args: &[String]) -> AiLaunchOptions {
    AiLaunchOptions {
        autostart: args.iter().any(|a| a == "--ai-play"),
        endpoint: get_arg_val(args, "--ai-endpoint"),
        model: get_arg_val(args, "--ai-model"),
        brain: get_arg_val(args, "--ai-brain"),
        objective: get_arg_val(args, "--ai-objective"),
        no_pause: args.iter().any(|a| a == "--ai-no-pause"),
    }
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

        // A .gb/.gbc ROM dumps a native 160x144 PNG from the SM83 core.
        if ConsoleKind::from_extension(&rom_path) == ConsoleKind::GameBoy {
            let force_dmg = args.iter().any(|a| a == "--force-dmg");
            let mut gb = match GameBoy::from_file(&rom_path, force_dmg) {
                Ok(gb) => gb,
                Err(e) => {
                    eprintln!("Failed to load GB ROM '{}': {}", rom_path.display(), e);
                    std::process::exit(1);
                }
            };
            println!(
                "Stepping to frame {} on '{}' ({})...",
                target_frame,
                rom_path.display(),
                if gb.is_cgb() { "Game Boy Color" } else { "Game Boy" }
            );
            for _ in 0..target_frame {
                gb.run_frame();
            }
            if let Err(e) = gb.dump_frame_png(&output_path) {
                eprintln!("Failed to save frame PNG: {}", e);
                std::process::exit(1);
            }
            println!("Saved frame {} to '{}'.", target_frame, output_path.display());
            std::process::exit(0);
        }

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

    // 3. Headless Tile and Sprite Dump Mode (ROADMAP M7)
    if let Some(pos) = args.iter().position(|a| a == "--dump-tiles" || a == "--dump-sprites") {
        if pos + 1 >= args.len() {
            eprintln!("Error: --dump-tiles requires a ROM path.");
            std::process::exit(1);
        }
        let rom_path = PathBuf::from(&args[pos + 1]);
        let target_frame: u64 = get_arg_val(args, "--frame")
            .or_else(|| get_arg_val(args, "--frames"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(60);
        let output_dir = get_arg_val(args, "--output")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("dumped_pack_frame_{}", target_frame)));

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

        match gba.dump_tiles_and_sprites(&output_dir) {
            Ok(manifest) => {
                println!(
                    "Dumped {} assets to '{}'. Generated manifest.json.",
                    manifest.replacements.len(),
                    output_dir.display()
                );
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("Failed to dump tiles/sprites: {}", e);
                std::process::exit(1);
            }
        }
    }

    // 4. Headless Audio Quality Audit
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

    // 4. Game Boy serial-port capture (test ROM harness)
    if let Some(pos) = args.iter().position(|a| a == "--gb-serial") {
        if pos + 1 >= args.len() {
            eprintln!("Error: --gb-serial requires a ROM path.");
            std::process::exit(1);
        }
        let rom_path = PathBuf::from(&args[pos + 1]);
        let frames: u64 = get_arg_val(args, "--frames")
            .and_then(|s| s.parse().ok())
            .unwrap_or(4000);
        let force_dmg = args.iter().any(|a| a == "--force-dmg");

        let mut gb = match GameBoy::from_file(&rom_path, force_dmg) {
            Ok(gb) => gb,
            Err(e) => {
                eprintln!("Failed to load GB ROM '{}': {}", rom_path.display(), e);
                std::process::exit(1);
            }
        };
        println!(
            "Running '{}' ({}) for up to {} frames...",
            rom_path.display(),
            if gb.is_cgb() { "Game Boy Color" } else { "Game Boy" },
            frames
        );

        for _ in 0..frames {
            gb.run_frame();
            // Blargg-style suites announce their verdict on the link port;
            // stop as soon as one appears instead of burning the full budget.
            if gb.mmu.serial_out.len() > 4 {
                let s = String::from_utf8_lossy(&gb.mmu.serial_out);
                if s.contains("Passed") || s.contains("Failed") {
                    break;
                }
            }
        }

        let out = String::from_utf8_lossy(&gb.mmu.serial_out).to_string();
        println!("--- serial output ---\n{}\n---------------------", out);
        if out.contains("Failed") {
            std::process::exit(1);
        }
        std::process::exit(0);
    }
}

fn main() -> eframe::Result<()> {
    // On Linux the eframe/winit stack pulls in D-Bus (zbus) and the AT-SPI
    // accessibility bridge, which are extremely chatty at INFO -- they emit
    // multi-kilobyte struct dumps per message and bury the emulator's own logs.
    // Windows has no such bus, so this noise is Linux-specific. Default those
    // crates to `warn` while keeping our own logging at `info`. An explicit
    // RUST_LOG still overrides everything.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(
            "info,zbus=warn,tracing=warn,atspi=warn,accesskit=warn,ashpd=warn,calloop=warn",
        ),
    )
    .init();

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

    // On Wayland the compositor identifies a window solely by its app_id and
    // ignores the pixel icon set below; it shows the icon from the installed
    // .desktop file whose basename matches. This must therefore stay in sync
    // with packaging/linux/io.github.ssilkdev.CrabBoyAdvance.desktop, or the
    // taskbar/dock falls back to a generic placeholder icon.
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        viewport = viewport.with_app_id("io.github.ssilkdev.CrabBoyAdvance");
    }

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

    let ai_opts = parse_ai_options(&args);
    let hd_pack_arg = get_arg_val(&args, "--hd-pack").map(PathBuf::from);

    eframe::run_native(
        "GBA Simulator",
        options,
        Box::new(move |cc| {
            let mut app = GbaApp::new(cc, initial_rom);
            if let Some(ref pack_dir) = hd_pack_arg {
                match crate::gba::hd_pack::HdPack::load_from_dir(pack_dir) {
                    Ok(pack) => {
                        log::info!("Loaded HD pack '{}' with {} replacements", pack.name, pack.len());
                        app.gba.load_hd_pack(pack);
                        app.hd_pack_path = Some(pack_dir.clone());
                        app.hd_pack_enabled = true;
                    }
                    Err(e) => {
                        log::error!("Failed to load HD pack from '{}': {}", pack_dir.display(), e);
                    }
                }
            }
            let summary = app.apply_ai_launch_options(
                ai_opts.endpoint.as_deref(),
                ai_opts.model.as_deref(),
                ai_opts.brain.as_deref(),
                ai_opts.objective.as_deref(),
                ai_opts.no_pause,
                ai_opts.autostart,
            );
            if ai_opts.autostart {
                log::info!("AI Agent autostarted: {}", summary);
            }
            Ok(Box::new(app))
        }),
    )
}
