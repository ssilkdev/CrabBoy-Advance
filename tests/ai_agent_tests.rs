//! End-to-end test for the AI Agent Player.
//!
//! Stands up a real HTTP server that speaks the OpenAI `/v1/chat/completions`
//! shape (the same contract llama.cpp's server exposes with `--mmproj`), points
//! the agent at it, and drives a real `Gba` instance. This exercises the whole
//! path: framebuffer -> PNG -> base64 -> curl POST -> JSON plan -> keypad bits.

use gba_simulator::gba::keypad::Key;
use gba_simulator::gba::Gba;
use gba_simulator::ui::ai_agent::{AiAgent, Brain};

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Minimal blocking HTTP/1.1 server that replies with `body` to every POST.
/// Returns (port, requests_served_counter, received_payload_sizes).
fn spawn_mock_server(responses: Vec<String>) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_thread = Arc::clone(&counter);

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let idx = counter_thread.fetch_add(1, Ordering::SeqCst);

            // Parse just enough HTTP to consume the body and confirm the image
            // actually arrived.
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut content_length = 0usize;
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if line == "\r\n" || line == "\n" {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; content_length];
            let _ = reader.read_exact(&mut body);

            // The request must carry a base64 PNG data URL.
            let body_str = String::from_utf8_lossy(&body);
            let has_image = body_str.contains("data:image/png;base64,iVBOR");

            let payload = if !has_image {
                r#"{"error":{"message":"no image in request"}}"#.to_string()
            } else {
                let content = responses
                    .get(idx)
                    .cloned()
                    .unwrap_or_else(|| responses.last().cloned().unwrap_or_default());
                serde_json::json!({
                    "choices": [ { "message": { "role": "assistant", "content": content } } ]
                })
                .to_string()
            };

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });

    (port, counter)
}

fn synthetic_rom() -> Vec<u8> {
    // Infinite-loop ROM: `b .` at the entry point. Enough for the PPU to
    // produce frames so the agent has something to screenshot.
    let mut rom = vec![0u8; 0x1000];
    // ARM: B (PC-8) -> 0xEAFFFFFE
    rom[0..4].copy_from_slice(&0xEAFF_FFFEu32.to_le_bytes());
    rom
}

fn agent_pointed_at(port: u16) -> AiAgent {
    let mut agent = AiAgent::new();
    agent.config.brain = Brain::VisionModel;
    agent.config.endpoint = format!("http://127.0.0.1:{}/v1/chat/completions", port);
    agent.config.model = "mock-vision".to_string();
    agent.config.log_transcript = false;
    agent.config.pause_while_thinking = true;
    agent.config.request_timeout_secs = 20;
    agent.config.image_scale = 2;
    agent
}

/// Runs the agent against a live mock server until `decisions` decisions land.
fn pump(agent: &mut AiAgent, gba: &mut Gba, decisions: u64, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while agent.decisions < decisions {
        assert!(
            Instant::now() < deadline,
            "timed out after {} decisions (status: {})",
            agent.decisions,
            agent.status.label()
        );
        agent.poll(gba.frame_counter);
        agent.maybe_request(gba, "TEST_ROM");
        agent.apply_inputs(gba);

        if !agent.is_thinking() {
            gba.run_frame();
            agent.tick(1);
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn agent_drives_the_keypad_from_a_live_vision_endpoint() {
    let (port, served) = spawn_mock_server(vec![
        r#"{"observation":"Title screen is showing","goal":"Start the game","actions":[{"buttons":["START"],"frames":4}]}"#.to_string(),
        // Second reply is deliberately messy: reasoning trace + markdown fence.
        "<think>Now a dialog. Press A.</think>\n```json\n{\"observation\":\"Dialog box\",\"goal\":\"Advance text\",\"actions\":[{\"buttons\":[\"A\",\"RIGHT\"],\"frames\":6}]}\n```".to_string(),
    ]);

    let mut gba = Gba::new();
    gba.load_rom_bytes(synthetic_rom());

    let mut agent = agent_pointed_at(port);
    agent.start("TEST_ROM");
    assert!(agent.enabled);

    // --- First decision: expect START held -----------------------------
    let deadline = Instant::now() + Duration::from_secs(30);
    while agent.decisions < 1 {
        assert!(Instant::now() < deadline, "no first decision: {}", agent.status.label());
        agent.poll(gba.frame_counter);
        agent.maybe_request(&gba, "TEST_ROM");
        agent.apply_inputs(&mut gba);
        if !agent.is_thinking() {
            gba.run_frame();
            agent.tick(1);
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    assert_eq!(agent.last_observation, "Title screen is showing");
    assert_eq!(agent.last_goal, "Start the game");

    // Load the plan's first action into the current slot and push it to the pad.
    agent.tick(1);
    agent.apply_inputs(&mut gba);
    assert!(
        gba.mmu.keypad.is_key_pressed(Key::Start),
        "START must be physically pressed in the emulated keypad"
    );
    assert!(!gba.mmu.keypad.is_key_pressed(Key::A), "A must not be pressed yet");

    // --- Second decision: messy output, simultaneous A+RIGHT -----------
    pump(&mut agent, &mut gba, 2, Duration::from_secs(30));
    agent.tick(1);
    agent.apply_inputs(&mut gba);

    assert_eq!(agent.last_goal, "Advance text");
    assert!(
        gba.mmu.keypad.is_key_pressed(Key::A) && gba.mmu.keypad.is_key_pressed(Key::Right),
        "agent must be able to hold two buttons at once"
    );
    assert!(!gba.mmu.keypad.is_key_pressed(Key::Start), "START released");

    assert!(served.load(Ordering::SeqCst) >= 2, "server should have served 2+ requests");

    // --- Stopping the agent releases the pad ---------------------------
    agent.stop();
    agent.apply_inputs(&mut gba);
    // stop() clears the plan; a fresh apply must not re-press anything.
    let mut fresh = Gba::new();
    fresh.load_rom_bytes(synthetic_rom());
    agent.apply_inputs(&mut fresh);
    assert_eq!(fresh.mmu.keypad.read_keyinput(), 0x03FF, "all keys released");
}

#[test]
fn agent_survives_a_dead_endpoint_without_stalling_the_emulator() {
    // Bind then immediately drop the listener so the port refuses connections.
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };

    let mut gba = Gba::new();
    gba.load_rom_bytes(synthetic_rom());

    let mut agent = agent_pointed_at(port);
    agent.config.request_timeout_secs = 5;
    agent.start("TEST_ROM");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut frames_emulated = 0u32;
    let mut saw_error = false;

    while Instant::now() < deadline {
        agent.poll(gba.frame_counter);
        agent.maybe_request(&gba, "TEST_ROM");
        agent.apply_inputs(&mut gba);

        if !agent.is_thinking() {
            gba.run_frame();
            agent.tick(1);
            frames_emulated += 1;
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }

        if matches!(agent.status, gba_simulator::ui::ai_agent::AgentStatus::Error(_)) {
            saw_error = true;
        }
        if saw_error && frames_emulated > 30 {
            break;
        }
    }

    assert!(saw_error, "a refused connection must surface as an agent error");
    assert!(
        frames_emulated > 30,
        "emulation must keep running after the endpoint fails (ran {} frames)",
        frames_emulated
    );
    // The backoff wait action must not leave buttons stuck down.
    assert_eq!(
        agent.held_buttons(),
        [false; 10],
        "no button may be stuck held during error backoff"
    );
}

#[test]
fn heuristic_brain_plays_with_no_server_at_all() {
    let mut gba = Gba::new();
    gba.load_rom_bytes(synthetic_rom());

    let mut agent = AiAgent::new();
    agent.config.brain = Brain::Heuristic;
    agent.config.log_transcript = false;
    agent.start("TEST_ROM");

    let mut any_button_seen = false;
    for _ in 0..600 {
        agent.poll(gba.frame_counter);
        agent.maybe_request(&gba, "TEST_ROM");
        agent.apply_inputs(&mut gba);
        gba.run_frame();
        agent.tick(1);
        if agent.held_buttons().iter().any(|&b| b) {
            any_button_seen = true;
        }
    }

    assert!(agent.decisions > 0, "heuristic brain must produce decisions offline");
    assert!(any_button_seen, "heuristic brain must actually press buttons");
    assert_eq!(agent.last_latency_ms, 0, "offline brain has no network latency");
}

/// Reasoning models (Qwen3, DeepSeek-R1) put their chain of thought in
/// `reasoning_content` and the answer in `content`. When the token budget runs
/// out mid-thought the server returns finish_reason=length with an EMPTY
/// `content` — which must produce an actionable error, not a silent hang.
#[test]
fn truncated_reasoning_model_reply_yields_an_actionable_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut len = 0usize;
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 { break; }
                if line == "\r\n" { break; }
                let l = line.to_ascii_lowercase();
                if let Some(v) = l.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; len];
            let _ = reader.read_exact(&mut body);

            // Exactly what llama.cpp returns for a truncated reasoning model.
            let payload = serde_json::json!({
                "choices": [{
                    "finish_reason": "length",
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "reasoning_content": "The user wants a precise description of a GBA screen. Let me examine the image systematically. Top portion: there's a horizontal bar"
                    }
                }],
                "usage": {"completion_tokens": 120}
            }).to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(), payload);
            let _ = stream.write_all(resp.as_bytes());
        }
    });

    let mut gba = Gba::new();
    gba.load_rom_bytes(synthetic_rom());
    let mut agent = agent_pointed_at(port);
    agent.config.max_tokens = 400;
    agent.start("TEST_ROM");

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(Instant::now() < deadline, "never surfaced an error");
        agent.poll(gba.frame_counter);
        agent.maybe_request(&gba, "TEST_ROM");
        agent.apply_inputs(&mut gba);
        if !agent.is_thinking() { gba.run_frame(); agent.tick(1); }
        else { std::thread::sleep(Duration::from_millis(5)); }

        if let gba_simulator::ui::ai_agent::AgentStatus::Error(e) = &agent.status {
            assert!(e.contains("tokens"), "error must name the token budget, got: {}", e);
            assert!(e.contains("400"), "error must report the configured budget, got: {}", e);
            break;
        }
    }
}

/// If a server puts the whole answer in `reasoning_content` and leaves
/// `content` empty (without truncating), recover the plan rather than failing.
#[test]
fn plan_is_recovered_from_reasoning_content_when_content_is_empty() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut len = 0usize;
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 { break; }
                if line == "\r\n" { break; }
                let l = line.to_ascii_lowercase();
                if let Some(v) = l.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; len];
            let _ = reader.read_exact(&mut body);

            let payload = serde_json::json!({
                "choices": [{
                    "finish_reason": "stop",
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "reasoning_content": "I should press START here.\n{\"observation\":\"Title screen\",\"goal\":\"Begin\",\"actions\":[{\"buttons\":[\"START\"],\"frames\":5}]}"
                    }
                }]
            }).to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(), payload);
            let _ = stream.write_all(resp.as_bytes());
        }
    });

    let mut gba = Gba::new();
    gba.load_rom_bytes(synthetic_rom());
    let mut agent = agent_pointed_at(port);
    agent.start("TEST_ROM");

    pump(&mut agent, &mut gba, 1, Duration::from_secs(30));
    agent.tick(1);
    agent.apply_inputs(&mut gba);

    assert_eq!(agent.last_goal, "Begin");
    assert!(gba.mmu.keypad.is_key_pressed(Key::Start));
}

/// The release profile sets `panic = "abort"`, so a panic on the inference
/// worker thread kills the whole emulator — not just the agent. Model output is
/// untrusted input, so `parse_plan` must be total: every input either returns a
/// plan or an Err, and never panics, overflows, or hangs.
#[test]
fn parse_plan_never_panics_on_hostile_input() {
    let mut cases: Vec<String> = vec![
        // Structural garbage
        String::new(),
        " ".repeat(10_000),
        "{".to_string(),
        "}".to_string(),
        "{{{{{{{{{{".to_string(),
        "}}}}}}}}}}".to_string(),
        "[]".to_string(),
        "null".to_string(),
        "\0\0\0".to_string(),
        // Unterminated strings / escapes
        r#"{"observation":"unterminated"#.to_string(),
        r#"{"observation":"\"#.to_string(),
        r#"{"actions":[{"buttons":["A\"#.to_string(),
        // Unclosed reasoning span
        "<think>forever and ever".to_string(),
        "<think></think>".to_string(),
        // Wrong types where objects/arrays/strings are expected
        r#"{"actions":"not an array"}"#.to_string(),
        r#"{"actions":[1,2,3]}"#.to_string(),
        r#"{"actions":[{"buttons":123,"frames":"abc"}]}"#.to_string(),
        r#"{"actions":[{"buttons":[null,true,{}],"frames":{}}]}"#.to_string(),
        r#"{"actions":[[]]}"#.to_string(),
        // Numeric edge cases: overflow, negatives, floats
        r#"{"actions":[{"buttons":["A"],"frames":99999999999999999999}]}"#.to_string(),
        r#"{"actions":[{"buttons":["A"],"frames":-5}]}"#.to_string(),
        r#"{"actions":[{"buttons":["A"],"frames":1e400}]}"#.to_string(),
        r#"{"actions":[{"buttons":["A"],"frames":0}]}"#.to_string(),
        // Unicode / multi-byte boundaries (truncate() must not split a char)
        "🦀".repeat(500),
        format!(r#"{{"observation":"{}","actions":[{{"buttons":["A"],"frames":4}}]}}"#, "🦀".repeat(300)),
        "\u{202e}\u{200b}".repeat(100),
    ];

    // Deeply nested braces: the balanced-span scanner must not recurse.
    cases.push(format!("{}{}", "{".repeat(5_000), "}".repeat(5_000)));
    // A huge but well-formed action list (bounded by the parser's take()).
    let many: Vec<String> = (0..2_000)
        .map(|_| r#"{"buttons":["A"],"frames":4}"#.to_string())
        .collect();
    cases.push(format!(r#"{{"actions":[{}]}}"#, many.join(",")));

    for case in &cases {
        // The contract is simply: return, don't panic. Err is a fine outcome.
        match gba_simulator::ui::ai_agent::parse_plan(case) {
            Ok(plan) => {
                assert!(!plan.actions.is_empty(), "an Ok plan must have actions");
                for a in &plan.actions {
                    assert!(a.frames >= 1, "frames must be clamped to >= 1");
                    for &b in &a.buttons {
                        assert!(b < 10, "button index must be in range, got {}", b);
                    }
                }
            }
            Err(e) => {
                assert!(!e.is_empty(), "an error must carry a message");
            }
        }
    }
}

/// Randomized fuzz over `parse_plan`. Deterministic (fixed seed) so a failure
/// is reproducible. Mutates valid plans and emits random byte soup, including
/// multi-byte UTF-8, to shake out panics the hand-written cases miss.
#[test]
fn parse_plan_fuzz_is_total() {
    // xorshift64*, same generator the heuristic brain uses.
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };

    let alphabet: Vec<char> = r#"{}[]",:0123456789.eE-+ ABLRSTU\/"#
        .chars()
        .chain("🦀é\u{0}\u{202e}<think></think>`".chars())
        .collect();

    let seed_plan = r#"{"observation":"o","goal":"g","actions":[{"buttons":["A"],"frames":4}]}"#;

    for i in 0..20_000u32 {
        let input: String = if i % 3 == 0 {
            // Pure random soup.
            let len = (next() % 200) as usize;
            (0..len)
                .map(|_| alphabet[(next() as usize) % alphabet.len()])
                .collect()
        } else {
            // Mutate a valid plan: chop it and splice in random chars.
            let mut s: Vec<char> = seed_plan.chars().collect();
            let muts = 1 + (next() % 6) as usize;
            for _ in 0..muts {
                if s.is_empty() {
                    break;
                }
                let idx = (next() as usize) % s.len();
                match next() % 3 {
                    0 => { s.remove(idx); }
                    1 => { s[idx] = alphabet[(next() as usize) % alphabet.len()]; }
                    _ => { s.truncate(idx); }
                }
            }
            s.into_iter().collect()
        };

        // Must return, not panic. Validate any accepted plan's invariants.
        if let Ok(plan) = gba_simulator::ui::ai_agent::parse_plan(&input) {
            assert!(!plan.actions.is_empty(), "input {:?}", input);
            for a in &plan.actions {
                assert!(a.frames >= 1);
                for &b in &a.buttons {
                    assert!(b < 10, "bad button index from input {:?}", input);
                }
            }
        }
    }
}
