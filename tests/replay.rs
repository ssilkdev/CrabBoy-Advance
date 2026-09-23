//! ROADMAP M2: movie replay with per-frame hashes, runnable in CI on every
//! platform (the program and movie live in `gba_simulator::gba::replay`).
//!
//! Checks the replay against:
//! - a second, independent run (determinism);
//! - a run that is saved mid-movie, loaded into a fresh core and finished
//!   (state completeness);
//! - golden hashes in `tests/data/replay_golden.txt`. Set `CRABBOY_BLESS=1`
//!   to rewrite that file after an intentional change to emulation output.
//!
//! `CRABBOY_REPLAY_PNG=path` also dumps the final frame for a visual check.

use gba_simulator::gba::replay::{boot, fnv, movie, parse_movie, play, report_text};
use std::path::PathBuf;

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/replay_golden.txt")
}

#[test]
fn replay_is_deterministic_and_matches_golden() {
    let mv = parse_movie(&movie());
    let n = mv.len();
    let (v1, a1) = play(&mut boot(), &mv, 0, n);
    assert_ne!(a1, fnv([]), "no audio was captured");
    let (v2, a2) = play(&mut boot(), &mv, 0, n);
    assert_eq!(v1, v2, "two replays of the same movie differ");
    assert_eq!(a1, a2, "audio differs between replays");

    // The movie must actually drive the program: many distinct frames.
    let distinct: std::collections::HashSet<_> = v1.iter().collect();
    assert!(distinct.len() > n / 4, "only {} distinct frames: ROM not reacting", distinct.len());

    if let Ok(out) = std::env::var("CRABBOY_REPLAY_PNG") {
        let mut g = boot();
        play(&mut g, &mv, 0, n);
        g.dump_frame_png(&out).unwrap();
    }

    // Save mid-movie, load into a fresh core, finish: must match.
    let mut g = boot();
    let (head, _) = play(&mut g, &mv, 0, n / 2);
    let st = g.save_state();
    let mut g2 = boot();
    assert!(g2.load_state(&st));
    let (tail, _) = play(&mut g2, &mv, n / 2, n - n / 2);
    let mut joined = head;
    joined.extend(tail);
    if let Some(i) = joined.iter().zip(&v1).position(|(a, b)| a != b) {
        panic!("state restore diverges at movie frame {i}");
    }

    let text = report_text(&v1, a1);
    let path = golden_path();
    if std::env::var_os("CRABBOY_BLESS").is_some() || !path.exists() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
        eprintln!("wrote golden hashes to {}", path.display());
    } else {
        // Normalise line endings so a Windows checkout compares equal.
        let want = std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
        if want != text {
            let diff = want.lines().zip(text.lines()).find(|(a, b)| a != b);
            panic!(
                "replay output changed from tests/data/replay_golden.txt (first difference: {diff:?}).\n\
                 If the change is intentional, rerun with CRABBOY_BLESS=1 and commit the file."
            );
        }
    }
}
