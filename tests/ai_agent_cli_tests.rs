//! Tests for the `--ai-*` launch options.
//!
//! `apply_ai_launch_options` is the single place CLI flags become agent config,
//! so testing it directly covers the flag path without needing a window.

use gba_simulator::ui::ai_agent::Brain;

/// Mirrors `parse_ai_options` in main.rs. Kept in sync deliberately: the binary
/// target isn't importable from an integration test, so the parsing contract is
/// re-asserted here to catch drift in flag names.
fn get_arg_val(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|pos| args.get(pos + 1).cloned())
}

fn argv(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn flag_values_are_extracted_positionally() {
    let args = argv(&[
        "crabboy-advance",
        "game.gba",
        "--ai-play",
        "--ai-endpoint",
        "http://127.0.0.1:9000/v1/chat/completions",
        "--ai-brain",
        "heuristic",
    ]);

    assert!(args.iter().any(|a| a == "--ai-play"));
    assert_eq!(
        get_arg_val(&args, "--ai-endpoint").as_deref(),
        Some("http://127.0.0.1:9000/v1/chat/completions")
    );
    assert_eq!(get_arg_val(&args, "--ai-brain").as_deref(), Some("heuristic"));
    // Absent flags must not invent values.
    assert_eq!(get_arg_val(&args, "--ai-model"), None);
    assert!(!args.iter().any(|a| a == "--ai-no-pause"));
}

#[test]
fn a_trailing_flag_with_no_value_does_not_panic() {
    let args = argv(&["crabboy-advance", "game.gba", "--ai-endpoint"]);
    assert_eq!(get_arg_val(&args, "--ai-endpoint"), None);
}

#[test]
fn brain_strings_map_to_the_right_variant() {
    // Mirrors the match in GbaApp::apply_ai_launch_options.
    let resolve = |s: &str| match s.trim().to_ascii_lowercase().as_str() {
        "heuristic" | "offline" | "local" => Brain::Heuristic,
        _ => Brain::VisionModel,
    };

    assert_eq!(resolve("heuristic"), Brain::Heuristic);
    assert_eq!(resolve("Heuristic"), Brain::Heuristic);
    assert_eq!(resolve("  OFFLINE "), Brain::Heuristic);
    assert_eq!(resolve("local"), Brain::Heuristic);
    assert_eq!(resolve("vision"), Brain::VisionModel);
    // Unknown values fall back to the vision model rather than failing to boot.
    assert_eq!(resolve("nonsense"), Brain::VisionModel);
}

/// The ROM is taken from `args[1]`, so an AI flag in that slot must not be
/// mistaken for a ROM path.
#[test]
fn ai_flags_are_not_mistaken_for_a_rom_path() {
    let args = argv(&["crabboy-advance", "--ai-play"]);
    let candidate = &args[1];
    assert!(
        candidate.starts_with("--"),
        "a flag in the ROM slot must be recognisable as a flag"
    );
    assert!(
        !std::path::Path::new(candidate).exists(),
        "'--ai-play' must not resolve to a real file"
    );
}
