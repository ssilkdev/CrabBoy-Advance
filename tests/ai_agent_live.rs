//! Manual harness: drive a REAL ROM with the AI agent against a real HTTP
//! vision endpoint, and dump the resulting frames to disk.
//!
//! Not part of the default test run — it needs a ROM and a live server.
//!
//!   python3 scripts/mock_vision_server.py --port 8099 &
//!   GBA_TEST_ROM="/path/to/game.gba" \
//!   AI_AGENT_ENDPOINT="http://127.0.0.1:8099/v1/chat/completions" \
//!   cargo test --test ai_agent_live -- --ignored --nocapture

use gba_simulator::gba::Gba;
use gba_simulator::ui::ai_agent::{AiAgent, Brain, KEY_NAMES};
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires GBA_TEST_ROM and a live vision endpoint"]
fn agent_plays_a_real_rom() {
    let rom = std::env::var("GBA_TEST_ROM").expect("set GBA_TEST_ROM");
    let endpoint = std::env::var("AI_AGENT_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:8099/v1/chat/completions".to_string());
    let decisions_wanted: u64 = std::env::var("AI_AGENT_DECISIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    let outdir = std::env::var("AI_AGENT_OUTDIR")
        .unwrap_or_else(|_| "/tmp/crabboy_ai_play".to_string());
    std::fs::create_dir_all(&outdir).unwrap();

    let mut gba = Gba::new();
    gba.load_rom(&rom).expect("load ROM");
    println!("Loaded ROM: {}", rom);

    // Let the BIOS/intro get somewhere before handing over control.
    for _ in 0..180 {
        gba.run_frame();
    }

    let mut agent = AiAgent::new();
    agent.config.brain = Brain::VisionModel;
    agent.config.endpoint = endpoint.clone();
    agent.config.model = "mock-vision".to_string();
    agent.config.image_scale = 3;
    agent.config.pause_while_thinking = true;
    agent.config.log_transcript = true;
    agent.start("LiveHarness");
    println!("Agent started against {}", endpoint);

    let deadline = Instant::now() + Duration::from_secs(120);
    let mut frames = 0u64;
    let mut last_reported = 0u64;

    while agent.decisions < decisions_wanted {
        assert!(
            Instant::now() < deadline,
            "timed out; status = {}",
            agent.status.label()
        );

        agent.poll(gba.frame_counter);
        agent.maybe_request(&gba, "LiveHarness");
        agent.apply_inputs(&mut gba);

        if agent.is_thinking() {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }

        gba.run_frame();
        agent.tick(1);
        frames += 1;

        if agent.decisions > last_reported {
            last_reported = agent.decisions;
            let held: Vec<&str> = agent
                .held_buttons()
                .iter()
                .enumerate()
                .filter(|(_, &h)| h)
                .map(|(i, _)| KEY_NAMES[i])
                .collect();
            println!(
                "\n── decision {} ({} ms) ──\n  sees : {}\n  wants: {}\n  holds: {}",
                agent.decisions,
                agent.last_latency_ms,
                agent.last_observation,
                agent.last_goal,
                if held.is_empty() { "—".to_string() } else { held.join("+") },
            );
            let path = format!("{}/decision_{:02}.png", outdir, agent.decisions);
            gba.dump_frame_png(&path).expect("dump png");
            println!("  frame: {}", path);
        }

        if frames % 60 == 0 {
            let held: Vec<&str> = agent
                .held_buttons()
                .iter()
                .enumerate()
                .filter(|(_, &h)| h)
                .map(|(i, _)| KEY_NAMES[i])
                .collect();
            println!(
                "  [frame {}] status={} holding={}",
                frames,
                agent.status.label(),
                if held.is_empty() { "—".to_string() } else { held.join("+") }
            );
        }
    }

    let final_png = format!("{}/final.png", outdir);
    gba.dump_frame_png(&final_png).unwrap();

    println!("\n=== SUMMARY ===");
    println!("emulated frames : {}", frames);
    println!("decisions       : {}", agent.decisions);
    println!("inputs executed : {}", agent.actions_executed);
    println!("final frame     : {}", final_png);
    if let Some(t) = agent.transcript_display() {
        println!("transcript      : {}", t);
    }

    assert!(agent.decisions >= decisions_wanted);
    assert!(agent.actions_executed > 0, "agent must have executed inputs");
    assert!(
        !matches!(agent.status, gba_simulator::ui::ai_agent::AgentStatus::Error(_)),
        "agent ended in error: {}",
        agent.status.label()
    );
}

/// Offline path on a REAL ROM: no server involved at all.
#[test]
#[ignore = "requires GBA_TEST_ROM"]
fn heuristic_agent_plays_a_real_rom_offline() {
    let rom = std::env::var("GBA_TEST_ROM").expect("set GBA_TEST_ROM");
    let mut gba = Gba::new();
    gba.load_rom(&rom).expect("load ROM");
    for _ in 0..180 { gba.run_frame(); }

    let mut agent = AiAgent::new();
    agent.config.brain = Brain::Heuristic;
    agent.config.log_transcript = false;
    agent.start("HeuristicHarness");

    let mut presses = 0u32;
    for _ in 0..1200 {
        agent.poll(gba.frame_counter);
        agent.maybe_request(&gba, "HeuristicHarness");
        agent.apply_inputs(&mut gba);
        gba.run_frame();
        agent.tick(1);
        if agent.held_buttons().iter().any(|&b| b) { presses += 1; }
    }

    let out = "/tmp/crabboy_ai_play/heuristic_final.png";
    std::fs::create_dir_all("/tmp/crabboy_ai_play").ok();
    gba.dump_frame_png(out).unwrap();
    println!("decisions={} inputs={} frames_with_button_held={}", agent.decisions, agent.actions_executed, presses);
    println!("final frame: {}", out);

    assert!(agent.decisions > 0);
    assert!(presses > 100, "heuristic brain should hold buttons often");
}
